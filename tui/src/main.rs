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
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        Wrap,
    },
    Terminal,
};
use std::time::Instant;

const ICON_IDLE: &str = "󰒲";
const ICON_ERROR: &str = "󰅚";
const ICON_ACTIVE: &str = "●";
use petname::petname;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, error::Error, io, process::Command, time::Duration};
use workforest_core::RepoConfig;

const THEME: Theme = Theme {
    bg: Color::Rgb(12, 12, 14),
    bg_alt: Color::Rgb(17, 17, 20),
    bg_alt2: Color::Rgb(22, 22, 27),
    fg: Color::Rgb(255, 255, 255),
    fg_mid: Color::Rgb(184, 184, 184),
    fg_dim: Color::Rgb(107, 107, 107),
    green: Color::Rgb(95, 255, 135),
    green_dim: Color::Rgb(63, 166, 106),
    orange: Color::Rgb(255, 175, 95),
    orange_dim: Color::Rgb(201, 138, 68),
    yellow: Color::Rgb(255, 215, 95),
    yellow_dim: Color::Rgb(230, 193, 90),
    blue: Color::Rgb(95, 175, 255),
    magenta: Color::Rgb(215, 135, 255),
    red: Color::Rgb(255, 95, 95),
    border: Color::Rgb(26, 26, 31),
    visual: Color::Rgb(42, 42, 42),
};

const AGENT_COLUMNS: usize = 3;

struct Theme {
    bg: Color,
    bg_alt: Color,
    bg_alt2: Color,
    fg: Color,
    fg_mid: Color,
    fg_dim: Color,
    green: Color,
    green_dim: Color,
    orange: Color,
    orange_dim: Color,
    yellow: Color,
    yellow_dim: Color,
    blue: Color,
    magenta: Color,
    red: Color,
    border: Color,
    visual: Color,
}

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

enum Modal {
    None,
    AddRepo,
    AddAgent,
    ShowRepos,
    DeleteAgent,
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
    modal: Modal,
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
            modal: Modal::None,
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

    let mut should_exit = false;
    match app.modal {
        Modal::None => {
            should_exit = handle_root_keys(app, key)?;
        }
        Modal::AddRepo => handle_add_repo_keys(app, key)?,
        Modal::AddAgent => handle_add_agent_keys(app, key)?,
        Modal::ShowRepos => handle_show_repos_keys(app, key),
        Modal::DeleteAgent => handle_delete_agent_keys(app, key)?,
    }

    Ok(should_exit)
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

fn handle_root_keys(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('r') => {
            app.modal = Modal::AddRepo;
            app.input.clear();
            app.status_message = None;
        }
        KeyCode::Char('a') => {
            if app.repos.is_empty() {
                app.set_status("add a repo first");
            } else {
                app.modal = Modal::AddAgent;
                app.selected_repo = app.selected_repo.min(app.repos.len() - 1);
                app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                app.agent_field = AgentField::Repo;
                app.agent_filter_input.clear();
                app.agent_name_input = petname(2, "-");
                sync_filtered_selection(app);
                app.status_message = None;
            }
        }
        KeyCode::Char('l') => {
            app.modal = Modal::ShowRepos;
        }
        KeyCode::Char('u') => {
            app.refresh_data();
        }
        KeyCode::Char('d') => {
            if app.agents.is_empty() {
                app.set_status("no agents to delete");
            } else if let Some(agent) = app.agents.get(app.selected_agent) {
                app.delete_agent = Some(DeleteAgentTarget {
                    name: agent.name.clone(),
                    label: agent.label.clone(),
                });
                app.delete_agent_action = DeleteAgentAction::Cancel;
                app.modal = Modal::DeleteAgent;
            }
        }
        KeyCode::Enter => {
            if app.agents.is_empty() {
                app.set_status("no agents to open");
            } else {
                let agent = &app.agents[app.selected_agent];
                match open_agent_tmux(agent) {
                    Ok(needs_reset) => {
                        if needs_reset {
                            app.needs_terminal_reset = true;
                        }
                    }
                    Err(err) => app.set_status(err.to_string()),
                }
            }
        }
        KeyCode::Left => {
            if app.selected_agent > 0 {
                app.selected_agent -= 1;
            }
        }
        KeyCode::Right => {
            if app.selected_agent + 1 < app.agents.len() {
                app.selected_agent += 1;
            }
        }
        KeyCode::Up => {
            if app.selected_agent >= AGENT_COLUMNS {
                app.selected_agent -= AGENT_COLUMNS;
            }
        }
        KeyCode::Down => {
            let col = app.selected_agent % AGENT_COLUMNS;
            let next_row_start = app.selected_agent - col + AGENT_COLUMNS;
            if next_row_start < app.agents.len() {
                let target = next_row_start + col;
                app.selected_agent = target.min(app.agents.len() - 1);
            }
        }
        _ => {}
    }

