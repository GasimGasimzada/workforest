use crate::theme::THEME;
use crate::{delete_agent, App, DeleteAgentAction};
use crossterm::event::KeyCode;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph, Wrap},
    Frame,
};
use std::error::Error;

use super::Window;

pub struct DeleteAgentWindow;

impl Window for DeleteAgentWindow {
    fn render(frame: &mut Frame, app: &mut App, area: Rect) {
        render_delete_agent_window(frame, app, area);
    }

    fn handle_key_event(
        app: &mut App,
        key: crossterm::event::KeyEvent,
    ) -> Result<bool, Box<dyn Error>> {
        handle_delete_agent_keys(app, key)
    }
}

fn handle_delete_agent_keys(
    app: &mut App,
    key: crossterm::event::KeyEvent,
) -> Result<bool, Box<dyn Error>> {
    match key.code {
        KeyCode::Esc => {
            app.focused_window = None;
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
                app.focused_window = None;
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
                app.focused_window = None;
            }
        },
        _ => {}
    }

    Ok(false)
}

fn render_delete_agent_window(frame: &mut Frame, app: &App, base: Rect) {
    let label = app
        .delete_agent
        .as_ref()
        .map(|agent| agent.label.as_str())
        .unwrap_or("agent");

    let area = crate::centered_rect(26, 23, base);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(
            Line::from(vec![
                Span::raw("Delete agent "),
                Span::styled(label, Style::default().fg(THEME.orange)).add_modifier(Modifier::BOLD),
                Span::raw("?"),
            ])
            .centered(),
        )
        .style(Style::default().bg(THEME.bg_alt2).fg(THEME.fg))
        .border_style(Style::default().fg(THEME.fg))
        .padding(Padding::new(1, 1, 1, 1));
    frame.render_widget(&block, area);

    let inner = block.inner(area);

    let text = "This will close its session, delete the worktree, and delete the agent.";
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
    let cancel_button_style = if cancel_selected {
        THEME.fg
    } else {
        THEME.fg_mid
    };

    let cancel_block = Block::bordered()
        .style(Style::default().bg(THEME.bg_alt2).fg(cancel_button_style))
        .border_style(Style::default().fg(cancel_button_style));
    let cancel_button = Paragraph::new("Cancel")
        .style(Style::default().fg(cancel_button_style))
        .alignment(Alignment::Center)
        .block(cancel_block);
    frame.render_widget(cancel_button, button_layout[0]);

    let delete_selected = matches!(app.delete_agent_action, DeleteAgentAction::Delete);
    let delete_button_style = if delete_selected {
        THEME.red
    } else {
        THEME.fg_mid
    };
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
