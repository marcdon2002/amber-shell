//! A transparent PTY relay with an in-memory, bounded command-output context.
mod protocol;
use crate::capture::{CaptureBuffer, SNAPSHOT_LIMIT};
use std::{
    collections::VecDeque,
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    process::Command,
    sync::atomic::{AtomicI32, Ordering},
    time::{Duration, Instant},
};

const QUEUE_LIMIT: usize = 64 * 1024;
static SIGNAL_FD: AtomicI32 = AtomicI32::new(-1);
extern "C" fn wake(signal: libc::c_int) {
    let fd = SIGNAL_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = signal as u8;
        unsafe {
            libc::write(fd, (&byte as *const u8).cast(), 1);
        }
    }
}
struct Signals {
    read: OwnedFd,
    _write: OwnedFd,
    old: Vec<(i32, libc::sigaction)>,
}
impl Signals {
    fn new() -> io::Result<Self> {
        let mut fds = [-1; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        for fd in fds {
            nonblocking(fd)?;
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        let mut this = Self {
            read,
            _write: write,
            old: vec![],
        };
        SIGNAL_FD.store(fds[1], Ordering::Relaxed);
        for signal in [
            libc::SIGWINCH,
            libc::SIGCHLD,
            libc::SIGHUP,
            libc::SIGTERM,
            libc::SIGINT,
            libc::SIGQUIT,
        ] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = wake as *const () as usize;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            let mut old = unsafe { std::mem::zeroed() };
            if unsafe { libc::sigaction(signal, &action, &mut old) } < 0 {
                return Err(io::Error::last_os_error());
            }
            this.old.push((signal, old));
        }
        Ok(this)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        SIGNAL_FD.store(-1, Ordering::Relaxed);
        for (signal, old) in &self.old {
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
    }
}
struct TerminalGuard {
    term: libc::termios,
    flags: [i32; 2],
}
impl TerminalGuard {
    fn new(term: libc::termios) -> io::Result<Self> {
        let flags = [unsafe { libc::fcntl(0, libc::F_GETFL) }, unsafe {
            libc::fcntl(1, libc::F_GETFL)
        }];
        let this = Self { term, flags };
        let mut raw = term;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        nonblocking(0)?;
        nonblocking(1)?;
        Ok(this)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.term);
            libc::fcntl(0, libc::F_SETFL, self.flags[0]);
            libc::fcntl(1, libc::F_SETFL, self.flags[1]);
        }
    }
}
struct Child {
    pid: i32,
    status: Option<i32>,
}
impl Child {
    fn poll(&mut self) {
        if self.status.is_some() {
            return;
        }
        let mut status = 0;
        if unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) } == self.pid {
            self.status = Some(if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else {
                128 + libc::WTERMSIG(status)
            });
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        if self.status.is_none() {
            unsafe {
                libc::kill(self.pid, libc::SIGHUP);
                libc::kill(self.pid, libc::SIGKILL);
                let mut status = 0;
                while libc::waitpid(self.pid, &mut status, 0) < 0
                    && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
                {
                }
            }
        }
    }
}
fn nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn resize(master: i32) {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) } == 0 {
        unsafe {
            libc::ioctl(master, libc::TIOCSWINSZ, &size);
        }
    }
}
fn forward_signal(master: i32, child: i32, signal: i32) {
    let mut group = 0;
    if unsafe { libc::ioctl(master, libc::TIOCGPGRP, &mut group) } == 0 && group > 0 {
        unsafe {
            libc::kill(-group, signal);
        }
    }
    if matches!(signal, libc::SIGHUP | libc::SIGTERM) {
        unsafe {
            libc::kill(child, signal);
        }
    }
}
fn write_queue(fd: i32, queue: &mut VecDeque<u8>) -> io::Result<()> {
    if queue.is_empty() {
        return Ok(());
    }
    let bytes = queue.make_contiguous();
    let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    if n > 0 {
        queue.drain(..n as usize);
    } else if n < 0 {
        let e = io::Error::last_os_error();
        if !matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) {
            return Err(e);
        }
    }
    Ok(())
}
#[derive(Default)]
struct InputPhase {
    ui: bool,
    awaiting_begin: bool,
    tainted: bool,
    paste: bool,
    escape: Vec<u8>,
}
impl InputPhase {
    fn input(&mut self, bytes: &[u8], capture: &mut CaptureBuffer) {
        if self.ui {
            return;
        }
        if !capture.is_active() && !self.awaiting_begin {
            self.tainted = false;
        }
        capture.input(bytes);
        for &byte in bytes {
            if self.awaiting_begin {
                self.tainted = true;
            }
            if !self.escape.is_empty() {
                self.escape.push(byte);
                if self.escape == b"\x1b[200~" {
                    self.paste = true;
                    self.escape.clear();
                } else if self.escape == b"\x1b[201~" {
                    self.paste = false;
                    self.escape.clear();
                } else if !b"\x1b[200~".starts_with(&self.escape)
                    && !b"\x1b[201~".starts_with(&self.escape)
                {
                    self.escape.clear();
                }
            } else if byte == 0x1b {
                self.escape.push(byte);
            } else if !self.paste && matches!(byte, b'\n' | b'\r') {
                self.awaiting_begin = true;
            }
        }
    }
    fn event(
        &mut self,
        event: protocol::Event,
        capture: &mut CaptureBuffer,
        queue: &mut VecDeque<u8>,
    ) {
        match event {
            protocol::Event::Output(bytes) => {
                capture.output(&bytes);
                queue.extend(bytes);
            }
            protocol::Event::Begin(command, cwd) => {
                self.ui = false;
                capture.begin(&command, &cwd);
                if self.tainted {
                    capture.input(b"typeahead");
                }
                self.awaiting_begin = false;
            }
            protocol::Event::Stop => capture.stop_output(),
            protocol::Event::End(status) => {
                capture.finish(status);
                self.awaiting_begin = false;
            }
            protocol::Event::Widget(active) => {
                self.ui = active;
                self.awaiting_begin = false;
                self.tainted = false;
                self.paste = false;
                self.escape.clear();
                if active {
                    capture.discard_active();
                }
            }
        }
    }
}

