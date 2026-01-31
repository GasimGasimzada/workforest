use crate::theme::THEME;
use crate::{add_repo, App};
use ratatui::{
    layout::Rect,
    style::Style,
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};
use std::error::Error;
use termwiz::input::{KeyCode, KeyEvent};

use super::Window;

pub struct AddRepoWindow;

impl Window for AddRepoWindow {
    fn render(frame: &mut Frame, app: &mut App, area: Rect) {
        render_add_repo_window(frame, app, area);
    }

    fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
        handle_add_repo_keys(app, key)
    }
}

fn handle_add_repo_keys(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
    match key.key {
        KeyCode::Escape => {
            app.focused_window = None;
        }
        KeyCode::Enter => {
            let path = app.input.trim();
            if path.is_empty() {
                app.set_status("repo path is required");
                return Ok(false);
            }

            match add_repo(&app.client, &app.server_url, path) {
                Ok(_) => {
                    app.refresh_data();
                    app.focused_window = None;
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
    Ok(false)
}

fn render_add_repo_window(frame: &mut Frame, app: &App, base: Rect) {
    let area = crate::centered_rect(70, 30, base);
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