    Ok(false)
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

fn handle_add_repo_keys(app: &mut App, key: KeyEvent) -> Result<(), Box<dyn Error>> {
    match key.code {
        KeyCode::Esc => {
            app.modal = Modal::None;
        }
        KeyCode::Enter => {
            let path = app.input.trim();
            if path.is_empty() {
                app.set_status("repo path is required");
                return Ok(());
            }

            match add_repo(&app.client, &app.server_url, path) {
                Ok(_) => {
                    app.refresh_data();
                    app.modal = Modal::None;
                    app.input.clear();
                }
                Err(err) => app.set_status(err),
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(value) => {
            app.input.push(value);
        }
        _ => {}
    }
    Ok(())
}

fn handle_add_agent_keys(app: &mut App, key: KeyEvent) -> Result<(), Box<dyn Error>> {
    match key.code {
        KeyCode::Esc => {
            app.modal = Modal::None;
            app.agent_filter_input.clear();
        }
        KeyCode::Tab => {
            let next_field = match app.agent_field {
                AgentField::Repo => AgentField::Name,
                AgentField::Name => AgentField::Tool,
                AgentField::Tool => AgentField::Create,
                AgentField::Create => AgentField::Repo,
            };
            app.agent_field = next_field;
            app.agent_filter_input.clear();
            if matches!(app.agent_field, AgentField::Repo | AgentField::Tool) {
                sync_filtered_selection(app);
            }
        }
        KeyCode::BackTab => {
            let next_field = match app.agent_field {
                AgentField::Repo => AgentField::Create,
                AgentField::Name => AgentField::Repo,
                AgentField::Tool => AgentField::Name,
                AgentField::Create => AgentField::Tool,
            };
            app.agent_field = next_field;
            app.agent_filter_input.clear();
            if matches!(app.agent_field, AgentField::Repo | AgentField::Tool) {
                sync_filtered_selection(app);
            }
        }
        KeyCode::Up => match app.agent_field {
            AgentField::Repo => {
                let indices = filtered_repo_indices(app);
                if let Some(current) = indices.iter().position(|index| *index == app.selected_repo)
                {
                    if current > 0 {
                        app.selected_repo = indices[current - 1];
                        app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                    }
                } else if let Some(first) = indices.first() {
                    app.selected_repo = *first;
                    app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                }
            }
            AgentField::Tool => {
                let indices = filtered_tool_indices(app);
                if let Some(current) = indices.iter().position(|index| *index == app.selected_tool)
                {
                    if current > 0 {
                        app.selected_tool = indices[current - 1];
                    }
                } else if let Some(first) = indices.first() {
                    app.selected_tool = *first;
                }
            }
            _ => {}
        },
        KeyCode::Down => match app.agent_field {
            AgentField::Repo => {
                let indices = filtered_repo_indices(app);
                if let Some(current) = indices.iter().position(|index| *index == app.selected_repo)
                {
                    if current + 1 < indices.len() {
                        app.selected_repo = indices[current + 1];
                        app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                    }
                } else if let Some(first) = indices.first() {
                    app.selected_repo = *first;
                    app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                }
            }
            AgentField::Tool => {
                let indices = filtered_tool_indices(app);
                if let Some(current) = indices.iter().position(|index| *index == app.selected_tool)
                {
                    if current + 1 < indices.len() {
                        app.selected_tool = indices[current + 1];
                    }
                } else if let Some(first) = indices.first() {
                    app.selected_tool = *first;
                }
            }
            _ => {}
        },
        KeyCode::Enter => match app.agent_field {
            AgentField::Repo => {}
            AgentField::Tool => {}
            AgentField::Name => {}
            AgentField::Create => {
                let repo = &app.repos[app.selected_repo];
                let tool = repo
                    .tools
                    .get(app.selected_tool)
                    .cloned()
                    .unwrap_or_else(|| repo.default_tool.clone());
                let name = app.agent_name_input.trim();
                let name = if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                };

                match add_agent(&app.client, &app.server_url, &repo.name, &tool, name) {
                    Ok(agent) => {
                        app.refresh_data();
                        if let Some(index) =
                            app.agents.iter().position(|entry| entry.name == agent.name)
                        {
                            app.selected_agent = index;
                        }
                        app.modal = Modal::None;
                        match open_agent_tmux(&agent) {
                            Ok(needs_reset) => {
                                if needs_reset {
                                    app.needs_terminal_reset = true;
                                }
                            }
                            Err(err) => app.set_status(err.to_string()),
                        }
                    }
                    Err(err) => app.set_status(err),
                }
            }
        },
        KeyCode::Backspace => {
            if matches!(app.agent_field, AgentField::Repo | AgentField::Tool) {
                app.agent_filter_input.pop();
                sync_filtered_selection(app);
            } else if matches!(app.agent_field, AgentField::Name) {
                app.agent_name_input.pop();
            }
        }
        KeyCode::Char(value) => {
            if matches!(app.agent_field, AgentField::Repo | AgentField::Tool) {
                app.agent_filter_input.push(value);
                sync_filtered_selection(app);
            } else if matches!(app.agent_field, AgentField::Name) {
                app.agent_name_input.push(value);
            }
        }
        _ => {}
    }

    Ok(())
}

fn handle_show_repos_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => app.modal = Modal::None,
        _ => {}
    }
}

