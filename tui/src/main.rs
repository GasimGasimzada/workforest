use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags, SockaddrStorage};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Terminal,
};
use std::io::IoSliceMut;
use std::{
    collections::HashMap,
    error::Error,
    io::{self, Read, Write},
    os::fd::FromRawFd,
    os::unix::io::{AsRawFd, RawFd},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use termwiz::cell::{AttributeChange, CellAttributes};
use termwiz::color::ColorAttribute;
use termwiz::escape::csi::{
    Cursor, CursorStyle, DecPrivateMode, DecPrivateModeCode, Edit, EraseInDisplay, EraseInLine,
    Mode, Sgr, TerminalMode, TerminalModeCode, CSI,
};
use termwiz::escape::esc::EscCode;
use termwiz::escape::osc::OperatingSystemCommand;
use termwiz::escape::{parser::Parser, Action, ControlCode, Esc};
use termwiz::surface::{Change, Position as TermwizPosition, Surface};

mod theme;
mod windows;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use theme::{ICON_ACTIVE, ICON_ERROR, ICON_IDLE, THEME};
use windows::root::TermwizPreview;
use windows::{handle_window_key_event, render_window, WindowId};
use workforest_core::{data_dir, RepoConfig};

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
    name: Option<String>,
}

struct DeleteAgentTarget {
    name: String,
    label: String,
}

enum DeleteAgentAction {
    Cancel,
    Delete,
}

enum AgentField {
    Repo,
    Name,
    Tool,
    Create,
}

struct App {
    server_url: String,
    client: Client,
    agents: Vec<Agent>,
    repos: Vec<RepoConfig>,
    windows: Vec<WindowId>,
    focused_window: Option<WindowId>,
    input: String,
    agent_name_input: String,
    agent_filter_input: String,
    selected_repo: usize,
    selected_tool: usize,
    selected_agent: usize,
    agent_scroll: usize,
    agent_field: AgentField,
    status_message: Option<String>,
    animation_start: Instant,
    delete_agent: Option<DeleteAgentTarget>,
    delete_agent_action: DeleteAgentAction,
    pty_socket_path: PathBuf,
    pty_views: HashMap<String, PtyView>,
    focused_agent: Option<String>,
}

struct PtyView {
    agent: String,
    main_surface: Surface,
    alt_surface: Surface,
    use_alt_screen: bool,
    saved_cursor_main: Option<(usize, usize)>,
    saved_cursor_alt: Option<(usize, usize)>,
    parser: Parser,
    receiver: Receiver<Vec<u8>>,
    reader: PtyReader,
    last_size: (u16, u16),
    scroll_region: Option<(usize, usize)>,
}

struct PtyReader {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
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
    let mut actions = Vec::new();
    let mut dirty = true;
    let mut last_blink_on =
        !app.focused_agent.is_some() || (app.animation_start.elapsed().as_millis() / 700) % 2 == 0;

    loop {
        let blink_on = !app.focused_agent.is_some()
            || (app.animation_start.elapsed().as_millis() / 700) % 2 == 0;
        if blink_on != last_blink_on {
            last_blink_on = blink_on;
            dirty = true;
        }
        if last_refresh.elapsed() >= Duration::from_secs(5) {
            app.refresh_data();
            last_refresh = Instant::now();
            dirty = true;
        }

        if app.pump_pty_output(&mut actions) {
            dirty = true;
        }

        if dirty {
            terminal.draw(|frame| draw(frame, &mut app))?;
            dirty = false;
        }

        let poll_timeout = if app.focused_agent.is_some() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(16)
        };

