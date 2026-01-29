use crate::theme::THEME;
use crate::{
    default_tool_index, sync_filtered_selection, Agent, AgentField, App, DeleteAgentAction,
    DeleteAgentTarget,
};
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget},
    Frame,
};
use std::error::Error;
use termwiz::cell::{Blink, CellAttributes, Intensity, Underline};
use termwiz::color::{ColorAttribute, SrgbaTuple};
use termwiz::surface::{CursorShape, CursorVisibility, Surface};

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
            if let Some(agent) = app.agents.get(app.selected_agent) {
                app.focused_agent = Some(agent.name.clone());
            }
        }
        KeyCode::Up => {
            if app.selected_agent > 0 {
                app.selected_agent -= 1;
            }
        }
        KeyCode::Down => {
            if app.selected_agent + 1 < app.agents.len() {
                app.selected_agent += 1;
            }
        }
        _ => {}
    }

    Ok(false)
}

fn render_agents(frame: &mut Frame, area: Rect, app: &mut App) {
    let padded_area = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };

    if app.agents.is_empty() {
        let empty = Paragraph::new("No agents yet. Press (a) to add one.")
            .style(Style::default().fg(THEME.fg_mid))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(empty, padded_area);
        return;
    }

    let sections =
        Layout::horizontal([Constraint::Length(32), Constraint::Min(0)]).split(padded_area);
    render_agent_sidebar(frame, sections[0], app);
    render_agent_preview(frame, sections[1], app);
}

fn render_agent_sidebar(frame: &mut Frame, area: Rect, app: &mut App) {
    let entry_height = 4usize;
    let visible_entries = (area.height as usize / entry_height).max(1);
    let total_entries = app.agents.len();

    if total_entries <= visible_entries {
        app.agent_scroll = 0;
    } else {
        let max_scroll = total_entries - visible_entries;
        if app.selected_agent < app.agent_scroll {
            app.agent_scroll = app.selected_agent;
        } else if app.selected_agent >= app.agent_scroll + visible_entries {
            app.agent_scroll = app.selected_agent + 1 - visible_entries;
        }
        app.agent_scroll = app.agent_scroll.min(max_scroll);
    }

    let max_scroll = total_entries.saturating_sub(visible_entries);
    let scrollbar_position = if max_scroll > 0 {
        app.agent_scroll * (total_entries.saturating_sub(1)) / max_scroll
    } else {
        0
    };

    let list_area = if total_entries > visible_entries && area.width > 1 {
        Rect {
            width: area.width - 1,
            ..area
        }
    } else {
        area
    };

    let mut row_constraints = Vec::new();
    let start_index = app.agent_scroll;
    let end_index = (start_index + visible_entries).min(total_entries);
    for index in start_index..end_index {
        row_constraints.push(Constraint::Length(entry_height as u16));
        if index + 1 < end_index {
            row_constraints.push(Constraint::Length(0));
        }
    }
    let row_areas = Layout::vertical(row_constraints).split(list_area);

    for (visible_index, agent_index) in (start_index..end_index).enumerate() {
        if let Some(agent) = app.agents.get(agent_index) {
            let area_index = visible_index * 2;
            let block_style = if agent_index == app.selected_agent {
                Style::default().bg(THEME.bg_alt)
            } else {
                Style::default().bg(THEME.bg)
            };
            let block = Block::default().style(block_style).padding(Padding {
                left: 2,
                right: 1,
                top: 1,
                bottom: 1,
            });
            let row_area = row_areas[area_index];
            frame.render_widget(&block, row_area);

            let inner_area = block.inner(row_area);
            let name_line = build_name_line(agent, app.animation_start);
            let repo_line =
                Line::from(Span::styled(&agent.repo, Style::default().fg(THEME.fg_mid)));
            let lines = vec![name_line, repo_line];
            let paragraph = Paragraph::new(lines)
                .style(block_style)
                .alignment(ratatui::layout::Alignment::Left);
            frame.render_widget(paragraph, inner_area);
        }
    }

    if total_entries > visible_entries {
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        let mut scrollbar_state = ScrollbarState::new(total_entries)
            .position(scrollbar_position)
            .viewport_content_length(visible_entries);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(THEME.fg_dim));
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn render_agent_preview(frame: &mut Frame, area: Rect, app: &mut App) {
    let inner_area = area;

    if app.agents.is_empty() {
        let empty = Paragraph::new("No agents yet. Press (a) to add one.")
            .style(Style::default().fg(THEME.fg_mid))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(empty, inner_area);
        return;
    }

    let agent = app.agents[app.selected_agent].clone();
    app.ensure_pty_view(&agent, inner_area);

    if let Some(view) = app.pty_views.get_mut(&agent.name) {
        let blink_on = !app.focused_agent.is_some()
            || (app.animation_start.elapsed().as_millis() / 700) % 2 == 0;
        let preview = TermwizPreview {
            surface: view.active_surface_mut(),
            blink_on,
        };
        frame.render_widget(preview, inner_area);
    } else {
        let paragraph = Paragraph::new("No PTY preview available yet.")
            .style(Style::default().fg(THEME.fg))
            .alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(paragraph, inner_area);
    }
}

pub(crate) struct TermwizPreview<'a> {
    pub(crate) surface: &'a mut Surface,
    pub(crate) blink_on: bool,
}