fn handle_delete_agent_keys(app: &mut App, key: KeyEvent) -> Result<(), Box<dyn Error>> {
    match key.code {
        KeyCode::Esc => {
            app.modal = Modal::None;
            app.delete_agent = None;
        }
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
            app.delete_agent_action = match app.delete_agent_action {
                DeleteAgentAction::Cancel => DeleteAgentAction::Delete,
                DeleteAgentAction::Delete => DeleteAgentAction::Cancel,
            };
        }
        KeyCode::Enter => match app.delete_agent_action {
            DeleteAgentAction::Cancel => {
                app.modal = Modal::None;
                app.delete_agent = None;
            }
            DeleteAgentAction::Delete => {
                if let Some(target) = app.delete_agent.take() {
                    match delete_agent(&app.client, &app.server_url, &target.name) {
                        Ok(()) => {
                            app.refresh_data();
                            app.set_status(format!("deleted agent {}", target.label));
                        }
                        Err(err) => app.set_status(err),
                    }
                }
                app.modal = Modal::None;
            }
        },
        _ => {}
    }

    Ok(())
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    let background_style = Style::default().bg(THEME.bg);
    let area = frame.area();
    frame.render_widget(Block::default().style(background_style), area);

    let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let padded = padded_rect(sections[0]);

    render_agents(frame, padded, app);

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

    match app.modal {
        Modal::None => {}
        Modal::AddRepo => render_add_repo_modal(frame, app, padded),
        Modal::AddAgent => render_add_agent_modal(frame, app, padded),
        Modal::ShowRepos => render_show_repos_modal(frame, app, padded),
        Modal::DeleteAgent => render_delete_agent_modal(frame, app, padded),
    }
}