        if event::poll(poll_timeout)? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(&mut app, key)? {
                    break;
                }
                dirty = true;
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
            windows: vec![
                WindowId::Root,
                WindowId::AddRepo,
                WindowId::AddAgent,
                WindowId::ShowRepos,
                WindowId::DeleteAgent,
            ],
            focused_window: None,
            input: String::new(),
            agent_name_input: String::new(),
            agent_filter_input: String::new(),
            selected_repo: 0,
            selected_tool: 0,
            selected_agent: 0,
            agent_scroll: 0,
            agent_field: AgentField::Repo,
            status_message: None,
            animation_start: Instant::now(),
            delete_agent: None,
            delete_agent_action: DeleteAgentAction::Cancel,
            pty_socket_path: data_dir().join("pty.sock"),
            pty_views: HashMap::new(),
            focused_agent: None,
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
        if self.agents.is_empty() {
            self.selected_agent = 0;
        } else if self.selected_agent >= self.agents.len() {
            self.selected_agent = self.agents.len() - 1;
        }
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

        let existing: std::collections::HashSet<String> =
            self.agents.iter().map(|agent| agent.name.clone()).collect();
        self.pty_views.retain(|name, _| existing.contains(name));
        if let Some(focused) = &self.focused_agent {
            if !existing.contains(focused) {
                self.focused_agent = None;
            }
        }
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }

    fn pump_pty_output(&mut self, actions: &mut Vec<Action>) -> bool {
        let mut updated = false;
        for view in self.pty_views.values_mut() {
            loop {
                let chunk = match view.receiver.try_recv() {
                    Ok(chunk) => chunk,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                };
                actions.clear();
                view.parser.parse(&chunk, |action| actions.push(action));
                for action in actions.drain(..) {
                    apply_action_to_view(action, view);
                }
                updated = true;
            }
        }
        updated
    }

    fn ensure_pty_view(&mut self, agent: &Agent, area: Rect) {
        let size = (area.width.max(1), area.height.max(1));
        if !self.pty_views.contains_key(&agent.name) {
            match PtyView::attach(&self.pty_socket_path, agent, size) {
                Ok(view) => {
                    self.pty_views.insert(agent.name.clone(), view);
                    if let Err(err) = send_resize(&self.pty_socket_path, &agent.name, size) {
                        self.set_status(err);
                    }
                }
                Err(err) => self.set_status(err),
            }
        }
        if let Some(view) = self.pty_views.get_mut(&agent.name) {
            if view.last_size != size {
                view.last_size = size;
                view.resize(size);
                if let Err(err) = send_resize(&self.pty_socket_path, &view.agent, size) {
                    self.set_status(err);
                }
            }
        }
    }
}

impl PtyView {
    fn attach(socket_path: &PathBuf, agent: &Agent, size: (u16, u16)) -> Result<Self, String> {
        let (fd, history) = request_attach(socket_path, &agent.name)?;
        let parser = Parser::new();
        let main_surface = Surface::new(size.0 as usize, size.1 as usize);
        let alt_surface = Surface::new(size.0 as usize, size.1 as usize);
        let (reader, receiver) = PtyReader::spawn(fd)?;
        let mut view = Self {
            agent: agent.name.clone(),
            main_surface,
            alt_surface,
            use_alt_screen: false,
            saved_cursor_main: None,
            saved_cursor_alt: None,
            parser,
            receiver,
            reader,
            last_size: size,
            scroll_region: None,
        };
        if !history.is_empty() {
            let mut actions = Vec::new();
            view.parser.parse(&history, |action| actions.push(action));
            for action in actions {
                apply_action_to_view(action, &mut view);
            }
        }
        Ok(view)
    }

    pub(crate) fn active_surface(&self) -> &Surface {
        if self.use_alt_screen {
            &self.alt_surface
        } else {
            &self.main_surface
        }
    }

    pub(crate) fn active_surface_mut(&mut self) -> &mut Surface {
        if self.use_alt_screen {
            &mut self.alt_surface
        } else {
            &mut self.main_surface
        }
    }

    fn active_saved_cursor_mut(&mut self) -> &mut Option<(usize, usize)> {
        if self.use_alt_screen {
            &mut self.saved_cursor_alt
        } else {
            &mut self.saved_cursor_main
        }
    }

