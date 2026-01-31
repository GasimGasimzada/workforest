use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags, SockaddrStorage};
use num_traits::ToPrimitive;
use petname::petname;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    io::{BufRead, BufReader, IoSlice, Read, Write},
    net::SocketAddr,
    os::fd::FromRawFd,
    os::unix::io::AsRawFd,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};
use termwiz::escape::csi::{
    Cursor, CursorStyle, DecPrivateMode, DecPrivateModeCode, Mode, Sgr, TerminalMode,
    TerminalModeCode,
};
use termwiz::escape::esc::EscCode;
use termwiz::escape::{parser::Parser, Action, Esc};
use tokio::sync::oneshot;
use workforest_core::{
    data_dir, repos_config_path, CursorShape, ModeEntry, RepoConfig, RepoConfigFile, ScrollRegion,
    TerminalAttributes, TerminalBlink, TerminalColor, TerminalIntensity, TerminalSnapshot,
    TerminalUnderline,
};

#[derive(Clone)]
struct AppState {
    shutdown_sender: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
    db: Arc<tokio::sync::Mutex<Connection>>,
    pty_sessions: Arc<Mutex<HashMap<String, PtySession>>>,
}

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    size: PtySize,
    history: Arc<Mutex<VecDeque<u8>>>,
    terminal_snapshot: Arc<Mutex<TerminalSnapshot>>,
    subscribers: Arc<Mutex<Vec<UnixStream>>>,
    _history_handle: thread::JoinHandle<()>,
}

const HISTORY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

struct PtyBroker {
    socket_path: PathBuf,
    _handle: thread::JoinHandle<()>,
}

impl Drop for PtyBroker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
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
    name: Option<String>,
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
    let db = Arc::new(tokio::sync::Mutex::new(init_database()?));
    let pty_sessions = Arc::new(Mutex::new(HashMap::new()));
    let broker = start_pty_broker(pty_sessions.clone(), db.clone())?;
    let state = AppState {
        shutdown_sender: Arc::new(tokio::sync::Mutex::new(Some(shutdown_sender))),
        db: db.clone(),
        pty_sessions,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/shutdown", get(shutdown))
        .route("/repos", get(list_repos).post(add_repo))
        .route("/agents", get(list_agents).post(add_agent))
        .route("/agents/:name", delete(delete_agent))
        .route("/agents/:name/restart", post(restart_agent))
        .route("/agents/output", get(agents_output))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = listener.local_addr()?;

    write_metadata(local_addr)?;

    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver))
        .await?;

    drop(broker);
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
        let status = pty_session_status(&name, &state.pty_sessions);
        outputs.push(AgentOutput {
            name: name.clone(),
            status,
            output: None,
        });
    }

    Ok(Json(outputs))
}

fn pty_session_status(
    agent_name: &str,
    sessions: &Arc<Mutex<HashMap<String, PtySession>>>,
) -> String {
    let sessions = sessions.lock().expect("pty sessions lock");
    if sessions.contains_key(agent_name) {
        "running".to_string()
    } else {
        "sleep".to_string()
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

    let requested_name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let agent_name = if let Some(name) = requested_name {
        let conn = state.db.lock().await;
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE name = ?1)",
                params![name],
                |row| row.get(0),
            )
            .map_err(|err| ApiError::internal(err.to_string()))?;
        if exists {
            return Err(ApiError::bad_request("agent name already exists"));
        }
        name.to_string()
    } else {
        generate_unique_agent_name(state.db.clone()).await?
    };
    let label = agent_name.clone();
    let worktree_path = create_worktree(&repo.path, &repo.name, &agent_name)?;
    start_tool_session(
        &agent_name,
        &request.tool,
        &worktree_path,
        &state.pty_sessions,
    )?;
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