/// Bounded draining is essential: a background writer must not starve control messages.
fn drain(
    master: i32,
    queue: &mut VecDeque<u8>,
    capture: &mut CaptureBuffer,
    decoder: &mut protocol::Decoder,
    phase: &mut InputPhase,
) -> io::Result<bool> {
    let mut buffer = [0u8; 8192];
    for _ in 0..8 {
        let length = buffer.len().min(QUEUE_LIMIT.saturating_sub(queue.len()));
        if length == 0 {
            return Ok(false);
        }
        let n = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), length) };
        if n > 0 {
            for event in decoder.feed(&buffer[..n as usize]) {
                phase.event(event, capture, queue);
            }
        } else if n == 0 {
            return Ok(true);
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EIO) {
                return Ok(true);
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(false);
            }
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    Ok(false)
}

struct Client {
    stream: UnixStream,
    request: Vec<u8>,
    response: Option<Vec<u8>>,
    sent: usize,
    deadline: Instant,
}
impl Client {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            request: vec![],
            response: None,
            sent: 0,
            deadline: Instant::now() + Duration::from_millis(180),
        })
    }
}

pub fn run(arguments: &[String]) -> Result<i32, String> {
    if unsafe { libc::isatty(0) } != 1 || unsafe { libc::isatty(1) } != 1 {
        return Err("Output capture requires an interactive terminal".into());
    }
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut term) } < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    unsafe {
        libc::ioctl(0, libc::TIOCGWINSZ, &mut size);
    }
    let directory = tempfile::Builder::new()
        .prefix("amber-session-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let socket = directory.path().join("c.sock");
    let listener = UnixListener::bind(&socket).map_err(|e| e.to_string())?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let token = directory
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let signals = Signals::new().map_err(|e| e.to_string())?;
    let mut master = -1;
    // forkpty gives the child its own controlling terminal and foreground process groups.
    let pid = unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), &mut term, &mut size) };
    if pid < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        let (program, args) = if arguments.first().map(String::as_str) == Some("--") {
            &arguments[1..]
        } else {
            arguments
        }
        .split_first()
        .map_or(("/bin/zsh", &[][..]), |(p, a)| (p.as_str(), a));
        let mut command = Command::new(program);
        if args.is_empty() {
            command.arg("-i");
        } else {
            command.args(args);
        }
        command
            .env("AMBER_SHELL_CAPTURE_SOCKET", &socket)
            .env("AMBER_SHELL_CAPTURE_OWNER", std::process::id().to_string())
            .env("AMBER_SHELL_CAPTURE_TOKEN", &token);
        let error = command.exec();
        eprintln!("amber-shell: Could not start the shell: {error}");
        unsafe {
            libc::_exit(127);
        }
    }
    let mut child = Child { pid, status: None };
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    nonblocking(master.as_raw_fd()).map_err(|e| e.to_string())?;
    let _terminal = TerminalGuard::new(term).map_err(|e| e.to_string())?;
    match relay(master.as_raw_fd(), &listener, &signals, &mut child, &token) {
        Ok(code) => Ok(code),
        Err(error) => {
            eprintln!("amber-shell: Terminal connection ended: {error}");
            Ok(1)
        }
    }
}