    fn resize(&mut self, size: (u16, u16)) {
        self.main_surface.resize(size.0 as usize, size.1 as usize);
        self.alt_surface.resize(size.0 as usize, size.1 as usize);
        self.scroll_region = None;
    }
}

impl PtyReader {
    fn spawn(fd: RawFd) -> Result<(Self, Receiver<Vec<u8>>), String> {
        fcntl(fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).map_err(|err| err.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let stop_thread = stop.clone();
        let handle = thread::spawn(move || read_pty_loop(fd, stop_thread, sender));
        Ok((
            Self {
                stop,
                handle: Some(handle),
            },
            receiver,
        ))
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PtyReader {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_pty_loop(fd: RawFd, stop: Arc<AtomicBool>, sender: Sender<Vec<u8>>) {
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut buffer = [0u8; 4096];
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(size) => {
                let _ = sender.send(buffer[..size].to_vec());
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

fn request_attach(socket_path: &PathBuf, agent: &str) -> Result<(RawFd, Vec<u8>), String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|err| err.to_string())?;
    stream
        .write_all(format!("ATTACH {}\n", agent).as_bytes())
        .map_err(|err| err.to_string())?;
    let history = receive_history(&mut stream)?;
    let fd = receive_fd(&stream)?;
    Ok((fd, history))
}

fn receive_history(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = stream.read(&mut byte).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("unexpected EOF while reading history header".to_string());
        }
        if byte[0] == b'\n' {
            break;
        }
        header.push(byte[0]);
    }
    let header = String::from_utf8(header).map_err(|err| err.to_string())?;
    let mut parts = header.split_whitespace();
    let label = parts.next().unwrap_or("");
    if label != "HISTORY" {
        return Err(format!("unexpected response: {label}"));
    }
    let len: usize = parts
        .next()
        .ok_or_else(|| "missing history length".to_string())?
        .parse()
        .map_err(|_| "invalid history length".to_string())?;
    let mut history = vec![0u8; len];
    if len > 0 {
        stream
            .read_exact(&mut history)
            .map_err(|err| err.to_string())?;
    }
    Ok(history)
}

fn receive_fd(stream: &UnixStream) -> Result<RawFd, String> {
    let mut buf = [0u8; 64];
    let mut cmsgspace = nix::cmsg_space!([RawFd; 1]);
    let mut iov = [IoSliceMut::new(&mut buf)];
    let (bytes, received_fd) = {
        let msg = recvmsg::<SockaddrStorage>(
            stream.as_raw_fd(),
            &mut iov,
            Some(&mut cmsgspace),
            MsgFlags::empty(),
        )
        .map_err(|err| err.to_string())?;
        let bytes = msg.bytes;
        let mut received_fd = None;
        if let Ok(cmsgs) = msg.cmsgs() {
            for cmsg in cmsgs {
                if let ControlMessageOwned::ScmRights(fds) = cmsg {
                    if let Some(fd) = fds.first() {
                        received_fd = Some(*fd);
                        break;
                    }
                }
            }
        }
        (bytes, received_fd)
    };
    drop(iov);
    let response = String::from_utf8_lossy(&buf[..bytes]).trim().to_string();
    if !response.starts_with("OK") {
        return Err(response);
    }
    received_fd.ok_or_else(|| "missing PTY fd".to_string())
}

fn send_resize(socket_path: &PathBuf, agent: &str, size: (u16, u16)) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|err| err.to_string())?;
    stream
        .write_all(format!("RESIZE {} {} {}\n", agent, size.0, size.1).as_bytes())
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn send_input(socket_path: &PathBuf, agent: &str, payload: &[u8]) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|err| err.to_string())?;
    stream
        .write_all(format!("INPUT {} {}\n", agent, payload.len()).as_bytes())
        .map_err(|err| err.to_string())?;
    if !payload.is_empty() {
        stream.write_all(payload).map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn apply_action_to_view(action: Action, view: &mut PtyView) {
    match action {
        Action::Print(ch) => {
            view.active_surface_mut()
                .add_change(Change::Text(ch.to_string()));
        }
        Action::PrintString(text) => {
            view.active_surface_mut().add_change(Change::Text(text));
        }
        Action::Control(code) => match code {
            ControlCode::CarriageReturn => {
                view.active_surface_mut()
                    .add_change(Change::Text("\r".to_string()));
            }
            ControlCode::LineFeed => {
                view.active_surface_mut()
                    .add_change(Change::Text("\n".to_string()));
            }
            ControlCode::HorizontalTab => {
                view.active_surface_mut()
                    .add_change(Change::Text("\t".to_string()));
            }
            ControlCode::Backspace => {
                view.active_surface_mut()
                    .add_change(Change::CursorPosition {
                        x: TermwizPosition::Relative(-1),
                        y: TermwizPosition::Relative(0),
                    });
            }
            _ => {}
        },
        Action::CSI(csi) => apply_csi_to_view(csi, view),
        Action::Esc(esc) => apply_esc_to_view(esc, view),
        Action::OperatingSystemCommand(osc) => apply_osc_to_view(*osc, view),
        _ => {}
    }
}

fn apply_esc_to_view(esc: Esc, view: &mut PtyView) {
    match esc {
        Esc::Code(code) => match code {
            EscCode::DecSaveCursorPosition => {
                let cursor_pos = view.active_surface().cursor_position();
                *view.active_saved_cursor_mut() = Some(cursor_pos);
            }
            EscCode::DecRestoreCursorPosition => {
                if let Some((x, y)) = *view.active_saved_cursor_mut() {
                    let surface = view.active_surface_mut();
                    surface.add_change(Change::CursorPosition {
                        x: TermwizPosition::Absolute(x),
                        y: TermwizPosition::Absolute(y),
                    });
                }
            }
            EscCode::Index => {
                let surface = view.active_surface_mut();
                surface.add_change(Change::Text("\n".to_string()));
            }
            EscCode::NextLine => {
                let surface = view.active_surface_mut();
                surface.add_change(Change::Text("\r\n".to_string()));
            }
            EscCode::ReverseIndex => {
                let surface = view.active_surface_mut();
                surface.add_change(Change::CursorPosition {
                    x: TermwizPosition::Relative(0),
                    y: TermwizPosition::Relative(-1),
                });
            }
            EscCode::FullReset => {
                let surface = view.active_surface_mut();
                surface.add_change(Change::ClearScreen(ColorAttribute::Default));
            }
            _ => {}
        },
        _ => {}
    }
}

fn apply_osc_to_view(osc: OperatingSystemCommand, view: &mut PtyView) {
    let surface = view.active_surface_mut();
    match osc {
        OperatingSystemCommand::SetIconNameAndWindowTitle(title)
        | OperatingSystemCommand::SetWindowTitle(title)
        | OperatingSystemCommand::SetWindowTitleSun(title)
        | OperatingSystemCommand::SetIconName(title)
        | OperatingSystemCommand::SetIconNameSun(title) => {
            surface.add_change(Change::Title(title));
        }
        _ => {}
    }
}

fn apply_csi_to_view(csi: CSI, view: &mut PtyView) {
    match csi {
        CSI::Cursor(cursor) => apply_cursor_to_view(cursor, view),
        CSI::Edit(edit) => apply_edit_to_view(edit, view),
        CSI::Mode(mode) => apply_mode_to_view(mode, view),
        CSI::Sgr(sgr) => {
            let surface = view.active_surface_mut();
            apply_sgr_to_surface(sgr, surface);
        }
        _ => {}
    }
}

fn apply_cursor_to_view(cursor: Cursor, view: &mut PtyView) {
    match cursor {
        Cursor::Left(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Relative(-(count as isize)),
                y: TermwizPosition::Relative(0),
            });
        }
        Cursor::Right(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Relative(count as isize),
                y: TermwizPosition::Relative(0),
            });
        }
        Cursor::Up(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Relative(0),
                y: TermwizPosition::Relative(-(count as isize)),
            });
        }
        Cursor::Down(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Relative(0),
                y: TermwizPosition::Relative(count as isize),
            });
        }
        Cursor::NextLine(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Absolute(0),
                y: TermwizPosition::Relative(count as isize),
            });
        }
        Cursor::PrecedingLine(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Absolute(0),
                y: TermwizPosition::Relative(-(count as isize)),
            });
        }
        Cursor::CharacterAbsolute(pos) | Cursor::CharacterPositionAbsolute(pos) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Absolute(pos.as_zero_based() as usize),
                y: TermwizPosition::Relative(0),
            });
        }
        Cursor::LinePositionAbsolute(pos) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Relative(0),
                y: TermwizPosition::Absolute(pos.saturating_sub(1) as usize),
            });
        }
        Cursor::LinePositionForward(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Relative(0),
                y: TermwizPosition::Relative(count as isize),
            });
        }
        Cursor::LinePositionBackward(count) => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Relative(0),
                y: TermwizPosition::Relative(-(count as isize)),
            });
        }
        Cursor::CharacterAndLinePosition { line, col }
        | Cursor::ActivePositionReport { line, col }
        | Cursor::Position { line, col } => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorPosition {
                x: TermwizPosition::Absolute(col.as_zero_based() as usize),
                y: TermwizPosition::Absolute(line.as_zero_based() as usize),
            });
        }
        Cursor::SaveCursor => {
            let cursor_pos = view.active_surface().cursor_position();
            *view.active_saved_cursor_mut() = Some(cursor_pos);
        }
        Cursor::RestoreCursor => {
            if let Some((x, y)) = *view.active_saved_cursor_mut() {
                let surface = view.active_surface_mut();
                surface.add_change(Change::CursorPosition {
                    x: TermwizPosition::Absolute(x),
                    y: TermwizPosition::Absolute(y),
                });
            }
        }
        Cursor::CursorStyle(style) => {
            let surface = view.active_surface_mut();
            let shape = match style {
                CursorStyle::Default => termwiz::surface::CursorShape::Default,
                CursorStyle::BlinkingBlock => termwiz::surface::CursorShape::BlinkingBlock,
                CursorStyle::SteadyBlock => termwiz::surface::CursorShape::SteadyBlock,
                CursorStyle::BlinkingUnderline => termwiz::surface::CursorShape::BlinkingUnderline,
                CursorStyle::SteadyUnderline => termwiz::surface::CursorShape::SteadyUnderline,
                CursorStyle::BlinkingBar => termwiz::surface::CursorShape::BlinkingBar,
                CursorStyle::SteadyBar => termwiz::surface::CursorShape::SteadyBar,
            };
            surface.add_change(Change::CursorShape(shape));
        }
        Cursor::SetTopAndBottomMargins { top, bottom } => {
            let height = view.active_surface().dimensions().1;
            let top = top.as_zero_based() as usize;
            let bottom = bottom.as_zero_based() as usize;
            if top < height && bottom < height && top < bottom {
                view.scroll_region = Some((top, bottom));
            } else {
                view.scroll_region = None;
            }
        }
        _ => {}
    }
}