async fn delete_agent(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let (repo_name, worktree_path) = {
        let conn = state.db.lock().await;
        conn.query_row(
            "SELECT repo, worktree_path FROM agents WHERE name = ?1",
            params![name.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => ApiError::not_found("agent not found"),
            _ => ApiError::internal(err.to_string()),
        })?
    };

    let config = load_repo_config()?;
    let repo = config
        .repos
        .iter()
        .find(|repo| repo.name == repo_name)
        .ok_or_else(|| ApiError::not_found("repo not found for agent"))?;

    stop_pty_session(&name, &state.pty_sessions);
    delete_worktree(&repo.path, Path::new(&worktree_path), &name)?;

    let conn = state.db.lock().await;
    conn.execute("DELETE FROM agents WHERE name = ?1", params![name.as_str()])
        .map_err(|err| ApiError::internal(err.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn restart_agent(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let (tool, worktree_path) = {
        let conn = state.db.lock().await;
        conn.query_row(
            "SELECT tool, worktree_path FROM agents WHERE name = ?1",
            params![name.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => ApiError::not_found("agent not found"),
            _ => ApiError::internal(err.to_string()),
        })?
    };

    stop_pty_session(&name, &state.pty_sessions);
    start_tool_session(&name, &tool, Path::new(&worktree_path), &state.pty_sessions)?;

    let now = Utc::now().to_rfc3339();
    let conn = state.db.lock().await;
    conn.execute(
        "UPDATE agents SET status = ?1, updated_at = ?2 WHERE name = ?3",
        params!["running", now, name.as_str()],
    )
    .map_err(|err| ApiError::internal(err.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
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

fn start_pty_broker(
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    db: Arc<tokio::sync::Mutex<Connection>>,
) -> Result<PtyBroker, Box<dyn Error>> {
    let socket_path = data_dir().join("pty.sock");
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let sessions = sessions.clone();
                    let db = db.clone();
                    thread::spawn(move || {
                        if let Err(err) = handle_pty_connection(stream, sessions, db) {
                            eprintln!("pty broker error: {err}");
                        }
                    });
                }
                Err(err) => {
                    eprintln!("pty broker accept error: {err}");
                    break;
                }
            }
        }
    });

    Ok(PtyBroker {
        socket_path,
        _handle: handle,
    })
}

fn handle_pty_connection(
    stream: UnixStream,
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    db: Arc<tokio::sync::Mutex<Connection>>,
) -> Result<(), Box<dyn Error>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let command = parts.next().unwrap_or("");
        match command {
            "ATTACH" => {
                let agent = parts.next().unwrap_or("");
                let response = attach_pty(agent, &stream, &sessions, &db);
                if let Err(err) = response {
                    let _ = write_response(&stream, &format!("ERR {err}\n"));
                }
            }
            "RESIZE" => {
                let agent = parts.next().unwrap_or("");
                let cols = parts.next().and_then(|value| value.parse::<u16>().ok());
                let rows = parts.next().and_then(|value| value.parse::<u16>().ok());
                match (cols, rows) {
                    (Some(cols), Some(rows)) => {
                        let result = resize_pty(agent, cols, rows, &sessions);
                        let _ = if result.is_ok() {
                            write_response(&stream, "OK\n")
                        } else {
                            write_response(&stream, "ERR resize failed\n")
                        };
                    }
                    _ => {
                        let _ = write_response(&stream, "ERR invalid resize\n");
                    }
                }
            }
            "INPUT" => {
                let agent = parts.next().unwrap_or("");
                let len = parts.next().and_then(|value| value.parse::<usize>().ok());
                match len {
                    Some(len) => {
                        let mut payload = vec![0u8; len];
                        if len > 0 {
                            if let Err(err) = reader.read_exact(&mut payload) {
                                let _ = write_response(&stream, &format!("ERR {err}\n"));
                                continue;
                            }
                        }
                        let result = ensure_pty_session(agent, &db, &sessions)
                            .map_err(|err| err.to_string())
                            .and_then(|_| write_pty_input(agent, &payload, &sessions));
                        let _ = if result.is_ok() {
                            write_response(&stream, "OK\n")
                        } else {
                            write_response(&stream, "ERR input failed\n")
                        };
                    }
                    _ => {
                        let _ = write_response(&stream, "ERR invalid input\n");
                    }
                }
            }
            _ => {
                let _ = write_response(&stream, "ERR unknown command\n");
            }
        }
    }

    Ok(())
}

