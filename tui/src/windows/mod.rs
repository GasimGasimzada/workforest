use crate::App;
use crossterm::event::KeyEvent;
use ratatui::{layout::Rect, Frame};
use std::error::Error;

pub mod add_agent;
pub mod add_repo;
pub mod delete_agent;
pub mod root;
pub mod show_repos;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowId {
    Root,
    AddRepo,
    AddAgent,
    ShowRepos,
    DeleteAgent,
}

pub trait Window {
    fn render(frame: &mut Frame, app: &mut App, area: Rect);
    fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>>;
}

pub fn render_window(id: WindowId, frame: &mut Frame, app: &mut App, area: Rect) {
    match id {
        WindowId::Root => <root::RootWindow as Window>::render(frame, app, area),
        WindowId::AddRepo => <add_repo::AddRepoWindow as Window>::render(frame, app, area),
        WindowId::AddAgent => <add_agent::AddAgentWindow as Window>::render(frame, app, area),
        WindowId::ShowRepos => <show_repos::ShowReposWindow as Window>::render(frame, app, area),
        WindowId::DeleteAgent => {
            <delete_agent::DeleteAgentWindow as Window>::render(frame, app, area)
        }
    }
}

pub fn handle_window_key_event(
    id: WindowId,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, Box<dyn Error>> {
    match id {
        WindowId::Root => <root::RootWindow as Window>::handle_key_event(app, key),
        WindowId::AddRepo => <add_repo::AddRepoWindow as Window>::handle_key_event(app, key),
        WindowId::AddAgent => <add_agent::AddAgentWindow as Window>::handle_key_event(app, key),
        WindowId::ShowRepos => <show_repos::ShowReposWindow as Window>::handle_key_event(app, key),
        WindowId::DeleteAgent => {
            <delete_agent::DeleteAgentWindow as Window>::handle_key_event(app, key)
        }
    }
}