fn apply_edit_to_view(edit: Edit, view: &mut PtyView) {
    match edit {
        Edit::EraseInDisplay(mode) => {
            let surface = view.active_surface_mut();
            match mode {
                EraseInDisplay::EraseToEndOfDisplay => {
                    surface.add_change(Change::ClearToEndOfScreen(ColorAttribute::Default));
                }
                EraseInDisplay::EraseToStartOfDisplay => {
                    surface.add_change(Change::ClearScreen(ColorAttribute::Default));
                }
                EraseInDisplay::EraseDisplay => {
                    surface.add_change(Change::ClearScreen(ColorAttribute::Default));
                }
                _ => {}
            }
        }
        Edit::EraseInLine(mode) => {
            let surface = view.active_surface_mut();
            match mode {
                EraseInLine::EraseToEndOfLine => {
                    surface.add_change(Change::ClearToEndOfLine(ColorAttribute::Default));
                }
                EraseInLine::EraseToStartOfLine => {
                    surface.add_change(Change::ClearToEndOfLine(ColorAttribute::Default));
                }
                EraseInLine::EraseLine => {
                    surface.add_change(Change::ClearToEndOfLine(ColorAttribute::Default));
                }
            }
        }
        Edit::ScrollUp(count) => {
            let height = view.active_surface().dimensions().1;
            let (first_row, region_size) = scroll_region(view, height);
            let surface = view.active_surface_mut();
            surface.add_change(Change::ScrollRegionUp {
                first_row,
                region_size,
                scroll_count: count as usize,
            });
        }
        Edit::ScrollDown(count) => {
            let height = view.active_surface().dimensions().1;
            let (first_row, region_size) = scroll_region(view, height);
            let surface = view.active_surface_mut();
            surface.add_change(Change::ScrollRegionDown {
                first_row,
                region_size,
                scroll_count: count as usize,
            });
        }
        _ => {}
    }
}