fn ensure_pty_session(
    agent: &str,
    db: &Arc<tokio::sync::Mutex<Connection>>,
    sessions: &Arc<Mutex<HashMap<String, PtySession>>>,
) -> Result<(), String> {
    {
        let sessions = sessions.lock().expect("pty sessions lock");
        if sessions.contains_key(agent) {
            return Ok(());
        }
    }

    let (tool, worktree_path) = {
        let conn = db.blocking_lock();
        conn.query_row(
            "SELECT tool, worktree_path FROM agents WHERE name = ?1",
            params![agent],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|err| err.to_string())?
    };

    start_tool_session(agent, &tool, Path::new(&worktree_path), sessions).map_err(|err| err.message)
}

fn attach_pty(
    agent: &str,
    stream: &UnixStream,
    sessions: &Arc<Mutex<HashMap<String, PtySession>>>,
    db: &Arc<tokio::sync::Mutex<Connection>>,
) -> Result<(), Box<dyn Error>> {
    if agent.trim().is_empty() {
        return Err("agent name required".into());
    }

    ensure_pty_session(agent, db, sessions)?;

    let (history, snapshot, client_stream) = {
        let mut sessions = sessions.lock().expect("pty sessions lock");
        let session = sessions.get_mut(agent).ok_or("agent not found")?;
        let history = session.history.lock().expect("pty history lock");
        let bytes: Vec<u8> = history.iter().copied().collect();
        let snapshot = session
            .terminal_snapshot
            .lock()
            .expect("pty terminal snapshot lock")
            .clone();
        let (server_stream, client_stream) = UnixStream::pair()?;
        session
            .subscribers
            .lock()
            .expect("pty subscribers lock")
            .push(server_stream);
        (bytes, snapshot, client_stream)
    };

    let snapshot_json = serde_json::to_string(&snapshot)?;
    write_response(stream, &format!("MODES {}\n", snapshot_json))?;
    write_response(stream, &format!("HISTORY {}\n", history.len()))?;
    if !history.is_empty() {
        let mut stream = stream.try_clone()?;
        stream.write_all(&history)?;
    }

    let client_fd = client_stream.as_raw_fd();
    sendmsg(
        stream.as_raw_fd(),
        &[IoSlice::new(b"OK\n")],
        &[ControlMessage::ScmRights(&[client_fd])],
        MsgFlags::empty(),
        None::<&SockaddrStorage>,
    )?;
    Ok(())
}

fn resize_pty(
    agent: &str,
    cols: u16,
    rows: u16,
    sessions: &Arc<Mutex<HashMap<String, PtySession>>>,
) -> Result<(), Box<dyn Error>> {
    let mut sessions = sessions.lock().expect("pty sessions lock");
    let session = sessions
        .get_mut(agent)
        .ok_or_else(|| "agent not found".to_string())?;
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    session.master.resize(size)?;
    session.size = size;
    Ok(())
}

fn write_pty_input(
    agent: &str,
    payload: &[u8],
    sessions: &Arc<Mutex<HashMap<String, PtySession>>>,
) -> Result<(), String> {
    let mut sessions = sessions.lock().expect("pty sessions lock");
    let session = sessions
        .get_mut(agent)
        .ok_or_else(|| "agent not found".to_string())?;
    let mut writer = session.writer.lock().expect("pty writer lock");
    writer.write_all(payload).map_err(|err| err.to_string())?;
    writer.flush().map_err(|err| err.to_string())?;
    Ok(())
}