fn relay(
    master: i32,
    listener: &UnixListener,
    signals: &Signals,
    child: &mut Child,
    token: &str,
) -> io::Result<i32> {
    let mut capture = CaptureBuffer::default();
    let mut incoming = VecDeque::with_capacity(QUEUE_LIMIT);
    let mut outgoing = VecDeque::with_capacity(QUEUE_LIMIT);
    let mut clients: Vec<Client> = vec![];
    let mut shutting_down: Option<Instant> = None;
    let mut phase = InputPhase::default();
    let mut decoder = protocol::Decoder::new(token);
    let mut master_open = true;
    let mut master_hungup = false;
    loop {
        child.poll();
        if let Some(status) = child.status {
            if master_open {
                master_open = !drain(
                    master,
                    &mut outgoing,
                    &mut capture,
                    &mut decoder,
                    &mut phase,
                )?;
            }
            if outgoing.is_empty() {
                return Ok(status);
            }
        }
        if shutting_down.is_some_and(|started| started.elapsed() > Duration::from_secs(2)) {
            unsafe {
                libc::kill(child.pid, libc::SIGKILL);
            }
            child.poll();
        }
        let now = Instant::now();
        clients.retain(|client| client.deadline > now);
        let mut poll = vec![
            libc::pollfd {
                fd: if shutting_down.is_none() { 0 } else { -1 },
                events: if incoming.len() < QUEUE_LIMIT && shutting_down.is_none() {
                    libc::POLLIN
                } else {
                    0
                },
                revents: 0,
            },
            libc::pollfd {
                fd: if master_open
                    && (outgoing.len() < QUEUE_LIMIT || !incoming.is_empty() && !master_hungup)
                {
                    master
                } else {
                    -1
                },
                events: (if outgoing.len() < QUEUE_LIMIT {
                    libc::POLLIN
                } else {
                    0
                }) | if incoming.is_empty() || master_hungup {
                    0
                } else {
                    libc::POLLOUT
                },
                revents: 0,
            },
            libc::pollfd {
                fd: 1,
                events: if outgoing.is_empty() {
                    0
                } else {
                    libc::POLLOUT
                },
                revents: 0,
            },
            libc::pollfd {
                fd: listener.as_raw_fd(),
                events: if clients.len() < 4 { libc::POLLIN } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: signals.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        for client in &clients {
            poll.push(libc::pollfd {
                fd: client.stream.as_raw_fd(),
                events: if client.response.is_some() {
                    libc::POLLOUT
                } else {
                    libc::POLLIN
                },
                revents: 0,
            });
        }
        let timeout = if shutting_down.is_some() {
            50
        } else {
            clients
                .iter()
                .map(|c| {
                    c.deadline
                        .saturating_duration_since(now)
                        .as_millis()
                        .min(180) as i32
                })
                .min()
                .unwrap_or(-1)
        };
        let result = unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as libc::nfds_t, timeout) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if poll[4].revents != 0 {
            let mut data = [0u8; 64];
            let n = unsafe {
                libc::read(
                    signals.read.as_raw_fd(),
                    data.as_mut_ptr().cast(),
                    data.len(),
                )
            };
            if n > 0 {
                for &signal in &data[..n as usize] {
                    match signal as i32 {
                        libc::SIGWINCH => resize(master),
                        libc::SIGCHLD => child.poll(),
                        value => {
                            forward_signal(master, child.pid, value);
                            if matches!(value, libc::SIGHUP | libc::SIGTERM) {
                                shutting_down.get_or_insert_with(Instant::now);
                            }
                        }
                    }
                }
            }
        }
        if poll[0].revents != 0 && shutting_down.is_none() {
            let mut data = [0u8; 8192];
            let length = data.len().min(QUEUE_LIMIT - incoming.len());
            let n = unsafe { libc::read(0, data.as_mut_ptr().cast(), length) };
            if n > 0 {
                let bytes = &data[..n as usize];
                phase.input(bytes, &mut capture);
                incoming.extend(bytes);
            } else if n == 0
                || n < 0
                    && !matches!(
                        io::Error::last_os_error().kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    )
            {
                forward_signal(master, child.pid, libc::SIGHUP);
                shutting_down.get_or_insert_with(Instant::now);
            }
        }
        if poll[2].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal output closed",
            ));
        }
        if poll[1].revents & libc::POLLHUP != 0 {
            master_hungup = true;
            incoming.clear();
        }
        if poll[1].revents & libc::POLLOUT != 0 {
            write_queue(master, &mut incoming)?;
        }
        if poll[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            master_open = !drain(
                master,
                &mut outgoing,
                &mut capture,
                &mut decoder,
                &mut phase,
            )?;
        }
        if poll[2].revents & libc::POLLOUT != 0 {
            write_queue(1, &mut outgoing)?;
        }
        // Existing clients first; newly accepted clients join the next poll iteration.
        for index in (0..clients.len()).rev() {
            let events = poll[5 + index].revents;
            if events == 0 {
                continue;
            }
            let client = &mut clients[index];
            let mut remove = false;
            if let Some(response) = &client.response {
                match client.stream.write(&response[client.sent..]) {
                    Ok(0) => remove = true,
                    Ok(n) => {
                        client.sent += n;
                        remove = client.sent == response.len();
                    }
                    Err(e) => remove = e.kind() != io::ErrorKind::WouldBlock,
                }
            } else {
                let mut data = [0u8; 4096];
                match client.stream.read(&mut data) {
                    Ok(0) => remove = true,
                    Ok(n) => {
                        client.request.extend_from_slice(&data[..n]);
                        if client.request.len() > 16 * 1024 {
                            remove = true;
                        } else if client.request.ends_with(b"\0\0") {
                            let fields: Vec<_> = client.request[..client.request.len() - 2]
                                .split(|b| *b == 0)
                                .collect();
                            let response = match fields.as_slice() {
                                [b"Q"] => serde_json::to_vec(capture.records())
                                    .unwrap_or_else(|_| b"[]".to_vec()),
                                _ => vec![b'-'],
                            };
                            if response.len() <= SNAPSHOT_LIMIT {
                                client.response = Some(response);
                            } else {
                                remove = true;
                            }
                        }
                    }
                    Err(e) => remove = e.kind() != io::ErrorKind::WouldBlock,
                }
            }
            if remove {
                clients.swap_remove(index);
            }
        }
        if poll[3].revents & libc::POLLIN != 0 {
            while clients.len() < 4 {
                match listener.accept() {
                    Ok((stream, _)) => clients.push(Client::new(stream)?),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

pub fn fallback(arguments: &[String], reason: &str) -> Result<i32, String> {
    eprintln!("amber-shell: Output capture is unavailable ({reason}); starting a normal shell.");
    let arguments = if arguments.first().map(String::as_str) == Some("--") {
        &arguments[1..]
    } else {
        arguments
    };
    let (program, args) = arguments
        .split_first()
        .map_or(("/bin/zsh", &[][..]), |(p, a)| (p.as_str(), a));
    let mut command = Command::new(program);
    if args.is_empty() {
        command.arg("-i");
    } else {
        command.args(args);
    }
    command
        .env("AMBER_SHELL_CAPTURE", "0")
        .env_remove("AMBER_SHELL_CAPTURE_SOCKET")
        .env_remove("AMBER_SHELL_CAPTURE_OWNER")
        .env_remove("AMBER_SHELL_CAPTURE_TOKEN");
    Err(command.exec().to_string())
}
