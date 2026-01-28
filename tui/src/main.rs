use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear as TermClear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Terminal,
};
use std::{
    collections::HashMap,
    env,
    error::Error,
    io,
    process::Command,
    time::{Duration, Instant},
};

mod theme;
mod windows;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use theme::{ICON_ACTIVE, ICON_ERROR, ICON_IDLE, THEME};
use windows::{handle_window_key_event, render_window, WindowId};
use workforest_core::RepoConfig;

const AGENT_COLUMNS: usize = 3;

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
struct Agent {
    name: String,
    label: String,
    repo: String,
    tool: String,
    status: String,
    worktree_path: String,
    output: Option<String>,
}

#[derive(Serialize)]
struct AddRepoRequest {
    path: String,
}

#[derive(Deserialize)]
struct AgentOutput {
    name: String,
    status: String,
    output: Option<String>,
}

#[derive(Serialize)]
struct AddAgentRequest {
    repo: String,
    tool: String,
    name: Option<String>,
}

struct DeleteAgentTarget {
    name: String,
    label: String,
}

enum DeleteAgentAction {
    Cancel,
    Delete,
}

enum AgentField {
    Repo,
    Name,
    Tool,
    Create,
}

struct App {
    server_url: String,
    client: Client,
    agents: Vec<Agent>,
    repos: Vec<RepoConfig>,
    windows: Vec<WindowId>,
    focused_window: Option<WindowId>,
    input: String,
    agent_name_input: String,
    agent_filter_input: String,
    selected_repo: usize,
    selected_tool: usize,
    selected_agent: usize,
    agent_scroll: usize,
    agent_field: AgentField,
    status_message: Option<String>,
    animation_start: Instant,
    needs_terminal_reset: bool,
    delete_agent: Option<DeleteAgentTarget>,
    delete_agent_action: DeleteAgentAction,
}