fn write_response(stream: &UnixStream, response: &str) -> Result<(), Box<dyn Error>> {
    let mut stream = stream.try_clone()?;
    stream.write_all(response.as_bytes())?;
    Ok(())
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

fn start_tool_session(
    agent_name: &str,
    tool: &str,
    worktree_path: &Path,
    sessions: &Arc<Mutex<HashMap<String, PtySession>>>,
) -> Result<(), ApiError> {
    let mut sessions = sessions.lock().expect("pty sessions lock");
    if sessions.contains_key(agent_name) {
        return Ok(());
    }

    let pty_system = native_pty_system();
    let size = PtySize::default();
    let pair = pty_system
        .openpty(size)
        .map_err(|err| ApiError::internal(err.to_string()))?;
    let mut cmd = CommandBuilder::new("sh");
    cmd.arg("-lc");
    cmd.arg(tool);
    cmd.cwd(worktree_path);
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let history = Arc::new(Mutex::new(VecDeque::new()));
    let terminal_snapshot = Arc::new(Mutex::new(default_terminal_snapshot()));
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    let master_fd = pair
        .master
        .as_raw_fd()
        .ok_or_else(|| ApiError::internal("missing master fd"))?;
    let history_handle = spawn_history_reader(
        master_fd,
        history.clone(),
        terminal_snapshot.clone(),
        subscribers.clone(),
    );
    let writer = pair
        .master
        .take_writer()
        .map_err(|err| ApiError::internal(err.to_string()))?;
    sessions.insert(
        agent_name.to_string(),
        PtySession {
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            child,
            size,
            history,
            terminal_snapshot,
            subscribers,
            _history_handle: history_handle,
        },
    );

    Ok(())
}

fn stop_pty_session(agent_name: &str, sessions: &Arc<Mutex<HashMap<String, PtySession>>>) {
    let mut sessions = sessions.lock().expect("pty sessions lock");
    if let Some(mut session) = sessions.remove(agent_name) {
        let _ = session.child.kill();
    }
}

fn spawn_history_reader(
    fd: i32,
    history: Arc<Mutex<VecDeque<u8>>>,
    terminal_snapshot: Arc<Mutex<TerminalSnapshot>>,
    subscribers: Arc<Mutex<Vec<UnixStream>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut buffer = [0u8; 4096];
        let mut parser = Parser::new();
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    {
                        let mut history = history.lock().expect("pty history lock");
                        for byte in &buffer[..size] {
                            history.push_back(*byte);
                        }
                        trim_history_to_boundary(&mut history, HISTORY_LIMIT_BYTES);
                    }
                    {
                        let mut snapshot = terminal_snapshot
                            .lock()
                            .expect("pty terminal snapshot lock");
                        parser.parse(&buffer[..size], |action| {
                            apply_action_to_snapshot(action, &mut snapshot);
                        });
                    }
                    let mut subs = subscribers.lock().expect("pty subscribers lock");
                    subs.retain_mut(|stream| stream.write_all(&buffer[..size]).is_ok());
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

fn default_terminal_snapshot() -> TerminalSnapshot {
    TerminalSnapshot {
        cursor_visible: true,
        wrap_mode: true,
        ..TerminalSnapshot::default()
    }
}

fn trim_history_to_boundary(history: &mut VecDeque<u8>, limit: usize) {
    if history.len() <= limit {
        return;
    }
    let overflow = history.len() - limit;
    let bytes: Vec<u8> = history.iter().copied().collect();
    let drop_count = find_safe_history_start(&bytes, overflow);
    for _ in 0..drop_count {
        history.pop_front();
    }
}

fn find_safe_history_start(bytes: &[u8], overflow: usize) -> usize {
    let mut index = 0;
    let mut last_safe = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            if let Some(next) = bytes.get(index + 1).copied() {
                match next {
                    b'[' => {
                        index = parse_csi_sequence(bytes, index + 2);
                    }
                    b']' | b'P' | b'^' | b'_' => {
                        index = parse_string_sequence(bytes, index + 2);
                    }
                    _ => {
                        index = (index + 2).min(bytes.len());
                    }
                }
            } else {
                break;
            }
        } else {
            index += 1;
        }
        if index <= overflow {
            last_safe = index;
        }
    }
    last_safe
}

fn parse_csi_sequence(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if (0x40..=0x7e).contains(&byte) {
            break;
        }
    }
    index
}

fn parse_string_sequence(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == 0x07 {
            return index + 1;
        }
        if byte == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
            return index + 2;
        }
        index += 1;
    }
    index
}

fn apply_action_to_snapshot(action: Action, snapshot: &mut TerminalSnapshot) {
    match action {
        Action::CSI(csi) => apply_csi_to_snapshot(csi, snapshot),
        Action::Esc(esc) => apply_esc_to_snapshot(esc, snapshot),
        _ => {}
    }
}

fn apply_esc_to_snapshot(esc: Esc, snapshot: &mut TerminalSnapshot) {
    if let Esc::Code(code) = esc {
        if matches!(code, EscCode::FullReset) {
            *snapshot = default_terminal_snapshot();
        }
    }
}

