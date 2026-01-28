use crate::theme::THEME;
use crate::{
    default_tool_index, open_agent_tmux, sync_filtered_selection, Agent, AgentField, App,
    DeleteAgentAction, DeleteAgentTarget, AGENT_COLUMNS,
};
use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{
        Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};
use std::error::Error;

use super::Window;

pub struct RootWindow;

impl Window for RootWindow {
    fn render(frame: &mut Frame, app: &mut App, area: Rect) {
        render_agents(frame, area, app);
    }

    fn handle_key_event(
        app: &mut App,
        key: crossterm::event::KeyEvent,
    ) -> Result<bool, Box<dyn Error>> {
        handle_root_keys(app, key)
    }
}

fn handle_root_keys(
    app: &mut App,
    key: crossterm::event::KeyEvent,
) -> Result<bool, Box<dyn Error>> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('r') => {
            app.focused_window = Some(super::WindowId::AddRepo);
            app.input.clear();
            app.status_message = None;
        }
        KeyCode::Char('a') => {
            if app.repos.is_empty() {
                app.set_status("add a repo first");
            } else {
                app.focused_window = Some(super::WindowId::AddAgent);
                app.selected_repo = app.selected_repo.min(app.repos.len() - 1);
                app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                app.agent_field = AgentField::Repo;
                app.agent_filter_input.clear();
                app.agent_name_input = petname::petname(2, "-");
                sync_filtered_selection(app);
                app.status_message = None;
            }
        }
        KeyCode::Char('l') => {
            app.focused_window = Some(super::WindowId::ShowRepos);
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
                app.focused_window = Some(super::WindowId::DeleteAgent);
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

fn render_agents(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.agents.is_empty() {
        let empty = Paragraph::new("No agents yet. Press (a) to add one.")
            .style(Style::default().fg(THEME.fg_mid))
            .alignment(ratatui::layout::Alignment::Center);
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
                    .alignment(ratatui::layout::Alignment::Left);
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

fn build_name_line(agent: &Agent, animation_start: std::time::Instant) -> Line<'static> {
    crate::build_name_line(agent, animation_start)
}
