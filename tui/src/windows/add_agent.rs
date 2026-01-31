use crate::theme::THEME;
use crate::{
    add_agent, default_tool_index, filtered_repo_indices, filtered_tool_indices,
    sync_filtered_selection, AgentField, App,
};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    Frame,
};
use std::error::Error;
use termwiz::input::{KeyCode, KeyEvent, Modifiers};

use super::Window;

pub struct AddAgentWindow;

impl Window for AddAgentWindow {
    fn render(frame: &mut Frame, app: &mut App, area: Rect) {
        render_add_agent_window(frame, app, area);
    }

    fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
        handle_add_agent_keys(app, key)
    }
}

fn handle_add_agent_keys(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
    match key.key {
        KeyCode::Escape => {
            app.focused_window = None;
            app.agent_filter_input.clear();
        }
        KeyCode::Tab if key.modifiers.contains(Modifiers::SHIFT) => {
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
        KeyCode::UpArrow => match app.agent_field {
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
        KeyCode::DownArrow => match app.agent_field {
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
                        app.focused_window = None;
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

    Ok(false)
}

fn render_add_agent_window(frame: &mut Frame, app: &App, base: Rect) {
    let area = crate::centered_rect(70, 60, base);
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