fn main() -> Result<(), Box<dyn Error>> {
    let server_url =
        std::env::var("WORKFOREST_SERVER_URL").unwrap_or_else(|_| "http://127.0.0.1:0".to_string());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(server_url);
    app.refresh_data();
    let mut last_refresh = Instant::now();

    loop {
        if last_refresh.elapsed() >= Duration::from_secs(5) {
            app.refresh_data();
            last_refresh = Instant::now();
        }

        if app.needs_terminal_reset {
            reset_terminal(&mut terminal)?;
            app.needs_terminal_reset = false;
        }

        terminal.draw(|frame| draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(&mut app, key)? {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

impl App {
    fn new(server_url: String) -> Self {
        Self {
            server_url,
            client: Client::new(),
            agents: Vec::new(),
            repos: Vec::new(),
            windows: vec![
                WindowId::Root,
                WindowId::AddRepo,
                WindowId::AddAgent,
                WindowId::ShowRepos,
                WindowId::DeleteAgent,
            ],
            focused_window: None,
            input: String::new(),
            agent_name_input: String::new(),
            agent_filter_input: String::new(),
            selected_repo: 0,
            selected_tool: 0,
            selected_agent: 0,
            agent_scroll: 0,
            agent_field: AgentField::Repo,
            status_message: None,
            animation_start: Instant::now(),
            needs_terminal_reset: false,
            delete_agent: None,
            delete_agent_action: DeleteAgentAction::Cancel,
        }
    }

    fn refresh_data(&mut self) {
        self.repos = fetch_repos(&self.client, &self.server_url).unwrap_or_else(|err| {
            self.status_message = Some(err);
            Vec::new()
        });
        self.agents = fetch_agents(&self.client, &self.server_url).unwrap_or_else(|err| {
            self.status_message = Some(err);
            Vec::new()
        });
        if self.agents.is_empty() {
            self.selected_agent = 0;
        } else if self.selected_agent >= self.agents.len() {
            self.selected_agent = self.agents.len() - 1;
        }
        match fetch_agents_output(&self.client, &self.server_url) {
            Ok(outputs) => {
                for agent in &mut self.agents {
                    if let Some(entry) = outputs.get(&agent.name) {
                        agent.status = entry.status.clone();
                        agent.output = entry.output.clone();
                    } else {
                        agent.status = "sleep".to_string();
                        agent.output = None;
                    }
                }
            }
            Err(err) => {
                self.status_message = Some(err);
                for agent in &mut self.agents {
                    agent.status = "sleep".to_string();
                    agent.output = None;
                }
            }
        }
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }
}

fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(true),
        _ => {}
    }

    if let Some(window) = app.focused_window {
        if app.windows.contains(&window) {
            return handle_window_key_event(window, app, key);
        }
        app.focused_window = None;
    }

    handle_window_key_event(WindowId::Root, app, key)
}

fn filtered_repo_indices(app: &App) -> Vec<usize> {
    let filter = app.agent_filter_input.trim().to_lowercase();
    app.repos
        .iter()
        .enumerate()
        .filter(|(_, repo)| filter.is_empty() || repo.name.to_lowercase().contains(&filter))
        .map(|(index, _)| index)
        .collect()
}

fn filtered_tool_indices(app: &App) -> Vec<usize> {
    let filter = app.agent_filter_input.trim().to_lowercase();
    app.repos
        .get(app.selected_repo)
        .map(|repo| {
            repo.tools
                .iter()
                .enumerate()
                .filter(|(_, tool)| filter.is_empty() || tool.to_lowercase().contains(&filter))
                .map(|(index, _)| index)
                .collect()
        })
        .unwrap_or_default()
}

fn sync_filtered_selection(app: &mut App) {
    match app.agent_field {
        AgentField::Repo => {
            let indices = filtered_repo_indices(app);
            if let Some(first) = indices.first() {
                if !indices.contains(&app.selected_repo) {
                    app.selected_repo = *first;
                    app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                }
            }
        }
        AgentField::Tool => {
            let indices = filtered_tool_indices(app);
            if let Some(first) = indices.first() {
                if !indices.contains(&app.selected_tool) {
                    app.selected_tool = *first;
                }
            }
        }
        _ => {}
    }
}

fn open_agent_tmux(agent: &Agent) -> Result<bool, Box<dyn Error>> {
    ensure_tmux_session(agent)?;
    if env::var("TMUX").is_ok() {
        let status = Command::new("tmux")
            .args(["switch-client", "-t", agent.name.as_str()])
            .status()?;
        if !status.success() {
            return Err("tmux switch-client failed".into());
        }
        return Ok(false);
    }

    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    let result = Command::new("tmux")
        .args(["attach", "-t", agent.name.as_str()])
        .status();
    enable_raw_mode()?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        TermClear(ClearType::All),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    let status = result?;
    if !status.success() {
        return Err("tmux attach failed".into());
    }

    Ok(true)
}

fn ensure_tmux_session(agent: &Agent) -> Result<(), Box<dyn Error>> {
    let status = Command::new("tmux")
        .args(["has-session", "-t", agent.name.as_str()])
        .status()?;
    if status.success() {
        return Ok(());
    }

    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s"])
        .arg(&agent.name)
        .arg("-c")
        .arg(&agent.worktree_path)
        .args(["--", "sh", "-lc"])
        .arg(&agent.tool)
        .status()?;
    if !status.success() {
        return Err("tmux session start failed".into());
    }

    Ok(())
}

fn reset_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        LeaveAlternateScreen,
        TermClear(ClearType::All),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    *terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    Ok(())
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    let background_style = Style::default().bg(THEME.bg);
    let area = frame.area();
    frame.render_widget(Block::default().style(background_style), area);

    let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let padded = padded_rect(sections[0]);

    render_window(WindowId::Root, frame, app, padded);

    let footer = Paragraph::new(
        "(a) add agent   (d) delete agent   (r) add repo   (l) show repos   (u) refresh   Enter open   (q) quit   Esc to close",
    )
    .style(Style::default().fg(THEME.fg_dim))
    .alignment(Alignment::Left);
    let footer_area = sections[1].inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    frame.render_widget(footer, footer_area);

    if let Some(window) = app.focused_window {
        render_window(window, frame, app, padded);
    }
}

fn build_name_line(agent: &Agent, animation_start: Instant) -> Line<'static> {
    match agent.status.as_str() {
        "running" => icon_name_line(
            ICON_ACTIVE,
            pulsing_green_color(animation_start),
            &agent.label,
        ),
        "error" => icon_name_line(ICON_ERROR, THEME.red, &agent.label),
        "idle" => icon_name_line(ICON_IDLE, THEME.blue, &agent.label),
        "sleep" => icon_name_line(ICON_IDLE, THEME.fg_dim, &agent.label),
        _ => icon_name_line(ICON_IDLE, THEME.fg_dim, &agent.label),
    }
}