fn apply_mode_to_view(mode: Mode, view: &mut PtyView) {
    match mode {
        Mode::SetDecPrivateMode(mode) => apply_dec_private_mode(mode, view, true),
        Mode::ResetDecPrivateMode(mode) => apply_dec_private_mode(mode, view, false),
        Mode::SetMode(mode) => apply_terminal_mode(mode, view, true),
        Mode::ResetMode(mode) => apply_terminal_mode(mode, view, false),
        _ => {}
    }
}

fn apply_dec_private_mode(mode: DecPrivateMode, view: &mut PtyView, enabled: bool) {
    let code = match mode {
        DecPrivateMode::Code(code) => code,
        DecPrivateMode::Unspecified(_) => return,
    };
    match code {
        DecPrivateModeCode::ShowCursor => {
            let surface = view.active_surface_mut();
            surface.add_change(Change::CursorVisibility(if enabled {
                termwiz::surface::CursorVisibility::Visible
            } else {
                termwiz::surface::CursorVisibility::Hidden
            }));
        }
        DecPrivateModeCode::StartBlinkingCursor => {
            if enabled {
                let surface = view.active_surface_mut();
                surface.add_change(Change::CursorShape(
                    termwiz::surface::CursorShape::BlinkingBlock,
                ));
            }
        }
        DecPrivateModeCode::SaveCursor => {
            if enabled {
                let cursor_pos = view.active_surface().cursor_position();
                *view.active_saved_cursor_mut() = Some(cursor_pos);
            }
        }
        DecPrivateModeCode::ClearAndEnableAlternateScreen
        | DecPrivateModeCode::EnableAlternateScreen
        | DecPrivateModeCode::OptEnableAlternateScreen => {
            if enabled {
                view.use_alt_screen = true;
                if matches!(code, DecPrivateModeCode::ClearAndEnableAlternateScreen) {
                    view.alt_surface
                        .add_change(Change::ClearScreen(ColorAttribute::Default));
                }
            } else {
                view.use_alt_screen = false;
            }
        }
        _ => {}
    }
}

