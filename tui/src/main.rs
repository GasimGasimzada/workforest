use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
    Terminal,
};
use std::time::Instant;
use tachyonfx::{color_from_hsl, color_to_hsl};

const ICON_IDLE: &str = "󰒲";
const ICON_ERROR: &str = "󰅚";
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, error::Error, io, time::Duration};
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
}

enum Modal {
    None,
    AddRepo,
    AddAgent,
    ShowRepos,
}

enum AgentField {
    Repo,
    Tool,
}

struct App {
    server_url: String,
    client: Client,
    agents: Vec<Agent>,
    repos: Vec<RepoConfig>,
    modal: Modal,
    input: String,
    selected_repo: usize,
    selected_tool: usize,
    agent_field: AgentField,
    status_message: Option<String>,
    animation_start: Instant,
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

        terminal.draw(|frame| draw(frame, &app))?;

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
            selected_repo: 0,
            selected_tool: 0,
            agent_field: AgentField::Repo,
            status_message: None,
            animation_start: Instant::now(),
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
    }

    Ok(should_exit)
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
                app.status_message = None;
            }
        }
        KeyCode::Char('l') => {
            app.modal = Modal::ShowRepos;
        }
        KeyCode::Char('u') => {
            app.refresh_data();
        }
        _ => {}
    }

    Ok(false)
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
        }
        KeyCode::Tab => {
            app.agent_field = match app.agent_field {
                AgentField::Repo => AgentField::Tool,
                AgentField::Tool => AgentField::Repo,
            };
        }
        KeyCode::Up => {
            if matches!(app.agent_field, AgentField::Repo) {
                if app.selected_repo > 0 {
                    app.selected_repo -= 1;
                    app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                }
            }
        }
        KeyCode::Down => {
            if matches!(app.agent_field, AgentField::Repo) {
                if app.selected_repo + 1 < app.repos.len() {
                    app.selected_repo += 1;
                    app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                }
            }
        }
        KeyCode::Left => {
            if matches!(app.agent_field, AgentField::Tool) {
                if app.selected_tool > 0 {
                    app.selected_tool -= 1;
                }
            }
        }
        KeyCode::Right => {
            if matches!(app.agent_field, AgentField::Tool) {
                let tools_len = app.repos[app.selected_repo].tools.len();
                if app.selected_tool + 1 < tools_len {
                    app.selected_tool += 1;
                }
            }
        }
        KeyCode::Enter => {
            let repo = &app.repos[app.selected_repo];
            let tool = repo
                .tools
                .get(app.selected_tool)
                .cloned()
                .unwrap_or_else(|| repo.default_tool.clone());

            match add_agent(&app.client, &app.server_url, &repo.name, &tool) {
                Ok(_) => {
                    app.refresh_data();
                    app.modal = Modal::None;
                }
                Err(err) => app.set_status(err),
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

fn draw(frame: &mut ratatui::Frame, app: &App) {
    let background_style = Style::default().bg(THEME.bg);
    let area = frame.area();
    frame.render_widget(Block::default().style(background_style), area);

    let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let padded = padded_rect(sections[0]);

    render_agents(frame, padded, app);

    let footer = Paragraph::new(
        "(a) add agent   (r) add repo   (l) show repos   (u) refresh   (q) quit   Esc to close",
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
    }
}

fn render_agents(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if app.agents.is_empty() {
        let empty = Paragraph::new("No agents yet. Press (a) to add one.")
            .style(Style::default().fg(THEME.fg_mid))
            .alignment(Alignment::Center);
        frame.render_widget(empty, area);
        return;
    }

    let columns = 3;
    let rows = (app.agents.len() + columns - 1) / columns;
    let mut row_constraints = Vec::new();
    for row in 0..rows {
        row_constraints.push(Constraint::Length(9));
        if row + 1 < rows {
            row_constraints.push(Constraint::Length(1));
        }
    }
    let row_areas = Layout::vertical(row_constraints).split(area);

    for row_index in 0..rows {
        let row_area = row_areas[row_index * 2];
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
                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(THEME.green))
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
    let area = centered_rect(70, 50, base);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title("Add agent")
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.border));
    frame.render_widget(&block, area);
    let inner = block.inner(area);

    let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(inner);

    let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(sections[0]);

    let repo_lines: Vec<String> = app
        .repos
        .iter()
        .enumerate()
        .map(|(index, repo)| {
            let marker = if index == app.selected_repo {
                if matches!(app.agent_field, AgentField::Repo) {
                    ">"
                } else {
                    "*"
                }
            } else {
                " "
            };
            format!("{} {}", marker, repo.name)
        })
        .collect();

    let repo_block = Block::bordered()
        .title("Repos")
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.border));
    frame.render_widget(
        Paragraph::new(repo_lines.join("\n")).block(repo_block),
        columns[0],
    );

    let tool_lines: Vec<String> = app
        .repos
        .get(app.selected_repo)
        .map(|repo| {
            repo.tools
                .iter()
                .enumerate()
                .map(|(index, tool)| {
                    let marker = if index == app.selected_tool {
                        if matches!(app.agent_field, AgentField::Tool) {
                            ">"
                        } else {
                            "*"
                        }
                    } else {
                        " "
                    };
                    format!("{} {}", marker, tool)
                })
                .collect()
        })
        .unwrap_or_else(Vec::new);

    let tool_block = Block::bordered()
        .title("Tools")
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.border));
    frame.render_widget(
        Paragraph::new(tool_lines.join("\n")).block(tool_block),
        columns[1],
    );

    let hint = Paragraph::new("Tab to switch, arrows to select, Enter to create, Esc to cancel")
        .style(Style::default().fg(THEME.fg_dim))
        .alignment(Alignment::Center);
    frame.render_widget(hint, sections[1]);
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
        "running" => {
            let animated = animated_running_style(animation_start);
            Line::from(Span::styled(
                agent.label.clone(),
                animated.add_modifier(Modifier::BOLD),
            ))
        }
        "error" => icon_name_line(ICON_ERROR, THEME.red, &agent.label),
        "idle" => icon_name_line(ICON_IDLE, THEME.blue, &agent.label),
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

fn animated_running_style(animation_start: Instant) -> Style {
    let (hue, saturation, lightness) = color_to_hsl(&THEME.orange);
    let elapsed = animation_start.elapsed().as_secs_f32();
    let shifted_hue = (hue + (elapsed * 60.0)) % 360.0;
    let color = color_from_hsl(shifted_hue, saturation, lightness);
    Style::default().fg(color)
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

fn add_agent(client: &Client, server_url: &str, repo: &str, tool: &str) -> Result<Agent, String> {
    let url = format!("{}/agents", server_url);
    let response = client
        .post(url)
        .json(&AddAgentRequest {
            repo: repo.to_string(),
            tool: tool.to_string(),
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