fn apply_csi_to_snapshot(csi: termwiz::escape::csi::CSI, snapshot: &mut TerminalSnapshot) {
    match csi {
        termwiz::escape::csi::CSI::Mode(mode) => apply_mode_to_snapshot(mode, snapshot),
        termwiz::escape::csi::CSI::Sgr(sgr) => apply_sgr_to_snapshot(sgr, snapshot),
        termwiz::escape::csi::CSI::Cursor(cursor) => apply_cursor_to_snapshot(cursor, snapshot),
        _ => {}
    }
}

fn apply_cursor_to_snapshot(cursor: Cursor, snapshot: &mut TerminalSnapshot) {
    match cursor {
        Cursor::SetTopAndBottomMargins { top, bottom } => {
            let top = top.as_zero_based() as usize;
            let bottom = bottom.as_zero_based() as usize;
            if top < bottom {
                snapshot.scroll_region = Some(ScrollRegion { top, bottom });
            } else {
                snapshot.scroll_region = None;
            }
        }
        Cursor::CursorStyle(style) => {
            snapshot.cursor_shape = match style {
                CursorStyle::Default => CursorShape::Default,
                CursorStyle::BlinkingBlock => CursorShape::BlinkingBlock,
                CursorStyle::SteadyBlock => CursorShape::SteadyBlock,
                CursorStyle::BlinkingUnderline => CursorShape::BlinkingUnderline,
                CursorStyle::SteadyUnderline => CursorShape::SteadyUnderline,
                CursorStyle::BlinkingBar => CursorShape::BlinkingBar,
                CursorStyle::SteadyBar => CursorShape::SteadyBar,
            };
        }
        _ => {}
    }
}

fn apply_mode_to_snapshot(mode: Mode, snapshot: &mut TerminalSnapshot) {
    match mode {
        Mode::SetDecPrivateMode(mode) => apply_dec_private_mode(mode, snapshot, true),
        Mode::ResetDecPrivateMode(mode) => apply_dec_private_mode(mode, snapshot, false),
        Mode::SetMode(mode) => apply_terminal_mode(mode, snapshot, true),
        Mode::ResetMode(mode) => apply_terminal_mode(mode, snapshot, false),
        _ => {}
    }
}

fn apply_dec_private_mode(mode: DecPrivateMode, snapshot: &mut TerminalSnapshot, enabled: bool) {
    let code = match mode {
        DecPrivateMode::Code(code) => code,
        DecPrivateMode::Unspecified(code) => {
            set_mode_entry(&mut snapshot.dec_private_modes, code, enabled);
            return;
        }
    };
    if let Some(value) = code.to_u16() {
        set_mode_entry(&mut snapshot.dec_private_modes, value, enabled);
    }
    match code {
        DecPrivateModeCode::ShowCursor => snapshot.cursor_visible = enabled,
        DecPrivateModeCode::StartBlinkingCursor => {
            if enabled {
                snapshot.cursor_shape = CursorShape::BlinkingBlock;
            }
        }
        DecPrivateModeCode::OriginMode => snapshot.origin_mode = enabled,
        DecPrivateModeCode::AutoWrap => snapshot.wrap_mode = enabled,
        DecPrivateModeCode::ClearAndEnableAlternateScreen
        | DecPrivateModeCode::EnableAlternateScreen
        | DecPrivateModeCode::OptEnableAlternateScreen => snapshot.alt_screen = enabled,
        DecPrivateModeCode::MouseTracking => snapshot.mouse_tracking = enabled,
        DecPrivateModeCode::ButtonEventMouse => snapshot.mouse_button_tracking = enabled,
        DecPrivateModeCode::AnyEventMouse => snapshot.mouse_any_event = enabled,
        DecPrivateModeCode::SGRMouse => snapshot.mouse_sgr = enabled,
        _ => {}
    }
}