fn apply_terminal_mode(mode: TerminalMode, view: &mut PtyView, enabled: bool) {
    let surface = view.active_surface_mut();
    let code = match mode {
        TerminalMode::Code(code) => code,
        TerminalMode::Unspecified(_) => return,
    };
    match code {
        TerminalModeCode::ShowCursor => {
            surface.add_change(Change::CursorVisibility(if enabled {
                termwiz::surface::CursorVisibility::Visible
            } else {
                termwiz::surface::CursorVisibility::Hidden
            }));
        }
        _ => {}
    }
}

fn scroll_region(view: &PtyView, height: usize) -> (usize, usize) {
    if let Some((top, bottom)) = view.scroll_region {
        if bottom >= top {
            return (top, bottom - top + 1);
        }
    }
    (0, height)
}

fn apply_sgr_to_surface(sgr: Sgr, surface: &mut Surface) {
    match sgr {
        Sgr::Reset => {
            surface.add_change(Change::AllAttributes(CellAttributes::default()));
        }
        Sgr::Intensity(value) => {
            surface.add_change(Change::Attribute(AttributeChange::Intensity(value)));
        }
        Sgr::Underline(value) => {
            surface.add_change(Change::Attribute(AttributeChange::Underline(value)));
        }
        Sgr::Blink(value) => {
            surface.add_change(Change::Attribute(AttributeChange::Blink(value)));
        }
        Sgr::Italic(value) => {
            surface.add_change(Change::Attribute(AttributeChange::Italic(value)));
        }
        Sgr::Inverse(value) => {
            surface.add_change(Change::Attribute(AttributeChange::Reverse(value)));
        }
        Sgr::Invisible(value) => {
            surface.add_change(Change::Attribute(AttributeChange::Invisible(value)));
        }
        Sgr::StrikeThrough(value) => {
            surface.add_change(Change::Attribute(AttributeChange::StrikeThrough(value)));
        }
        Sgr::Foreground(color) => {
            surface.add_change(Change::Attribute(AttributeChange::Foreground(
                ColorAttribute::from(color),
            )));
        }
        Sgr::Background(color) => {
            surface.add_change(Change::Attribute(AttributeChange::Background(
                ColorAttribute::from(color),
            )));
        }
        Sgr::UnderlineColor(_) | Sgr::Font(_) | Sgr::Overline(_) | Sgr::VerticalAlign(_) => {}
    }
}

fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
    if let Some(agent) = app.focused_agent.clone() {
        if key.code == KeyCode::Char('d') && key.modifiers == KeyModifiers::CONTROL {
            app.focused_agent = None;
            return Ok(false);
        }
        if let Some(bytes) = key_event_to_bytes(key) {
            if let Err(err) = send_input(&app.pty_socket_path, &agent, &bytes) {
                app.set_status(err);
            }
        }
        return Ok(false);
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(true),
        _ => {}
    }

    if let Some(window) = app.focused_window {
        if app.windows.contains(&window) {
            return handle_window_key_event(window, app, key);
        }
        app.focused_window = None;
    }

    handle_window_key_event(WindowId::Root, app, key)
}

fn key_event_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers == KeyModifiers::CONTROL {
                if c.is_ascii_alphabetic() {
                    let value = (c.to_ascii_lowercase() as u8) - b'a' + 1;
                    return Some(vec![value]);
                }
                return None;
            }
            let mut buf = [0u8; 4];
            let encoded = c.encode_utf8(&mut buf);
            Some(encoded.as_bytes().to_vec())
        }
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        _ => None,
    }
}

fn filtered_repo_indices(app: &App) -> Vec<usize> {
    let filter = app.agent_filter_input.trim().to_lowercase();
    app.repos
        .iter()
        .enumerate()
        .filter(|(_, repo)| filter.is_empty() || repo.name.to_lowercase().contains(&filter))
        .map(|(index, _)| index)
        .collect()
}

fn filtered_tool_indices(app: &App) -> Vec<usize> {
    let filter = app.agent_filter_input.trim().to_lowercase();
    app.repos
        .get(app.selected_repo)
        .map(|repo| {
            repo.tools
                .iter()
                .enumerate()
                .filter(|(_, tool)| filter.is_empty() || tool.to_lowercase().contains(&filter))
                .map(|(index, _)| index)
                .collect()
        })
        .unwrap_or_default()
}

