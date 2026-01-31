use crate::theme::THEME;
use crate::{restart_agent, App, RestartAgentAction};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph, Wrap},
    Frame,
};
use std::error::Error;
use termwiz::input::{KeyCode, KeyEvent};

use super::Window;

pub struct RestartAgentWindow;

impl Window for RestartAgentWindow {
    fn render(frame: &mut Frame, app: &mut App, area: Rect) {
        render_restart_agent_window(frame, app, area);
    }

    fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
        handle_restart_agent_keys(app, key)
    }
}

fn handle_restart_agent_keys(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
    match key.key {
        KeyCode::Escape => {
            app.focused_window = None;
            app.restart_agent = None;
        }
        KeyCode::Tab | KeyCode::LeftArrow | KeyCode::RightArrow => {
            app.restart_agent_action = match app.restart_agent_action {
                RestartAgentAction::Cancel => RestartAgentAction::Restart,
                RestartAgentAction::Restart => RestartAgentAction::Cancel,
            };
        }
        KeyCode::Enter => match app.restart_agent_action {
            RestartAgentAction::Cancel => {
                app.focused_window = None;
                app.restart_agent = None;
            }
            RestartAgentAction::Restart => {
                if let Some(target) = app.restart_agent.take() {
                    match restart_agent(&app.client, &app.server_url, &target.name) {
                        Ok(()) => {
                            app.pty_views.remove(&target.name);
                            app.pending_pty.remove(&target.name);
                            app.set_status(format!("restarted agent {}", target.label));
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

fn render_restart_agent_window(frame: &mut Frame, app: &App, base: Rect) {
    let label = app
        .restart_agent
        .as_ref()
        .map(|agent| agent.label.as_str())
        .unwrap_or("agent");

    let area = crate::centered_rect(26, 23, base);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(
            Line::from(vec![
                Span::raw("Restart agent "),
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

    let text = "This will close its session and start a fresh one.";
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

    let cancel_selected = matches!(app.restart_agent_action, RestartAgentAction::Cancel);
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

    let restart_selected = matches!(app.restart_agent_action, RestartAgentAction::Restart);
    let restart_button_style = if restart_selected {
        THEME.orange
    } else {
        THEME.fg_mid
    };
    let restart_block = Block::bordered()
        .style(Style::default().bg(THEME.bg_alt2).fg(restart_button_style))
        .border_style(Style::default().fg(restart_button_style));
    let restart_button = Paragraph::new("Restart")
        .style(Style::default().fg(restart_button_style))
        .alignment(Alignment::Center)
        .block(restart_block);
    frame.render_widget(restart_button, button_layout[1]);

    let hint = Paragraph::new("Tab or arrow keys to switch, Enter to confirm, Esc to cancel.")
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(THEME.fg_dim))
        .alignment(Alignment::Center);
    frame.render_widget(hint, sections[3]);
}