fn render_agents(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    if app.agents.is_empty() {
        let empty = Paragraph::new("No agents yet. Press (a) to add one.")
            .style(Style::default().fg(THEME.fg_mid))
            .alignment(Alignment::Center);
        frame.render_widget(empty, area);
        return;
    }

    let columns = AGENT_COLUMNS;
    let total_rows = (app.agents.len() + columns - 1) / columns;
    let row_height = 9usize;
    let row_gap = 1usize;
    let visible_rows = ((area.height as usize + row_gap) / (row_height + row_gap)).max(1);
    let scrollbar_needed = total_rows > visible_rows;
    let grid_area = if scrollbar_needed && area.width > 1 {
        Rect {
            width: area.width - 1,
            ..area
        }
    } else {
        area
    };

    if total_rows <= visible_rows {
        app.agent_scroll = 0;
    } else {
        let max_scroll = total_rows - visible_rows;
        let selected_row = app.selected_agent / columns;
        if selected_row < app.agent_scroll {
            app.agent_scroll = selected_row;
        } else if selected_row >= app.agent_scroll + visible_rows {
            app.agent_scroll = selected_row + 1 - visible_rows;
        }
        app.agent_scroll = app.agent_scroll.min(max_scroll);
    }

    let max_scroll = total_rows.saturating_sub(visible_rows);
    let scrollbar_position = if max_scroll > 0 {
        app.agent_scroll * (total_rows.saturating_sub(1)) / max_scroll
    } else {
        0
    };

    let start_row = app.agent_scroll;
    let end_row = (start_row + visible_rows).min(total_rows);
    let mut row_constraints = Vec::new();
    for row in start_row..end_row {
        row_constraints.push(Constraint::Length(row_height as u16));
        if row + 1 < end_row {
            row_constraints.push(Constraint::Length(row_gap as u16));
        }
    }
    let row_areas = Layout::vertical(row_constraints).split(grid_area);

    for (visible_index, row_index) in (start_row..end_row).enumerate() {
        let row_area = row_areas[visible_index * 2];
        let col_areas = Layout::horizontal([
            Constraint::Percentage(33),
            Constraint::Length(2),
            Constraint::Percentage(33),
            Constraint::Length(2),
            Constraint::Percentage(34),
        ])
        .split(row_area);
        let card_areas = [col_areas[0], col_areas[2], col_areas[4]];

        for col_index in 0..columns {
            let agent_index = row_index * columns + col_index;
            if let Some(agent) = app.agents.get(agent_index) {
                let card_style = Style::default().bg(THEME.bg_alt).fg(THEME.fg);
                let border_color = if agent_index == app.selected_agent {
                    THEME.magenta
                } else {
                    THEME.green
                };
                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(border_color))
                    .style(card_style)
                    .padding(Padding {
                        left: 1,
                        right: 1,
                        top: 1,
                        bottom: 1,
                    });
                frame.render_widget(&block, card_areas[col_index]);

                let inner_area = block.inner(card_areas[col_index]);
                let name_line = build_name_line(agent, app.animation_start);
                let repo_line =
                    Line::from(Span::styled(&agent.repo, Style::default().fg(THEME.fg_mid)));
                let mut lines = vec![name_line, repo_line];
                if let Some(output) = agent.output.as_deref() {
                    for line in output.lines() {
                        lines.push(Line::from(Span::styled(
                            line,
                            Style::default().fg(THEME.fg_dim),
                        )));
                    }
                }
                let paragraph = Paragraph::new(lines)
                    .style(card_style)
                    .alignment(Alignment::Left);
                frame.render_widget(paragraph, inner_area);
            }
        }
    }

    if scrollbar_needed {
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        let mut scrollbar_state = ScrollbarState::new(total_rows)
            .position(scrollbar_position)
            .viewport_content_length(visible_rows);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(THEME.fg_dim));
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn render_add_repo_modal(frame: &mut ratatui::Frame, app: &App, base: Rect) {
    let area = centered_rect(70, 30, base);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title("Add repo")
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.border));
    frame.render_widget(&block, area);

    let inner = block.inner(area);
    let text = format!("Path:\n{}\n\nEnter to save, Esc to cancel.", app.input);
    let paragraph = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(THEME.fg_mid));
    frame.render_widget(paragraph, inner);
}