fn sync_filtered_selection(app: &mut App) {
    match app.agent_field {
        AgentField::Repo => {
            let indices = filtered_repo_indices(app);
            if let Some(first) = indices.first() {
                if !indices.contains(&app.selected_repo) {
                    app.selected_repo = *first;
                    app.selected_tool = default_tool_index(&app.repos[app.selected_repo]);
                }
            }
        }
        AgentField::Tool => {
            let indices = filtered_tool_indices(app);
            if let Some(first) = indices.first() {
                if !indices.contains(&app.selected_tool) {
                    app.selected_tool = *first;
                }
            }
        }
        _ => {}
    }
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    let background_style = Style::default().bg(THEME.bg);
    let area = frame.area();
    frame.render_widget(Block::default().style(background_style), area);

    let sections = Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).split(area);
    let content_area = sections[0];

    render_window(WindowId::Root, frame, app, content_area);

    let footer_line = if app.focused_agent.is_some() {
        Line::from(vec![
            Span::styled(
                " Agent focused ",
                Style::default()
                    .fg(THEME.bg)
                    .bg(THEME.orange)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled("Ctrl+D to unfocus", Style::default().fg(THEME.fg_dim)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " NORMAL ",
                Style::default().fg(THEME.fg_mid).bg(THEME.bg_alt2),
            ),
            Span::raw(" "),
            Span::styled(
                "(a) add agent   (d) delete agent   (R) restart agent   (r) add repo   (l) show repos   (u) refresh   Enter focus   Ctrl+D unfocus   (q) quit   Esc to close",
                Style::default().fg(THEME.fg_dim),
            ),
        ])
    };
    let footer = Paragraph::new(footer_line).alignment(Alignment::Left);
    let footer_area = sections[1].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(footer, footer_area);

    if let Some(window) = app.focused_window {
        render_window(window, frame, app, content_area);
    }
}

fn build_name_line(agent: &Agent, animation_start: Instant) -> Line<'static> {
    match agent.status.as_str() {
        "running" => icon_name_line(
            ICON_ACTIVE,
            pulsing_green_color(animation_start),
            &agent.label,
        ),
        "error" => icon_name_line(ICON_ERROR, THEME.red, &agent.label),
        "idle" => icon_name_line(ICON_IDLE, THEME.blue, &agent.label),
        "sleep" => icon_name_line(ICON_IDLE, THEME.fg_dim, &agent.label),
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

fn pulsing_green_color(animation_start: Instant) -> Color {
    let elapsed = animation_start.elapsed().as_secs_f32();
    let pulse = (elapsed * 2.0).sin().abs();
    blend_color(THEME.green_dim, THEME.green, pulse)
}

fn blend_color(from: Color, to: Color, amount: f32) -> Color {
    let clamped = amount.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(from_r, from_g, from_b), Color::Rgb(to_r, to_g, to_b)) => {
            let lerp_channel = |start: u8, end: u8| -> u8 {
                let start = f32::from(start);
                let end = f32::from(end);
                (start + (end - start) * clamped).round() as u8
            };
            Color::Rgb(
                lerp_channel(from_r, to_r),
                lerp_channel(from_g, to_g),
                lerp_channel(from_b, to_b),
            )
        }
        _ => to,
    }
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

fn add_agent(
    client: &Client,
    server_url: &str,
    repo: &str,
    tool: &str,
    name: Option<String>,
) -> Result<Agent, String> {
    let url = format!("{}/agents", server_url);
    let response = client
        .post(url)
        .json(&AddAgentRequest {
            repo: repo.to_string(),
            tool: tool.to_string(),
            name,
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

fn delete_agent(client: &Client, server_url: &str, name: &str) -> Result<(), String> {
    let url = format!("{}/agents/{}", server_url, name);
    let response = client.delete(url).send().map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(response
            .text()
            .unwrap_or_else(|_| "failed to delete agent".to_string()));
    }
    Ok(())
}

fn restart_agent(client: &Client, server_url: &str, name: &str) -> Result<(), String> {
    let url = format!("{}/agents/{}/restart", server_url, name);
    let response = client.post(url).send().map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(response
            .text()
            .unwrap_or_else(|_| "failed to restart agent".to_string()));
    }
    Ok(())
}