impl Widget for TermwizPreview<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let width = area.width as usize;
        let height = area.height as usize;
        for y in 0..height {
            for x in 0..width {
                let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
                cell.set_symbol(" ");
            }
        }
        let lines = self.surface.screen_lines();
        for (row, line) in lines.into_iter().take(height).enumerate() {
            for cell in line.visible_cells() {
                let col = cell.cell_index();
                if col >= width {
                    continue;
                }
                let symbol = cell.str();
                let attrs = cell.attrs();
                let style = termwiz_style_to_ratatui(attrs);
                let cell_buf = buf.get_mut(area.x + col as u16, area.y + row as u16);
                cell_buf.set_symbol(symbol);
                cell_buf.set_style(style);
            }
        }

        let cursor_visible = matches!(self.surface.cursor_visibility(), CursorVisibility::Visible);
        let cursor_shape = self.surface.cursor_shape().unwrap_or(CursorShape::Default);
        let should_blink = cursor_shape.is_blinking();
        let show_cursor = cursor_visible && (!should_blink || self.blink_on);
        let is_block = matches!(
            cursor_shape,
            CursorShape::Default | CursorShape::BlinkingBlock | CursorShape::SteadyBlock
        );

        if show_cursor && is_block {
            let (cursor_x, cursor_y) = self.surface.cursor_position();
            if cursor_x < width && cursor_y < height {
                let cursor_cell = buf.get_mut(area.x + cursor_x as u16, area.y + cursor_y as u16);
                let mut style = cursor_cell.style();
                style = style.add_modifier(Modifier::REVERSED);
                cursor_cell.set_style(style);
            }
        }
    }
}

fn termwiz_style_to_ratatui(attrs: &CellAttributes) -> Style {
    let mut style = Style::default();
    if let Some(color) = termwiz_color_to_ratatui(attrs.foreground()) {
        style = style.fg(color);
    }
    if let Some(color) = termwiz_color_to_ratatui(attrs.background()) {
        style = style.bg(color);
    }
    let mut modifier = Modifier::empty();
    match attrs.intensity() {
        Intensity::Bold => modifier |= Modifier::BOLD,
        Intensity::Half => modifier |= Modifier::DIM,
        Intensity::Normal => {}
    }
    if matches!(attrs.underline(), Underline::Single | Underline::Double) {
        modifier |= Modifier::UNDERLINED;
    }
    if attrs.italic() {
        modifier |= Modifier::ITALIC;
    }
    if matches!(attrs.blink(), Blink::Slow | Blink::Rapid) {
        modifier |= Modifier::SLOW_BLINK;
    }
    if attrs.reverse() {
        modifier |= Modifier::REVERSED;
    }
    if attrs.strikethrough() {
        modifier |= Modifier::CROSSED_OUT;
    }
    if attrs.invisible() {
        modifier |= Modifier::HIDDEN;
    }
    style.add_modifier(modifier)
}

fn termwiz_color_to_ratatui(color: ColorAttribute) -> Option<Color> {
    match color {
        ColorAttribute::Default => None,
        ColorAttribute::PaletteIndex(index) => Some(Color::Indexed(index)),
        ColorAttribute::TrueColorWithDefaultFallback(tuple)
        | ColorAttribute::TrueColorWithPaletteFallback(tuple, _) => {
            let SrgbaTuple(r, g, b, _) = tuple;
            Some(Color::Rgb(
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
            ))
        }
    }
}

fn build_name_line(agent: &Agent, animation_start: std::time::Instant) -> Line<'static> {
    crate::build_name_line(agent, animation_start)
}