fn icon_name_line(icon: &str, color: Color, label: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            label.to_string(),
            Style::default().fg(THEME.fg).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(icon.to_string(), Style::default().fg(color)),
    ])
}

fn pulsing_green_color(animation_start: Instant) -> Color {
    let elapsed = animation_start.elapsed().as_secs_f32();
    let pulse = (elapsed * 2.0).sin().abs();
    blend_color(THEME.green_dim, THEME.green, pulse)
}

fn blend_color(from: Color, to: Color, amount: f32) -> Color {
    let clamped = amount.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(from_r, from_g, from_b), Color::Rgb(to_r, to_g, to_b)) => {
            let lerp_channel = |start: u8, end: u8| -> u8 {
                let start = f32::from(start);
                let end = f32::from(end);
                (start + (end - start) * clamped).round() as u8
            };
            Color::Rgb(
                lerp_channel(from_r, to_r),
                lerp_channel(from_g, to_g),
                lerp_channel(from_b, to_b),
            )
        }
        _ => to,
    }
}

fn padded_rect(rect: Rect) -> Rect {
    rect.inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
}

fn centered_rect(percent_x: u16, percent_y: u16, rect: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(rect);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}

fn default_tool_index(repo: &RepoConfig) -> usize {
    repo.tools
        .iter()
        .position(|tool| tool == &repo.default_tool)
        .unwrap_or(0)
}

fn fetch_repos(client: &Client, server_url: &str) -> Result<Vec<RepoConfig>, String> {
    let url = format!("{}/repos", server_url);
    let response = client.get(url).send().map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(response
            .text()
            .unwrap_or_else(|_| "failed to load repos".to_string()));
    }
    response.json().map_err(|err| err.to_string())
}

fn fetch_agents(client: &Client, server_url: &str) -> Result<Vec<Agent>, String> {
    let url = format!("{}/agents", server_url);
    client
        .get(&url)
        .send()
        .map_err(|err| err.to_string())?
        .json()
        .map_err(|err| err.to_string())
}

fn fetch_agents_output(
    client: &Client,
    server_url: &str,
) -> Result<HashMap<String, AgentOutput>, String> {
    let url = format!("{}/agents/output", server_url);
    let outputs = client
        .get(&url)
        .send()
        .map_err(|err| err.to_string())?
        .json::<Vec<AgentOutput>>()
        .map_err(|err| err.to_string())?;

    Ok(outputs
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect())
}

fn add_repo(client: &Client, server_url: &str, path: &str) -> Result<RepoConfig, String> {
    let url = format!("{}/repos", server_url);
    let response = client
        .post(url)
        .json(&AddRepoRequest {
            path: path.to_string(),
        })
        .send()
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(response
            .text()
            .unwrap_or_else(|_| "failed to add repo".to_string()));
    }
    response.json().map_err(|err| err.to_string())
}

fn add_agent(
    client: &Client,
    server_url: &str,
    repo: &str,
    tool: &str,
    name: Option<String>,
) -> Result<Agent, String> {
    let url = format!("{}/agents", server_url);
    let response = client
        .post(url)
        .json(&AddAgentRequest {
            repo: repo.to_string(),
            tool: tool.to_string(),
            name,
        })
        .send()
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(response
            .text()
            .unwrap_or_else(|_| "failed to add agent".to_string()));
    }
    response.json().map_err(|err| err.to_string())
}

fn delete_agent(client: &Client, server_url: &str, name: &str) -> Result<(), String> {
    let url = format!("{}/agents/{}", server_url, name);
    let response = client.delete(url).send().map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(response
            .text()
            .unwrap_or_else(|_| "failed to delete agent".to_string()));
    }
    Ok(())
}