fn render_add_agent_modal(frame: &mut ratatui::Frame, app: &App, base: Rect) {
    let area = centered_rect(70, 60, base);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title("Add agent")
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.border));
    frame.render_widget(&block, area);
    let inner = block.inner(area);

    let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(inner);

    let repo_filtered = filtered_repo_indices(app);
    let tool_filtered = filtered_tool_indices(app);
    let repo_list_lines = 1 + repo_filtered.len().max(1);
    let tool_list_lines = 1 + tool_filtered.len().max(1);
    let repo_box_height = (repo_list_lines + 2) as u16;
    let tool_box_height = (tool_list_lines + 2) as u16;

    let mut constraints = Vec::new();
    constraints.push(Constraint::Length(repo_box_height));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(3));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(tool_box_height));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(3));
    constraints.push(Constraint::Min(0));

    let content_layout = Layout::vertical(constraints).split(sections[0]);
    let mut index = 0;
    let repo_rect = content_layout[index];
    index += 1;
    index += 1;
    let name_rect = content_layout[index];
    index += 1;
    index += 1;
    let tool_rect = content_layout[index];
    index += 1;
    index += 1;
    let create_rect = content_layout[index];

    let repo_name = app
        .repos
        .get(app.selected_repo)
        .map(|repo| repo.name.as_str())
        .unwrap_or("No repos");
    let repo_selected = matches!(app.agent_field, AgentField::Repo);
    let repo_display = if repo_selected && !app.agent_filter_input.is_empty() {
        app.agent_filter_input.as_str()
    } else {
        repo_name
    };
    let repo_border = if repo_selected {
        THEME.fg
    } else {
        THEME.fg_mid
    };
    let repo_title = if repo_selected {
        THEME.fg
    } else {
        THEME.fg_mid
    };
    let repo_text = if repo_selected {
        THEME.fg
    } else {
        THEME.fg_dim
    };
    let repo_block = Block::bordered()
        .title(Span::styled("Repository", Style::default().fg(repo_title)))
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(repo_border));
    let mut repo_lines = Vec::new();
    repo_lines.push(Line::from(Span::styled(
        repo_display.to_string(),
        Style::default().fg(repo_text),
    )));
    if repo_filtered.is_empty() {
        repo_lines.push(Line::from(Span::styled(
            "No matches",
            Style::default().fg(THEME.fg_dim),
        )));
    } else {
        for index in &repo_filtered {
            let repo = &app.repos[*index];
            let marker = if *index == app.selected_repo {
                ">"
            } else {
                " "
            };
            let style = if *index == app.selected_repo {
                Style::default().fg(THEME.fg)
            } else {
                Style::default().fg(THEME.fg_dim)
            };
            repo_lines.push(Line::from(Span::styled(
                format!("{} {}", marker, repo.name),
                style,
            )));
        }
    }
    let repo_content = Paragraph::new(repo_lines).block(repo_block);
    frame.render_widget(repo_content, repo_rect);

    let name_selected = matches!(app.agent_field, AgentField::Name);
    let name_border = if name_selected {
        THEME.fg
    } else {
        THEME.fg_mid
    };
    let name_title = if name_selected {
        THEME.fg
    } else {
        THEME.fg_mid
    };
    let name_text = if name_selected {
        THEME.fg
    } else {
        THEME.fg_dim
    };
    let name_block = Block::bordered()
        .title(Span::styled("Agent name", Style::default().fg(name_title)))
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(name_border));
    let name_content = Paragraph::new(app.agent_name_input.as_str())
        .style(Style::default().fg(name_text))
        .block(name_block);
    frame.render_widget(name_content, name_rect);

    let tool_name = app
        .repos
        .get(app.selected_repo)
        .and_then(|repo| {
            repo.tools
                .get(app.selected_tool)
                .or_else(|| repo.tools.iter().find(|tool| *tool == &repo.default_tool))
                .map(|tool| tool.as_str())
        })
        .unwrap_or("Default agent for repo");
    let tool_selected = matches!(app.agent_field, AgentField::Tool);
    let tool_display = if tool_selected && !app.agent_filter_input.is_empty() {
        app.agent_filter_input.as_str()
    } else {
        tool_name
    };
    let tool_border = if tool_selected {
        THEME.fg
    } else {
        THEME.fg_mid
    };
    let tool_title = if tool_selected {
        THEME.fg
    } else {
        THEME.fg_mid
    };
    let tool_text = if tool_selected {
        THEME.fg
    } else {
        THEME.fg_dim
    };
    let tool_block = Block::bordered()
        .title(Span::styled(
            "Agent to use",
            Style::default().fg(tool_title),
        ))
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(tool_border));
    let mut tool_lines = Vec::new();
    tool_lines.push(Line::from(Span::styled(
        tool_display.to_string(),
        Style::default().fg(tool_text),
    )));
    if let Some(repo) = app.repos.get(app.selected_repo) {
        if tool_filtered.is_empty() {
            tool_lines.push(Line::from(Span::styled(
                "No matches",
                Style::default().fg(THEME.fg_dim),
            )));
        } else {
            for index in &tool_filtered {
                let tool = &repo.tools[*index];
                let marker = if *index == app.selected_tool {
                    ">"
                } else {
                    " "
                };
                let style = if *index == app.selected_tool {
                    Style::default().fg(THEME.fg)
                } else {
                    Style::default().fg(THEME.fg_dim)
                };
                tool_lines.push(Line::from(Span::styled(
                    format!("{} {}", marker, tool),
                    style,
                )));
            }
        }
    }
    let tool_content = Paragraph::new(tool_lines).block(tool_block);
    frame.render_widget(tool_content, tool_rect);

    let create_selected = matches!(app.agent_field, AgentField::Create);
    let create_border = if create_selected {
        Color::White
    } else {
        THEME.fg_mid
    };
    let create_text = if create_selected {
        THEME.fg
    } else {
        THEME.fg_dim
    };
    let create_block = Block::bordered()
        .style(Style::default().bg(THEME.bg_alt2).fg(create_text))
        .border_style(Style::default().fg(create_border));
    let create_content = Paragraph::new("Create agent")
        .style(Style::default().fg(create_text))
        .alignment(Alignment::Center)
        .block(create_block);
    frame.render_widget(create_content, create_rect);

    let hint = Paragraph::new(
        "Tab to switch, type to filter, Enter to select, Enter on Create agent to confirm, Esc to cancel",
    )
    .style(Style::default().fg(THEME.fg_dim))
    .alignment(Alignment::Center);
    frame.render_widget(hint, sections[1]);
}

