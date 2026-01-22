use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::Utc;
use petname::petname;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    error::Error,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};
use tokio::sync::oneshot;
use workforest_core::{data_dir, repos_config_path, RepoConfig, RepoConfigFile};

#[derive(Clone)]
struct AppState {
    shutdown_sender: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
    db: Arc<tokio::sync::Mutex<Connection>>,
}

#[derive(Serialize)]
struct ServerMetadata {
    pid: u32,
    port: u16,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Agent {
    name: String,
    label: String,
    repo: String,
    tool: String,
    status: String,
    worktree_path: String,
    styles: Option<serde_json::Value>,
    output: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct AddRepoRequest {
    path: String,
}

#[derive(Deserialize)]
struct AddAgentRequest {
    repo: String,
    tool: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

#[derive(Serialize)]
struct AgentOutput {
    name: String,
    status: String,
    output: Option<String>,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let db = init_database()?;
    let state = AppState {
        shutdown_sender: Arc::new(tokio::sync::Mutex::new(Some(shutdown_sender))),
        db: Arc::new(tokio::sync::Mutex::new(db)),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/shutdown", get(shutdown))
        .route("/repos", get(list_repos).post(add_repo))
        .route("/agents", get(list_agents).post(add_agent))
        .route("/agents/output", get(agents_output))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = listener.local_addr()?;

    write_metadata(local_addr)?;

    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver))
        .await?;

    remove_metadata();

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn list_repos() -> Result<Json<Vec<RepoConfig>>, ApiError> {
    let config = load_repo_config()?;
    Ok(Json(config.repos))
}

async fn add_repo(Json(request): Json<AddRepoRequest>) -> Result<Json<RepoConfig>, ApiError> {
    let repo_path = PathBuf::from(request.path.trim());
    if repo_path.as_os_str().is_empty() {
        return Err(ApiError::bad_request("repo path is required"));
    }
    if !repo_path.exists() {
        return Err(ApiError::bad_request("repo path does not exist"));
    }
    if !is_git_repo(&repo_path) {
        return Err(ApiError::bad_request("path is not a git repo"));
    }

    let mut config = load_repo_config()?;
    let name = generate_repo_name(&repo_path, &config.repos)?;

    let repo = RepoConfig {
        name: name.clone(),
        path: repo_path,
        tools: default_tools(),
        default_tool: "opencode".to_string(),
    };

    config.repos.push(repo.clone());
    save_repo_config(&config)?;

    Ok(Json(repo))
}

async fn list_agents(State(state): State<AppState>) -> Result<Json<Vec<Agent>>, ApiError> {
    let conn = state.db.lock().await;
    let mut stmt = conn
        .prepare(
            "SELECT name, label, repo, tool, status, worktree_path, styles, created_at, updated_at FROM agents ORDER BY created_at DESC",
        )
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let agents = stmt
        .query_map([], |row| {
            let styles: Option<String> = row.get(6)?;
            let styles =
                styles.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok());
            Ok(Agent {
                name: row.get(0)?,
                label: row.get(1)?,
                repo: row.get(2)?,
                tool: row.get(3)?,
                status: row.get(4)?,
                worktree_path: row.get(5)?,
                styles,
                output: None,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let mut results = Vec::new();
    for agent in agents {
        results.push(agent.map_err(|err| ApiError::internal(err.to_string()))?);
    }

    Ok(Json(results))
}

async fn agents_output(State(state): State<AppState>) -> Result<Json<Vec<AgentOutput>>, ApiError> {
    let conn = state.db.lock().await;
    let mut stmt = conn
        .prepare("SELECT name FROM agents ORDER BY created_at DESC")
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let agents = stmt
        .query_map([], |row| Ok(row.get::<_, String>(0)?))
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let mut outputs = Vec::new();
    for agent in agents {
        let name = agent.map_err(|err| ApiError::internal(err.to_string()))?;
        let status = tmux_session_status(&name);
        let output = if status == "running" {
            tmux_output(&name, 20)
        } else {
            None
        };
        outputs.push(AgentOutput {
            name: name.clone(),
            status,
            output,
        });
    }

    Ok(Json(outputs))
}

fn tmux_session_status(agent_name: &str) -> String {
    let status = Command::new("tmux")
        .args(["has-session", "-t", agent_name])
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(status) if status.success() => "running".to_string(),
        _ => "sleep".to_string(),
    }
}

fn tmux_output(agent_name: &str, lines: usize) -> Option<String> {
    if lines == 0 {
        return None;
    }

    let start = format!("-{}", lines);
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-t", agent_name, "-S", &start])
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let content = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string();
    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

async fn add_agent(
    State(state): State<AppState>,
    Json(request): Json<AddAgentRequest>,
) -> Result<Json<Agent>, ApiError> {
    let config = load_repo_config()?;
    let repo = config
        .repos
        .iter()
        .find(|repo| repo.name == request.repo)
        .ok_or_else(|| ApiError::not_found("repo not found"))?;

    if request.tool.trim().is_empty() {
        return Err(ApiError::bad_request("tool is required"));
    }

    if !repo.tools.iter().any(|tool| tool == &request.tool) {
        return Err(ApiError::bad_request("tool not configured for repo"));
    }

    let agent_name = generate_unique_agent_name(state.db.clone()).await?;
    let label = agent_name.clone();
    let worktree_path = create_worktree(&repo.path, &repo.name, &agent_name)?;
    start_tool_session(&agent_name, &request.tool, &worktree_path)?;
    let now = Utc::now().to_rfc3339();

    let agent = Agent {
        name: agent_name,
        label,
        repo: repo.name.clone(),
        tool: request.tool,
        status: "running".to_string(),
        worktree_path: worktree_path.to_string_lossy().to_string(),
        styles: None,
        output: None,
        created_at: now.clone(),
        updated_at: now,
    };

    let conn = state.db.lock().await;
    conn.execute(
        "INSERT INTO agents (name, label, repo, tool, status, worktree_path, styles, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            agent.name,
            agent.label,
            agent.repo,
            agent.tool,
            agent.status,
            agent.worktree_path,
            agent
                .styles
                .as_ref()
                .map(|value| value.to_string()),
            agent.created_at,
            agent.updated_at,
        ],
    )
    .map_err(|err| ApiError::internal(err.to_string()))?;

    Ok(Json(agent))
}

async fn shutdown(State(state): State<AppState>) -> &'static str {
    let mut sender_guard = state.shutdown_sender.lock().await;
    if let Some(sender) = sender_guard.take() {
        let _ = sender.send(());
    }
    "shutting down"
}

async fn wait_for_shutdown(mut shutdown_receiver: oneshot::Receiver<()>) {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = &mut shutdown_receiver => {},
    }
}

fn init_database() -> Result<Connection, Box<dyn Error>> {
    let data_dir = data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("app.db");
    let conn = Connection::open(db_path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agents (
            name TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            repo TEXT NOT NULL,
            tool TEXT NOT NULL,
            status TEXT NOT NULL,
            worktree_path TEXT NOT NULL,
            styles TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        [],
    )?;
    Ok(conn)
}

fn load_repo_config() -> Result<RepoConfigFile, ApiError> {
    let path = repos_config_path();
    if !path.exists() {
        return Ok(RepoConfigFile::default());
    }
    let data = std::fs::read_to_string(path).map_err(|err| ApiError::internal(err.to_string()))?;
    toml::from_str(&data).map_err(|err| ApiError::internal(err.to_string()))
}

fn save_repo_config(config: &RepoConfigFile) -> Result<(), ApiError> {
    let config_path = repos_config_path();
    let config_dir = config_path
        .parent()
        .ok_or_else(|| ApiError::internal("config dir missing"))?;
    std::fs::create_dir_all(config_dir).map_err(|err| ApiError::internal(err.to_string()))?;
    let data = toml::to_string_pretty(config).map_err(|err| ApiError::internal(err.to_string()))?;
    std::fs::write(config_path, data).map_err(|err| ApiError::internal(err.to_string()))?;
    Ok(())
}

fn default_tools() -> Vec<String> {
    vec![
        "opencode".to_string(),
        "claude".to_string(),
        "codex".to_string(),
    ]
}

fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

fn generate_repo_name(path: &Path, repos: &[RepoConfig]) -> Result<String, ApiError> {
    let base_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ApiError::bad_request("invalid repo path"))?;

    Ok(generate_repo_name_with_suffix(base_name, repos, || {
        petname(2, "-")
    }))
}

fn generate_repo_name_with_suffix<F>(
    base_name: &str,
    repos: &[RepoConfig],
    mut suffix_fn: F,
) -> String
where
    F: FnMut() -> String,
{
    let existing: HashSet<String> = repos.iter().map(|repo| repo.name.clone()).collect();

    if !existing.contains(base_name) {
        return base_name.to_string();
    }

    loop {
        let candidate = format!("{}-{}", base_name, suffix_fn());
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
}

async fn generate_unique_agent_name(
    db: Arc<tokio::sync::Mutex<Connection>>,
) -> Result<String, ApiError> {
    let conn = db.lock().await;
    loop {
        let candidate = petname(2, "-");
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE name = ?1)",
                params![candidate.as_str()],
                |row| row.get(0),
            )
            .map_err(|err| ApiError::internal(err.to_string()))?;
        if !exists {
            return Ok(candidate);
        }
    }
}

fn create_worktree(
    repo_path: &Path,
    repo_name: &str,
    agent_name: &str,
) -> Result<PathBuf, ApiError> {
    let trees_dir = data_dir().join("trees");
    std::fs::create_dir_all(&trees_dir).map_err(|err| ApiError::internal(err.to_string()))?;
    let kebab_name = to_kebab(agent_name);
    let worktree_path = trees_dir.join(format!("{}-{}", repo_name, kebab_name));

    if worktree_path.exists() {
        return Err(ApiError::bad_request("worktree already exists"));
    }

    let branch_name = format!("agent/{}", kebab_name);
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["worktree", "add", "-b"])
        .arg(&branch_name)
        .arg(&worktree_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| ApiError::internal(err.to_string()))?;

    if !status.success() {
        return Err(ApiError::internal("git worktree add failed"));
    }

    Ok(worktree_path)
}

fn start_tool_session(agent_name: &str, tool: &str, worktree_path: &Path) -> Result<(), ApiError> {
    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s"])
        .arg(agent_name)
        .arg("-c")
        .arg(worktree_path)
        .arg("--")
        .arg("sh")
        .arg("-lc")
        .arg(tool)
        .status()
        .map_err(|err| ApiError::internal(err.to_string()))?;

    if !status.success() {
        return Err(ApiError::internal("tmux session start failed"));
    }

    Ok(())
}

fn to_kebab(value: &str) -> String {
    value.trim().to_lowercase().replace([' ', '_'], "-")
}

fn write_metadata(addr: SocketAddr) -> Result<(), Box<dyn Error>> {
    let config_dir = workforest_core::config_dir();
    std::fs::create_dir_all(&config_dir)?;

    let metadata = ServerMetadata {
        pid: std::process::id(),
        port: addr.port(),
    };

    let metadata_path = config_dir.join("server.json");
    let data = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(metadata_path, data)?;

    Ok(())
}

fn remove_metadata() {
    let metadata_path = workforest_core::config_dir().join("server.json");
    let _ = std::fs::remove_file(metadata_path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;

    fn repo_named(name: &str) -> RepoConfig {
        RepoConfig {
            name: name.to_string(),
            path: PathBuf::from("/tmp"),
            tools: Vec::new(),
            default_tool: String::new(),
        }
    }

    #[test]
    fn repo_name_uses_base_when_unique() {
        let repos = vec![repo_named("other")];
        let name = generate_repo_name_with_suffix("demo", &repos, || "suffix".to_string());
        assert_eq!(name, "demo");
    }

    #[test]
    fn repo_name_uses_suffix_on_collision() {
        let repos = vec![repo_named("demo")];
        let mut suffixes = VecDeque::from(vec!["alpha".to_string()]);
        let name = generate_repo_name_with_suffix("demo", &repos, || suffixes.pop_front().unwrap());
        assert_eq!(name, "demo-alpha");
    }

    #[test]
    fn repo_name_rerolls_on_suffix_collision() {
        let repos = vec![repo_named("demo"), repo_named("demo-alpha")];
        let mut suffixes = VecDeque::from(vec!["alpha".to_string(), "bravo".to_string()]);
        let name = generate_repo_name_with_suffix("demo", &repos, || suffixes.pop_front().unwrap());
        assert_eq!(name, "demo-bravo");
    }

    #[test]
    fn kebab_cases_agent_names() {
        assert_eq!(to_kebab("Wild_Cat"), "wild-cat");
        assert_eq!(to_kebab("Blue Fox"), "blue-fox");
    }
}
