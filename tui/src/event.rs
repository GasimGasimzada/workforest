use crossterm::terminal;
use nix::errno::Errno;
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::unistd::{close, pipe2, read, write};
use polling::{Event, Events, PollMode, Poller};
use signal_hook::consts::SIGWINCH;
use signal_hook::low_level::pipe::register;
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, RawFd};
use std::time::Duration;
use termwiz::input::{InputEvent, InputParser};

const STDIN_KEY: usize = 0;
const SIGWINCH_KEY: usize = 1;
const WAKE_KEY: usize = 2;

pub struct UIEvent {
    pub raw: Vec<u8>,
    pub event: InputEvent,
}

pub struct EventLoop {
    poller: Poller,
    events: Events,
    parser: InputParser,
    queue: VecDeque<UIEvent>,
    stdin_fd: RawFd,
    sigwinch_read: RawFd,
    sigwinch_write: RawFd,
    wake_read: RawFd,
    wake_write: RawFd,
}

impl EventLoop {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let poller = Poller::new()?;
        let events = Events::new();
        let stdin_fd = io::stdin().as_raw_fd();
        set_nonblocking(stdin_fd)?;

        let (sigwinch_read, sigwinch_write) = pipe2(OFlag::O_NONBLOCK).map_err(to_io_error)?;
        let sigwinch_read = sigwinch_read.into_raw_fd();
        let sigwinch_write = sigwinch_write.into_raw_fd();
        register(SIGWINCH, sigwinch_write)?;

        let (wake_read, wake_write) = pipe2(OFlag::O_NONBLOCK).map_err(to_io_error)?;
        let wake_read = wake_read.into_raw_fd();
        let wake_write = wake_write.into_raw_fd();

        unsafe {
            poller.add_with_mode(stdin_fd, Event::readable(STDIN_KEY), PollMode::Level)?;
            poller.add_with_mode(
                sigwinch_read,
                Event::readable(SIGWINCH_KEY),
                PollMode::Level,
            )?;
            poller.add_with_mode(wake_read, Event::readable(WAKE_KEY), PollMode::Level)?;
        }

        Ok(Self {
            poller,
            events,
            parser: InputParser::new(),
            queue: VecDeque::new(),
            stdin_fd,
            sigwinch_read,
            sigwinch_write,
            wake_read,
            wake_write,
        })
    }

    pub fn poll(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<UIEvent>, Box<dyn std::error::Error>> {
        if let Some(event) = self.queue.pop_front() {
            return Ok(Some(event));
        }

        self.events.clear();
        self.poller.wait(&mut self.events, Some(timeout))?;

        let keys: Vec<usize> = self.events.iter().map(|event| event.key).collect();
        for key in keys {
            match key {
                STDIN_KEY => {
                    let events = self.read_stdin_events()?;
                    self.queue.extend(events);
                }
                SIGWINCH_KEY => {
                    let events = self.handle_sigwinch()?;
                    self.queue.extend(events);
                }
                WAKE_KEY => {
                    let events = self.handle_wake()?;
                    self.queue.extend(events);
                }
                _ => {}
            }
        }

        Ok(self.queue.pop_front())
    }

    pub fn wake(&self) -> io::Result<()> {
        write_pipe(self.wake_write)
    }

    fn read_stdin_events(&mut self) -> io::Result<Vec<UIEvent>> {
        let raw = read_pipe(self.stdin_fd)?;
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        let events = self.parser.parse_as_vec(&raw, false);
        Ok(events
            .into_iter()
            .map(|event| UIEvent {
                raw: raw.clone(),
                event,
            })
            .collect())
    }

    fn handle_sigwinch(&mut self) -> io::Result<Vec<UIEvent>> {
        drain_pipe(self.sigwinch_read)?;
        let (cols, rows) = terminal::size()?;
        Ok(vec![UIEvent {
            raw: Vec::new(),
            event: InputEvent::Resized {
                cols: cols as usize,
                rows: rows as usize,
            },
        }])
    }

    fn handle_wake(&mut self) -> io::Result<Vec<UIEvent>> {
        drain_pipe(self.wake_read)?;
        Ok(vec![UIEvent {
            raw: Vec::new(),
            event: InputEvent::Wake,
        }])
    }
}

impl Drop for EventLoop {
    fn drop(&mut self) {
        unsafe {
            let _ = self.poller.delete(BorrowedFd::borrow_raw(self.stdin_fd));
            let _ = self
                .poller
                .delete(BorrowedFd::borrow_raw(self.sigwinch_read));
            let _ = self.poller.delete(BorrowedFd::borrow_raw(self.wake_read));
        }
        let _ = close(self.sigwinch_read);
        let _ = close(self.sigwinch_write);
        let _ = close(self.wake_read);
        let _ = close(self.wake_write);
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(to_io_error)?;
    let flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
    fcntl(fd, FcntlArg::F_SETFL(flags)).map_err(to_io_error)?;
    Ok(())
}

fn read_pipe(fd: RawFd) -> io::Result<Vec<u8>> {
    let mut buffer = [0u8; 4096];
    let mut output = Vec::new();
    loop {
        match read(fd, &mut buffer) {
            Ok(0) => break,
            Ok(size) => output.extend_from_slice(&buffer[..size]),
            Err(Errno::EAGAIN) => break,
            Err(err) => return Err(to_io_error(err)),
        }
    }
    Ok(output)
}

fn drain_pipe(fd: RawFd) -> io::Result<()> {
    let _ = read_pipe(fd)?;
    Ok(())
}

fn write_pipe(fd: RawFd) -> io::Result<()> {
    let buffer = [0u8; 1];
    match write(unsafe { BorrowedFd::borrow_raw(fd) }, &buffer) {
        Ok(_) => Ok(()),
        Err(Errno::EAGAIN) => Ok(()),
        Err(err) => Err(to_io_error(err)),
    }
}

fn to_io_error(err: Errno) -> io::Error {
    io::Error::from_raw_os_error(err as i32)
}