fn render_delete_agent_modal(frame: &mut ratatui::Frame, app: &App, base: Rect) {
    let label = app
        .delete_agent
        .as_ref()
        .map(|agent| agent.label.as_str())
        .unwrap_or("agent");

    let area = centered_rect(26, 23, base);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(Line::from(vec![
            Span::raw("Delete agent "),
            Span::styled(label, Style::default().fg(THEME.orange)).add_modifier(Modifier::BOLD),
            Span::raw("?")
        ]).centered())
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.fg))
        .padding(Padding::new(1, 1, 1, 1)); 
    frame.render_widget(&block, area); 

    let inner = block.inner(area);

    let text = 
        "This will close its tmux session, delete the worktree, and delete the agent.";
    let sections = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let paragraph = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center)
        .style(Style::default().fg(THEME.fg_mid));
    frame.render_widget(paragraph, sections[0]);

    let button_layout =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(sections[1]);

    let cancel_selected = matches!(app.delete_agent_action, DeleteAgentAction::Cancel);
    let cancel_button_style = if cancel_selected { THEME.fg } else { THEME.fg_mid };

    let cancel_block = Block::bordered()
        .style(Style::default().bg(THEME.bg_alt2).fg(cancel_button_style))
        .border_style(Style::default().fg(cancel_button_style));
    let cancel_button = Paragraph::new("Cancel")
        .style(Style::default().fg(cancel_button_style))
        .alignment(Alignment::Center)
        .block(cancel_block);
    frame.render_widget(cancel_button, button_layout[0]);

    let delete_selected = matches!(app.delete_agent_action, DeleteAgentAction::Delete);
    let delete_button_style = if delete_selected { THEME.red } else { THEME.fg_mid };
    let delete_block = Block::bordered()
        .style(Style::default().bg(THEME.bg_alt2).fg(delete_button_style))
        .border_style(Style::default().fg(delete_button_style));
    let delete_button = Paragraph::new("Delete")
        .style(Style::default().fg(delete_button_style))
        .alignment(Alignment::Center)
        .block(delete_block);
    frame.render_widget(delete_button, button_layout[1]);

    let hint = Paragraph::new("Tab or arrow keys to switch, Enter to confirm, Esc to cancel.")
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(THEME.fg_dim))
        .alignment(Alignment::Center);
    frame.render_widget(hint, sections[3]);
}

fn render_show_repos_modal(frame: &mut ratatui::Frame, app: &App, base: Rect) {
    let area = centered_rect(70, 50, base);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title("Repos")
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.border));
    frame.render_widget(&block, area);
    let inner = block.inner(area);

    let repo_lines: Vec<String> = app
        .repos
        .iter()
        .map(|repo| format!("{}  {}", repo.name, repo.path.to_string_lossy()))
        .collect();

    let paragraph = Paragraph::new(repo_lines.join("\n"))
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(THEME.fg_mid));
    frame.render_widget(paragraph, inner);
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
