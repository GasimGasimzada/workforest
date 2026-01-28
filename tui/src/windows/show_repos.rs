use crate::theme::THEME;
use crate::App;
use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::Style,
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};
use std::error::Error;

use super::Window;

pub struct ShowReposWindow;

impl Window for ShowReposWindow {
    fn render(frame: &mut Frame, app: &mut App, area: Rect) {
        render_show_repos_window(frame, app, area);
    }

    fn handle_key_event(
        app: &mut App,
        key: crossterm::event::KeyEvent,
    ) -> Result<bool, Box<dyn Error>> {
        handle_show_repos_keys(app, key)
    }
}

fn handle_show_repos_keys(
    app: &mut App,
    key: crossterm::event::KeyEvent,
) -> Result<bool, Box<dyn Error>> {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => app.focused_window = None,
        _ => {}
    }
    Ok(false)
}

fn render_show_repos_window(frame: &mut Frame, app: &App, base: Rect) {
    let area = crate::centered_rect(70, 50, base);
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