fn apply_terminal_mode(mode: TerminalMode, snapshot: &mut TerminalSnapshot, enabled: bool) {
    let code = match mode {
        TerminalMode::Code(code) => code,
        TerminalMode::Unspecified(code) => {
            set_mode_entry(&mut snapshot.terminal_modes, code, enabled);
            return;
        }
    };
    if let Some(value) = code.to_u16() {
        set_mode_entry(&mut snapshot.terminal_modes, value, enabled);
    }
    match code {
        TerminalModeCode::Insert => snapshot.insert_mode = enabled,
        TerminalModeCode::ShowCursor => snapshot.cursor_visible = enabled,
        _ => {}
    }
}

fn apply_sgr_to_snapshot(sgr: Sgr, snapshot: &mut TerminalSnapshot) {
    match sgr {
        Sgr::Reset => snapshot.attributes = TerminalAttributes::default(),
        Sgr::Intensity(value) => {
            snapshot.attributes.intensity = match value {
                termwiz::cell::Intensity::Normal => TerminalIntensity::Normal,
                termwiz::cell::Intensity::Bold => TerminalIntensity::Bold,
                termwiz::cell::Intensity::Half => TerminalIntensity::Faint,
            };
        }
        Sgr::Underline(value) => {
            snapshot.attributes.underline = match value {
                termwiz::cell::Underline::None => TerminalUnderline::None,
                termwiz::cell::Underline::Single => TerminalUnderline::Single,
                termwiz::cell::Underline::Double => TerminalUnderline::Double,
                _ => TerminalUnderline::Single,
            };
        }
        Sgr::Blink(value) => {
            snapshot.attributes.blink = match value {
                termwiz::cell::Blink::None => TerminalBlink::None,
                termwiz::cell::Blink::Slow => TerminalBlink::Slow,
                termwiz::cell::Blink::Rapid => TerminalBlink::Rapid,
            };
        }
        Sgr::Italic(value) => snapshot.attributes.italic = value,
        Sgr::Inverse(value) => snapshot.attributes.inverse = value,
        Sgr::Invisible(value) => snapshot.attributes.hidden = value,
        Sgr::StrikeThrough(value) => snapshot.attributes.strikethrough = value,
        Sgr::Foreground(color) => {
            snapshot.attributes.foreground = color_to_snapshot(color.into());
        }
        Sgr::Background(color) => {
            snapshot.attributes.background = color_to_snapshot(color.into());
        }
        _ => {}
    }
}

fn color_to_snapshot(color: termwiz::color::ColorAttribute) -> TerminalColor {
    match color {
        termwiz::color::ColorAttribute::Default => TerminalColor::Default,
        termwiz::color::ColorAttribute::TrueColorWithDefaultFallback(color)
        | termwiz::color::ColorAttribute::TrueColorWithPaletteFallback(color, _) => {
            let (r, g, b, _) = color.as_rgba_u8();
            TerminalColor::Rgb { r, g, b }
        }
        termwiz::color::ColorAttribute::PaletteIndex(index) => TerminalColor::Ansi(index),
    }
}

fn set_mode_entry(entries: &mut Vec<ModeEntry>, code: u16, enabled: bool) {
    if let Some(entry) = entries.iter_mut().find(|entry| entry.code == code) {
        entry.enabled = enabled;
    } else {
        entries.push(ModeEntry { code, enabled });
    }
}

fn delete_worktree(
    repo_path: &Path,
    worktree_path: &Path,
    agent_name: &str,
) -> Result<(), ApiError> {
    if worktree_path.exists() {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["worktree", "remove", "-f"])
            .arg(worktree_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|err| ApiError::internal(err.to_string()))?;

        if !status.success() {
            return Err(ApiError::internal("git worktree remove failed"));
        }
    }

    let branch_name = format!("agent/{}", to_kebab(agent_name));
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["branch", "-D", &branch_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if worktree_path.exists() {
        std::fs::remove_dir_all(worktree_path)
            .map_err(|err| ApiError::internal(err.to_string()))?;
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

    #[test]
    fn history_trim_avoids_mid_sequence_cut() {
        let history = b"hello\x1b[31mworld";
        let overflow = 7;
        let start = find_safe_history_start(history, overflow);
        assert_eq!(start, 5);
    }

    #[test]
    fn history_trim_allows_plain_cut() {
        let history = b"hello world";
        let overflow = 3;
        let start = find_safe_history_start(history, overflow);
        assert_eq!(start, 3);
    }
}
