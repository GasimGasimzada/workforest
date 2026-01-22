use workforest_core::APP_NAME;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    widgets::{Block, Paragraph},
    Terminal,
};
use reqwest::blocking::Client;
use std::{error::Error, io, time::Duration};

fn main() -> Result<(), Box<dyn Error>> {
    let server_url = std::env::var("WORKFOREST_SERVER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:0".to_string());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut status = fetch_status(&server_url);

    loop {
        terminal.draw(|frame| {
            let size = frame.size();
            let sections = Layout::vertical([Constraint::Percentage(100)]).split(size);
            let text = vec![
                format!("{}", APP_NAME.to_uppercase()).bold().to_string(),
                format!("Server: {}", server_url),
                format!("Status: {}", status),
                "Press r to refresh, q to quit.".to_string(),
            ]
            .join("\n");

            let block = Block::bordered().title("Workforest");
            frame.render_widget(Paragraph::new(text).block(block).style(Style::default()), sections[0]);
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? {
                match (code, modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                    (KeyCode::Char('r'), _) => status = fetch_status(&server_url),
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn fetch_status(server_url: &str) -> String {
    let url = format!("{}/health", server_url);
    Client::new()
        .get(url)
        .send()
        .map(|response| if response.status().is_success() { "connected" } else { "error" })
        .unwrap_or("offline")
        .to_string()
}
