use crate::selection::{self, SelectionAction};
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line},
    term::{Config, TermMode, cell::Flags},
    vte::ansi::{Color, CursorShape, CursorStyle, NamedColor, Processor, Rgb},
};
use anyhow::{Context as _, Result, bail};
use eframe::egui::{Color32, Context};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    os::fd::{AsRawFd, BorrowedFd},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
#[cfg(target_os = "macos")]
fn process_directory(pid: u32) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    // SAFETY: the writable buffer has exactly the size required by this flavor.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size as libc::c_int,
        )
    };
    if read != size as libc::c_int {
        bail!(
            "cannot read shell working directory: {}",
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: proc_pidinfo returned the complete initialized structure.
    let info = unsafe { info.assume_init() };
    let bytes: Vec<u8> = info
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    if bytes.is_empty() {
        bail!("shell working directory is unavailable");
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}
#[cfg(target_os = "linux")]
fn process_directory(pid: u32) -> Result<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).context("cannot read shell working directory")
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_directory(_pid: u32) -> Result<PathBuf> {
    bail!("reading the shell working directory is unsupported on this platform")
}

#[derive(Clone, Copy)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}
impl Size {
    fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows as u16,
            cols: self.cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}
fn terminal_config() -> Config {
    Config {
        scrolling_history: 10_000,
        default_cursor_style: CursorStyle {
            shape: CursorShape::Beam,
            blinking: false,
        },
        vi_mode_cursor_style: Some(CursorStyle {
            shape: CursorShape::HollowBlock,
            blinking: false,
        }),
        ..Default::default()
    }
}
pub const SNAPSHOT_MAX_BYTES: usize = 256 * 1024;
pub const SNAPSHOT_MAX_LINES: usize = 2_000;

/// Rendered text only: persisted output must never pass through the ANSI parser.
fn snapshot_grid<T: EventListener>(term: &Term<T>) -> String {
    let grid = term.grid();
    let first = -(grid.history_size() as i32);
    let last = grid.screen_lines() as i32;
    let mut lines = Vec::new();
    let mut bytes = 0;
    for row in (first..last).rev() {
        let mut line = String::new();
        for column in 0..grid.columns() {
            let cell = &grid[Line(row)][Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            if cell.flags.contains(Flags::HIDDEN) {
                line.push(' ');
                continue;
            }
            if !cell.c.is_control() {
                line.push(cell.c);
            }
            if let Some(marks) = cell.zerowidth() {
                line.extend(marks.iter().copied().filter(|c| !c.is_control()));
            }
        }
        line.truncate(line.trim_end().len());
        if lines.is_empty() && line.is_empty() {
            continue;
        }
        let separator = usize::from(!lines.is_empty());
        let available = SNAPSHOT_MAX_BYTES.saturating_sub(bytes + separator);
        if line.len() > available {
            let mut start = line.len() - available;
            while !line.is_char_boundary(start) {
                start += 1;
            }
            line.drain(..start);
        }
        bytes += line.len() + separator;
        lines.push(line);
        if bytes >= SNAPSHOT_MAX_BYTES || lines.len() >= SNAPSHOT_MAX_LINES {
            break;
        }
    }
    lines.reverse();
    lines.join("\n")
}

fn request_repaint(ctx: &Context) {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr_for(viewport);
    let request = ctx.data_mut(|data| {
        let key = eframe::egui::Id::new(("terminal repaint", viewport));
        if data.get_temp::<u64>(key) == Some(pass) {
            false
        } else {
            data.insert_temp(key, pass);
            true
        }
    });
    // egui retains one diagnostic cause per request until the next rendered pass.
    if request {
        ctx.request_repaint_of(viewport);
    }
}

const INPUT_BUFFER_BYTES: usize = 64 * 1024 * 1024;
const WRITE_CHUNK_BYTES: usize = 16 * 1024;
const PROTOCOL_BUFFER_BYTES: usize = 1024 * 1024;

struct InputBudget {
    used: AtomicUsize,
    limit: usize,
}

struct Input {
    bytes: Vec<u8>,
    paste: Option<bool>,
    follow_cursor: bool,
    budget: Arc<InputBudget>,
}

impl Drop for Input {
    fn drop(&mut self) {
        self.budget
            .used
            .fetch_sub(self.bytes.capacity(), Ordering::AcqRel);
    }
}

#[derive(Clone)]
struct InputSender {
    tx: SyncSender<Input>,
    budget: Arc<InputBudget>,
}

impl InputSender {
    fn send(&self, bytes: Vec<u8>, paste: Option<bool>, follow_cursor: bool) -> Result<()> {
        let size = bytes.capacity();
        self.budget.used.try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(size).filter(|total| *total <= self.budget.limit)
        }).map_err(|_| anyhow::anyhow!(
            "terminal input buffer full ({} bytes maximum); nothing from this input was sent, retry after pending input drains",
            self.budget.limit
        ))?;
        self.tx
            .try_send(Input {
                bytes,
                paste,
                follow_cursor,
                budget: self.budget.clone(),
            })
            .map_err(|_| {
                anyhow::anyhow!(
                    "terminal input queue unavailable; nothing from this input was sent"
                )
            })
    }
}

fn next_input<T>(input: &Receiver<T>, replies: &Receiver<T>, prefer_reply: &mut bool) -> Option<T> {
    let (first, second) = if *prefer_reply {
        (replies, input)
    } else {
        (input, replies)
    };
    match first.try_recv() {
        Ok(input) => {
            *prefer_reply = !*prefer_reply;
            Some(input)
        }
        Err(_) => second.try_recv().ok(),
    }
}

struct PendingInput {
    input: Input,
    offset: usize,
    buffer: Vec<u8>,
    buffer_offset: usize,
    prefix: bool,
    suffix: bool,
}

impl PendingInput {
    fn new(input: Input) -> Self {
        let buffer = if input.paste.is_some() {
            Vec::with_capacity(WRITE_CHUNK_BYTES + 12)
        } else {
            Vec::new()
        };
        Self {
            input,
            offset: 0,
            buffer,
            buffer_offset: 0,
            prefix: false,
            suffix: false,
        }
    }

    fn complete(&self) -> bool {
        self.offset == self.input.bytes.len()
            && self.buffer_offset == self.buffer.len()
            && (self.input.paste != Some(true) || self.suffix)
    }

    fn write(&mut self, writer: &mut dyn Write) -> std::io::Result<usize> {
        let Some(bracketed) = self.input.paste else {
            let end = (self.offset + WRITE_CHUNK_BYTES).min(self.input.bytes.len());
            let n = writer.write(&self.input.bytes[self.offset..end])?;
            self.offset += n;
            return Ok(n);
        };
        if self.buffer_offset == self.buffer.len() {
            self.buffer.clear();
            self.buffer_offset = 0;
            if bracketed && !self.prefix {
                self.buffer.extend_from_slice(b"\x1b[200~");
                self.prefix = true;
            }
            let end = (self.offset + WRITE_CHUNK_BYTES).min(self.input.bytes.len());
            while self.offset < end && self.buffer.len() < WRITE_CHUNK_BYTES {
                let byte = self.input.bytes[self.offset];
                self.offset += 1;
                if byte != 0x1b {
                    self.buffer.push(if byte == b'\n' && !bracketed {
                        b'\r'
                    } else {
                        byte
                    });
                }
            }
            if bracketed && self.offset == self.input.bytes.len() && !self.suffix {
                self.buffer.extend_from_slice(b"\x1b[201~");
                self.suffix = true;
            }
        }
        if self.buffer.is_empty() {
            return Ok(0);
        }
        let n = writer.write(&self.buffer[self.buffer_offset..])?;
        self.buffer_offset += n;
        Ok(n)
    }
}

enum Command {
    Wake,
    Resize(Size),
    Selection(SelectionAction),
    Scroll(i32),
    Stop,
}
#[derive(Clone)]
pub struct Listener {
    replies: Arc<Mutex<Vec<Event>>>,
}
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if matches!(event, Event::PtyWrite(_) | Event::ColorRequest(_, _))
            && let Ok(mut replies) = self.replies.lock()
        {
            replies.push(event);
        }
        // OSC 52 clipboard access is denied, including reads.
    }
}
fn protocol_reply(term: &Term<Listener>, event: Event) -> Option<Vec<u8>> {
    match event {
        Event::PtyWrite(text) => Some(text.into_bytes()),
        Event::ColorRequest(index, format) if index <= NamedColor::DimForeground as usize => {
            let fallback = color_index(index);
            let rgb = term.colors()[index].unwrap_or(Rgb {
                r: fallback.r(),
                g: fallback.g(),
                b: fallback.b(),
            });
            Some(format(rgb).into_bytes())
        }
        _ => None,
    }
}

struct ProcessLifecycle {
    exited: bool,
    exit_code: Option<u32>,
}

struct RemoteCache {
    path: PathBuf,
}

impl RemoteCache {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("tessera-remote-cache-{}", Uuid::new_v4()));
        let mut directory = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        directory
            .create(&path)
            .context("Cannot create a private cache for remote logs")?;
        Ok(Self { path })
    }
}

impl Drop for RemoteCache {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const CLAUDE_LAUNCH: &str = "if ! command -v claude >/dev/null 2>&1; then printf '%s\\n' 'Claude Code was not found in your login shell PATH. Make sure the claude command runs in your usual terminal, then retry.' >&2; exit 127; fi; exec claude";

fn agent_script(directory: &Path, launch: &str) -> Result<String> {
    let directory = std::path::absolute(directory)?;
    let directory = directory
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Working directory is not valid UTF-8"))?
        .replace('\'', "'\\''");
    // Reset login-file cwd changes using the same physical resolution as the PTY.
    Ok(format!(
        "cd -P -- '{directory}' || {{ printf '%s\\n' 'Cannot enter the selected folder. It may have moved or become inaccessible. Choose the folder again.' >&2; exit 1; }}; {launch}"
    ))
}

pub(crate) fn agent_shell() -> String {
    let preferred = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/bin/zsh".into()
        } else {
            "/bin/sh".into()
        }
    });
    if matches!(
        Path::new(&preferred)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("sh" | "bash" | "zsh")
    ) {
        preferred
    } else {
        "/bin/sh".into()
    }
}

fn agent_command(directory: &Path, launch: &str) -> Result<CommandBuilder> {
    let mut command = CommandBuilder::new(agent_shell());
    command.args(["-lic", &agent_script(directory, launch)?]);
    Ok(command)
}

pub struct Terminal {
    pub term: Arc<Mutex<Term<Listener>>>,
    tx: SyncSender<Command>,
    input_tx: InputSender,
    #[cfg(test)]
    output_bytes: Arc<AtomicU64>,
    #[cfg(test)]
    input_bytes: Arc<AtomicU64>,
    reader: Option<std::thread::JoinHandle<()>>,
    pub alive: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    pub size: Size,
    pub copy_mode: bool,
    ctx: Context,
    mode: Arc<AtomicU32>,
    pub revision: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    lifecycle: Arc<Mutex<ProcessLifecycle>>,
    host: Option<std::thread::JoinHandle<()>>,
    shell_pid: Option<u32>,
    launch_directory: PathBuf,
}
impl Terminal {
    pub fn try_snapshot_output(&self) -> Option<String> {
        self.term.try_lock().ok().map(|term| snapshot_grid(&term))
    }

    pub fn snapshot_output(&self) -> String {
        self.term
            .lock()
            .map(|term| snapshot_grid(&term))
            .unwrap_or_default()
    }

    #[cfg(not(test))]
    pub fn spawn(id: Uuid, directory: &Path, socket: &Path, ctx: Context) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/sh".into()
            }
        });
        let mut command = CommandBuilder::new(shell);
        command.arg("-l");
        Self::spawn_command(id, directory, socket, ctx, command)
    }

    pub fn spawn_agent(
        id: Uuid,
        directory: &Path,
        socket: &Path,
        ctx: Context,
        launch: &str,
    ) -> Result<Self> {
        let command = agent_command(directory, &format!("exec {launch}"))?;
        Self::spawn_command(id, directory, socket, ctx, command)
    }

    pub fn spawn_remote(
        id: Uuid,
        directory: &Path,
        socket: &Path,
        ctx: Context,
        launch: &str,
    ) -> Result<Self> {
        let cache = RemoteCache::new()?;
        let cache_path = cache
            .path
            .to_str()
            .context("Remote cache path is not valid UTF-8")?
            .replace('\'', "'\\''");
        let mut command = agent_command(
            directory,
            &format!("export HISTFILE=/dev/null XDG_CACHE_HOME='{cache_path}'; {launch}"),
        )?;
        command.env("HISTFILE", "/dev/null");
        command.env("XDG_CACHE_HOME", &cache.path);
        Self::spawn_command_with_cache(id, directory, socket, ctx, command, Some(cache))
    }

    pub fn spawn_claude(id: Uuid, directory: &Path, socket: &Path, ctx: Context) -> Result<Self> {
        let command = agent_command(directory, CLAUDE_LAUNCH)?;
        let metadata = std::fs::metadata(directory).with_context(|| {
            format!(
                "Cannot open {}. The folder may have moved or become inaccessible; select an existing folder again",
                directory.display()
            )
        })?;
        anyhow::ensure!(
            metadata.is_dir(),
            "{} is not a folder; select an existing folder",
            directory.display()
        );
        Self::spawn_command(id, directory, socket, ctx, command)
    }

    #[cfg(test)]
    pub(crate) fn spawn_test(
        id: Uuid,
        directory: &Path,
        socket: &Path,
        ctx: Context,
    ) -> Result<Self> {
        // Run an interactive shell without login files or a line editor. In
        // particular, Readline in a C locale can reinterpret UTF-8 path bytes.
        // Wait for explicit readiness instead of racing shell initialization.
        let mut command = CommandBuilder::new("/bin/bash");
        command.args(["--noprofile", "--norc", "--noediting", "-i"]);
        command.env_clear();
        command.env("PATH", "/usr/bin:/bin");
        command.env("HOME", std::env::temp_dir());
        command.env("HISTFILE", "/dev/null");
        command.env("LC_ALL", "C");
        command.env("PS1", "TESSERA_TEST_READY> ");
        command.env("PS2", "");
        let terminal = Self::spawn_command(id, directory, socket, ctx, command)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let output: String = terminal
                .term
                .lock()
                .unwrap()
                .grid()
                .display_iter()
                .map(|cell| cell.cell.c)
                .collect();
            if output.contains("TESSERA_TEST_READY> ") {
                return Ok(terminal);
            }
            if !terminal.alive.load(Ordering::Acquire) || std::time::Instant::now() >= deadline {
                bail!(
                    "test shell did not become ready; output: {output:?}; PTY error: {:?}",
                    terminal.error.lock().unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn_command(
        id: Uuid,
        directory: &Path,
        socket: &Path,
        ctx: Context,
        command: CommandBuilder,
    ) -> Result<Self> {
        Self::spawn_command_with_cache(id, directory, socket, ctx, command, None)
    }

    fn spawn_command_with_cache(
        id: Uuid,
        directory: &Path,
        socket: &Path,
        ctx: Context,
        command: CommandBuilder,
        cache: Option<RemoteCache>,
    ) -> Result<Self> {
        let input_limit = match std::env::var("TESSERA_INPUT_BUFFER_BYTES") {
            Ok(value) => value
                .parse::<usize>()
                .ok()
                .filter(|limit| *limit > 0)
                .context("TESSERA_INPUT_BUFFER_BYTES must be a positive byte count")?,
            Err(_) => INPUT_BUFFER_BYTES,
        };
        Self::spawn_with_input_limit(id, directory, socket, ctx, command, cache, input_limit)
    }

    fn spawn_with_input_limit(
        id: Uuid,
        directory: &Path,
        socket: &Path,
        ctx: Context,
        mut command: CommandBuilder,
        cache: Option<RemoteCache>,
        input_limit: usize,
    ) -> Result<Self> {
        let size = Size {
            cols: 100,
            rows: 30,
        };
        let pair = native_pty_system().openpty(size.pty())?;
        let fd = pair
            .master
            .as_raw_fd()
            .context("PTY does not expose a native descriptor")?;
        // The duplicate owns its lifetime; nonblocking status is shared with the writer.
        let reader_fd = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?;
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        command.cwd(directory);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TESSERA_PANE", id.to_string());
        command.env("TESSERA_SOCKET", socket);
        let mut child = pair.slave.spawn_command(command)?;
        let shell_pid = child.process_id();
        let lifecycle = Arc::new(Mutex::new(ProcessLifecycle {
            exited: false,
            exit_code: None,
        }));
        drop(pair.slave);
        let mut reader = std::fs::File::from(reader_fd);
        let mut writer = pair.master.take_writer()?;
        let (tx, rx) = sync_channel(256);
        let (input_channel, input_rx) = sync_channel(256);
        let input_tx = InputSender {
            tx: input_channel,
            budget: Arc::new(InputBudget {
                used: AtomicUsize::new(0),
                limit: input_limit,
            }),
        };
        let output_bytes = Arc::new(AtomicU64::new(0));
        let input_bytes = Arc::new(AtomicU64::new(0));
        let replies = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener {
            replies: replies.clone(),
        };
        let term = Arc::new(Mutex::new(Term::new(terminal_config(), &size, listener)));
        let mode = Arc::new(AtomicU32::new(term.lock().unwrap().mode().bits()));
        let parser_mode = mode.clone();
        let revision = Arc::new(AtomicU64::new(0));
        let parser_revision = revision.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let error = Arc::new(Mutex::new(None));
        let (response_channel, response_rx) = sync_channel(256);
        let response_tx = InputSender {
            tx: response_channel,
            budget: Arc::new(InputBudget {
                used: AtomicUsize::new(0),
                limit: PROTOCOL_BUFFER_BYTES,
            }),
        };
        let response_wake = tx.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let host_stopping = stopping.clone();
        let parser_term = term.clone();
        let read_alive = alive.clone();
        let read_ctx = ctx.clone();
        let read_error = error.clone();
        let read_stopping = stopping.clone();
        let read_lifecycle = lifecycle.clone();
        let read_bytes = output_bytes.clone();
        let reader = std::thread::spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut buf = [0u8; 16 * 1024];
            loop {
                if read_stopping.load(Ordering::Acquire) {
                    break;
                }
                let result = reader.read(&mut buf);
                let eof = matches!(&result, Ok(0))
                    || matches!(&result, Err(error) if error.raw_os_error() == Some(libc::EIO));
                let sync_expired = parser
                    .sync_timeout()
                    .sync_timeout()
                    .is_some_and(|deadline| Instant::now() >= deadline);
                let n = match &result {
                    Ok(n) => *n,
                    Err(_) => 0,
                };
                if n > 0 || eof || sync_expired {
                    let pending = if let Ok(mut t) = parser_term.lock() {
                        if n > 0 {
                            parser.advance(&mut *t, &buf[..n]);
                            read_bytes.fetch_add(n as u64, Ordering::Relaxed);
                        }
                        if eof
                            || parser
                                .sync_timeout()
                                .sync_timeout()
                                .is_some_and(|deadline| Instant::now() >= deadline)
                        {
                            parser.stop_sync(&mut *t);
                        }
                        parser_mode.store(t.mode().bits(), Ordering::Release);
                        parser_revision.fetch_add(1, Ordering::Release);
                        let events = std::mem::take(&mut *replies.lock().unwrap());
                        events
                            .into_iter()
                            .filter_map(|event| protocol_reply(&t, event))
                            .flatten()
                            .collect::<Vec<_>>()
                    } else {
                        break;
                    };
                    if !pending.is_empty() {
                        if let Err(error) = response_tx.send(pending, None, false)
                            && !read_lifecycle.lock().unwrap().exited
                        {
                            *read_error.lock().unwrap() =
                                Some(format!("terminal protocol reply overflow: {error}"));
                            read_stopping.store(true, Ordering::Release);
                            let _ = response_wake.try_send(Command::Wake);
                            break;
                        }
                        let _ = response_wake.try_send(Command::Wake);
                    }
                    request_repaint(&read_ctx);
                }
                if eof {
                    break;
                }
                match result {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        let mut descriptor = libc::pollfd {
                            fd: reader.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        let timeout =
                            parser
                                .sync_timeout()
                                .sync_timeout()
                                .map_or(250, |deadline| {
                                    deadline
                                        .saturating_duration_since(Instant::now())
                                        .as_millis()
                                        .clamp(1, 250) as i32
                                });
                        // The file owns the descriptor. Idle readers wake at most four times/sec.
                        unsafe {
                            libc::poll(&mut descriptor, 1, timeout);
                        }
                    }
                    Err(error) => {
                        *read_error.lock().unwrap() = Some(error.to_string());
                        break;
                    }
                }
            }
            read_alive.store(false, Ordering::Release);
            request_repaint(&read_ctx);
        });
        let host_term = term.clone();
        let host_revision = revision.clone();
        let host_mode = mode.clone();
        let host_bytes = input_bytes.clone();
        let host_budget = input_tx.budget.clone();
        let host_error = error.clone();
        let host_lifecycle = lifecycle.clone();
        let selection_ctx = ctx.clone();
        let host = std::thread::spawn(move || {
            let mut pending = None;
            let mut writing: Option<PendingInput> = None;
            let mut prefer_reply = true;
            loop {
                if host_stopping.load(Ordering::Acquire) {
                    break;
                }
                let mut wrote = false;
                if writing.is_none()
                    && let Some(input) = next_input(&input_rx, &response_rx, &mut prefer_reply)
                {
                    if input.follow_cursor
                        && let Ok(mut term) = host_term.lock()
                    {
                        term.scroll_display(Scroll::Bottom);
                        request_repaint(&ctx);
                    }
                    writing = Some(PendingInput::new(input));
                }
                if let Some(input) = writing.as_mut() {
                    let source_offset = input.offset;
                    match input.write(&mut *writer) {
                        Ok(n) => {
                            host_bytes.fetch_add(n as u64, Ordering::Relaxed);
                            wrote = n > 0 || input.offset > source_offset;
                            if input.complete() {
                                writing = None;
                            } else if !wrote {
                                *host_error.lock().unwrap() = Some("terminal input write made no progress; pending input cancelled".into());
                                host_stopping.store(true, Ordering::Release);
                                continue;
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(error) => {
                            *host_error.lock().unwrap() = Some(format!(
                                "terminal input failed; pending input cancelled: {error}"
                            ));
                            host_stopping.store(true, Ordering::Release);
                            continue;
                        }
                    }
                }
                if !wrote && writing.is_some() {
                    let mut descriptor = libc::pollfd {
                        fd,
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    // The master owns fd. Use a short readiness wait between control checks.
                    unsafe {
                        libc::poll(&mut descriptor, 1, 2);
                    }
                }
                let wait = if wrote || writing.is_some() {
                    Duration::ZERO
                } else {
                    Duration::from_millis(250)
                };
                let command = pending
                    .take()
                    .map(Ok)
                    .unwrap_or_else(|| rx.recv_timeout(wait));
                let result = match command {
                    Ok(Command::Wake) => Ok(()),
                    Ok(Command::Selection(action)) => {
                        let text = host_term
                            .lock()
                            .map_err(|_| std::io::Error::other("terminal state unavailable"))
                            .map(|mut term| {
                                let text = selection::apply(&mut *term, action);
                                host_mode.store(term.mode().bits(), Ordering::Release);
                                text
                            });
                        match text {
                            Ok(text) => {
                                if let Some(text) = text {
                                    ctx.copy_text(text);
                                }
                                request_repaint(&ctx);
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Ok(Command::Scroll(lines)) => host_term
                        .lock()
                        .map_err(|_| std::io::Error::other("terminal state unavailable"))
                        .map(|mut term| {
                            term.scroll_display(Scroll::Delta(lines));
                            request_repaint(&ctx);
                        }),
                    Ok(Command::Resize(mut s)) => {
                        while let Ok(command) = rx.try_recv() {
                            match command {
                                Command::Resize(size) => s = size,
                                command => {
                                    pending = Some(command);
                                    break;
                                }
                            }
                        }
                        host_term
                            .lock()
                            .map_err(|_| std::io::Error::other("terminal state unavailable"))
                            .and_then(|mut term| {
                                term.resize(s);
                                let result =
                                    pair.master.resize(s.pty()).map_err(std::io::Error::other);
                                host_revision.fetch_add(1, Ordering::Release);
                                request_repaint(&ctx);
                                result
                            })
                    }
                    Ok(Command::Stop) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(()),
                };
                if let Err(e) = result
                    && let Ok(mut err) = host_error.lock()
                {
                    *err = Some(e.to_string());
                }
                let exited = {
                    let mut life = host_lifecycle.lock().unwrap();
                    if let Ok(Some(status)) = child.try_wait() {
                        life.exit_code = Some(status.exit_code());
                        life.exited = true;
                        true
                    } else {
                        false
                    }
                };
                if exited {
                    break;
                }
            }
            let reap = !host_lifecycle.lock().unwrap().exited;
            if reap {
                host_stopping.store(true, Ordering::Release);
            }
            // macOS may wait for pending PTY output during child exit. Close host handles first.
            drop(writer);
            drop(pair.master);
            if reap {
                let _ = child.kill();
                let status = child.wait();
                let mut life = host_lifecycle.lock().unwrap();
                life.exit_code = status.ok().map(|status| status.exit_code());
                life.exited = true;
            }
            drop(cache);
            let undelivered = host_budget.used.load(Ordering::Acquire);
            if undelivered > 0 && !host_stopping.load(Ordering::Acquire) {
                *host_error.lock().unwrap() = Some(format!(
                    "shell exited with {undelivered} bytes of pending input storage; delivery may be incomplete"
                ));
            }
            request_repaint(&ctx);
        });
        Ok(Self {
            term,
            tx,
            input_tx,
            #[cfg(test)]
            output_bytes,
            #[cfg(test)]
            input_bytes,
            reader: Some(reader),
            alive,
            error,
            size,
            copy_mode: false,
            ctx: selection_ctx,
            mode,
            revision,
            stopping,
            lifecycle,
            host: Some(host),
            shell_pid,
            launch_directory: directory.to_path_buf(),
        })
    }
    pub fn has_exited(&self) -> bool {
        // PTY EOF/read failures alone do not prove that the shell stopped.
        !self.alive.load(Ordering::Acquire)
            && self.lifecycle.try_lock().is_ok_and(|life| life.exited)
    }
    pub fn current_directory(&self) -> Result<PathBuf> {
        let lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal process state unavailable"))?;
        if !self.alive.load(Ordering::Acquire)
            || self.stopping.load(Ordering::Acquire)
            || lifecycle.exited
        {
            return Ok(self.launch_directory.clone());
        }
        let pid = self
            .shell_pid
            .ok_or_else(|| anyhow::anyhow!("shell process ID unavailable"))?;
        let directory = process_directory(pid);
        drop(lifecycle);
        directory
    }
    pub fn shell_pid(&self) -> Option<u32> {
        self.shell_pid
    }
    pub fn selection_action(&mut self, action: SelectionAction) -> Result<()> {
        if self.alive.load(Ordering::Acquire) {
            self.tx.try_send(Command::Selection(action)).map_err(|_| {
                anyhow::anyhow!("terminal control queue unavailable; retry selection")
            })?;
        } else {
            // Retained output remains selectable after the shell/control worker exits.
            let mut term = self
                .term
                .try_lock()
                .map_err(|_| anyhow::anyhow!("terminal state busy; retry selection"))?;
            if let Some(text) = selection::apply(&mut *term, action) {
                self.ctx.copy_text(text);
            }
            self.mode.store(term.mode().bits(), Ordering::Release);
            request_repaint(&self.ctx);
        }
        match action {
            SelectionAction::Enter => self.copy_mode = true,
            SelectionAction::Exit | SelectionAction::Copy { exit: true } => self.copy_mode = false,
            _ => {}
        }
        Ok(())
    }
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    pub fn mode(&self) -> TermMode {
        TermMode::from_bits_retain(self.mode.load(Ordering::Acquire))
    }
    pub fn input(&self, bytes: Vec<u8>) -> Result<()> {
        self.send_input(bytes, false)
    }
    pub fn input_at_cursor(&self, bytes: Vec<u8>) -> Result<()> {
        self.send_input(bytes, true)
    }
    fn send_input(&self, bytes: Vec<u8>, follow_cursor: bool) -> Result<()> {
        self.enqueue_input(bytes, None, follow_cursor)
    }
    fn enqueue_input(
        &self,
        bytes: Vec<u8>,
        paste: Option<bool>,
        follow_cursor: bool,
    ) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
            bail!("shell has exited");
        }
        self.input_tx.send(bytes, paste, follow_cursor)?;
        let _ = self.tx.try_send(Command::Wake);
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn output_bytes(&self) -> u64 {
        self.output_bytes.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    pub(crate) fn input_bytes(&self) -> u64 {
        self.input_bytes.load(Ordering::Relaxed)
    }
    pub fn resize(&mut self, size: Size) {
        if (self.size.cols != size.cols || self.size.rows != size.rows)
            && self.tx.try_send(Command::Resize(size)).is_ok()
        {
            self.size = size;
        }
    }
    pub fn exit_code(&self) -> Option<u32> {
        self.lifecycle.lock().ok().and_then(|life| life.exit_code)
    }
    pub fn paste(&self, s: &str) -> Result<()> {
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        if s.len()
            > self
                .input_tx
                .budget
                .limit
                .saturating_sub(self.input_tx.budget.used.load(Ordering::Acquire))
        {
            bail!(
                "terminal input buffer full ({} bytes maximum); nothing from this paste was sent",
                self.input_tx.budget.limit
            );
        }
        self.enqueue_input(s.as_bytes().to_vec(), Some(bracketed), true)
    }
    pub fn scroll(&self, lines: i32) -> Result<()> {
        if self.alive.load(Ordering::Acquire) {
            self.tx.try_send(Command::Scroll(lines)).map_err(|_| {
                anyhow::anyhow!("terminal control queue unavailable; retry scrolling")
            })?;
        } else {
            let mut term = self
                .term
                .try_lock()
                .map_err(|_| anyhow::anyhow!("terminal state busy; retry scrolling"))?;
            term.scroll_display(Scroll::Delta(lines));
            request_repaint(&self.ctx);
        }
        Ok(())
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.tx.try_send(Command::Stop);
        if let Some(host) = self.host.take() {
            let _ = host.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
pub fn color(c: Color) -> Color32 {
    let index = match c {
        Color::Spec(rgb) => return Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => i as usize,
        Color::Named(n) => n as usize,
    };
    color_index(index)
}

fn color_index(index: usize) -> Color32 {
    const PALETTE: [[u8; 3]; 16] = [
        [28, 31, 38],
        [231, 115, 131],
        [132, 194, 155],
        [231, 194, 124],
        [132, 170, 227],
        [186, 154, 220],
        [120, 197, 205],
        [216, 221, 230],
        [92, 101, 117],
        [249, 147, 162],
        [162, 217, 181],
        [246, 216, 156],
        [160, 193, 246],
        [213, 180, 244],
        [151, 224, 229],
        [245, 246, 250],
    ];
    if index < 16 {
        let [r, g, b] = PALETTE[index];
        Color32::from_rgb(r, g, b)
    } else if index < 232 {
        let i = index - 16;
        let level = |n| if n == 0 { 0 } else { 55 + n * 40 };
        Color32::from_rgb(
            level(i / 36) as u8,
            level((i / 6) % 6) as u8,
            level(i % 6) as u8,
        )
    } else if index < 256 {
        Color32::from_gray((8 + (index - 232) * 10) as u8)
    } else if index == NamedColor::Background as usize {
        Color32::from_rgb(24, 27, 34)
    } else if index == NamedColor::Cursor as usize {
        Color32::from_rgb(130, 198, 180)
    } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&index) {
        color_index(index - NamedColor::DimBlack as usize).gamma_multiply(0.65)
    } else if index == NamedColor::BrightForeground as usize {
        color_index(NamedColor::BrightWhite as usize)
    } else if index == NamedColor::DimForeground as usize {
        color_index(NamedColor::Foreground as usize).gamma_multiply(0.65)
    } else {
        Color32::from_rgb(216, 221, 230)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::{
        event::VoidListener,
        index::{Column, Line, Point},
    };
    #[test]
    fn repaint_requests_are_bounded_without_frames_and_promote_delayed_wakes() {
        let ctx = Context::default();
        for _ in 0..3 {
            let _ = ctx.run(Default::default(), |_| {});
        }
        let immediate = Arc::new(AtomicUsize::new(0));
        let observed = immediate.clone();
        ctx.set_request_repaint_callback(move |info| {
            if info.delay == Duration::ZERO {
                observed.fetch_add(1, Ordering::Relaxed);
            }
        });
        ctx.request_repaint_after(Duration::from_secs(5));
        for _ in 0..100_000 {
            request_repaint(&ctx);
        }
        assert_eq!(immediate.load(Ordering::Relaxed), 1);
        let _ = ctx.run(Default::default(), |ctx| {
            assert!(
                ctx.repaint_causes().len() <= 2,
                "hidden terminal accumulated repaint causes"
            );
        });
        for _ in 0..100_000 {
            request_repaint(&ctx);
        }
        let _ = ctx.run(Default::default(), |ctx| {
            assert!(ctx.repaint_causes().len() <= 1);
        });
    }

    #[test]
    fn continuously_available_replies_cannot_starve_user_input() {
        let (input_tx, input_rx) = sync_channel(1);
        let (reply_tx, reply_rx) = sync_channel(2);
        let mut prefer_reply = true;
        reply_tx.send("reply").unwrap();
        reply_tx.send("reply").unwrap();
        for _ in 0..1_000 {
            input_tx.send("user").unwrap();
            assert_eq!(
                next_input(&input_rx, &reply_rx, &mut prefer_reply),
                Some("reply")
            );
            reply_tx.send("reply").unwrap();
            assert_eq!(
                next_input(&input_rx, &reply_rx, &mut prefer_reply),
                Some("user")
            );
        }
        assert_eq!(
            next_input(&input_rx, &reply_rx, &mut prefer_reply),
            Some("reply")
        );
        assert_eq!(
            next_input(&input_rx, &reply_rx, &mut prefer_reply),
            Some("reply")
        );
        input_tx.send("user").unwrap();
        assert_eq!(
            next_input(&input_rx, &reply_rx, &mut prefer_reply),
            Some("user")
        );
        assert!(next_input(&input_rx, &reply_rx, &mut prefer_reply).is_none());
    }

    #[test]
    fn input_admission_is_atomic_bounded_and_recovers_after_consumption() {
        let (tx, rx) = sync_channel(1);
        let sender = InputSender {
            tx,
            budget: Arc::new(InputBudget {
                used: AtomicUsize::new(0),
                limit: 16,
            }),
        };
        sender.send(vec![1; 16], None, false).unwrap();
        assert!(sender.send(vec![2; 1], None, false).is_err());
        assert_eq!(sender.budget.used.load(Ordering::Acquire), 16);
        let first = rx.recv().unwrap();
        assert_eq!(first.bytes, [1; 16]);
        assert!(rx.try_recv().is_err());
        drop(first);
        sender.send(vec![3; 8], None, false).unwrap();
        assert!(sender.send(vec![4; 8], None, false).is_err());
        assert_eq!(sender.budget.used.load(Ordering::Acquire), 8);
        drop(rx.recv().unwrap());
        assert_eq!(sender.budget.used.load(Ordering::Acquire), 0);
        drop(rx);
        assert!(sender.send(vec![5; 8], None, false).is_err());
        assert_eq!(sender.budget.used.load(Ordering::Acquire), 0);
    }

    #[test]
    fn filtered_paste_yields_after_a_bounded_source_chunk() {
        let (tx, rx) = sync_channel(1);
        let sender = InputSender {
            tx,
            budget: Arc::new(InputBudget {
                used: AtomicUsize::new(0),
                limit: 4 * WRITE_CHUNK_BYTES,
            }),
        };
        sender
            .send(vec![0x1b; 4 * WRITE_CHUNK_BYTES], Some(false), false)
            .unwrap();
        let mut pending = PendingInput::new(rx.recv().unwrap());
        let mut output = Vec::new();
        for i in 1..=4 {
            assert_eq!(pending.write(&mut output).unwrap(), 0);
            assert_eq!(pending.offset, i * WRITE_CHUNK_BYTES);
            assert_eq!(pending.complete(), i == 4);
        }
        assert!(output.is_empty());
    }

    #[test]
    fn paste_stream_preserves_utf8_framing_and_partial_write_order() {
        struct SlowWriter {
            bytes: Vec<u8>,
            calls: usize,
        }
        impl Write for SlowWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                if self.calls.is_multiple_of(3) {
                    return Err(std::io::ErrorKind::WouldBlock.into());
                }
                let n = bytes.len().min(7);
                self.bytes.extend_from_slice(&bytes[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let text = "界é\n\x1b[201~".repeat(8_000);
        for bracketed in [false, true] {
            let (tx, rx) = sync_channel(1);
            let sender = InputSender {
                tx,
                budget: Arc::new(InputBudget {
                    used: AtomicUsize::new(0),
                    limit: text.len(),
                }),
            };
            sender
                .send(text.as_bytes().to_vec(), Some(bracketed), true)
                .unwrap();
            let mut pending = PendingInput::new(rx.recv().unwrap());
            let mut writer = SlowWriter {
                bytes: Vec::new(),
                calls: 0,
            };
            while !pending.complete() {
                if let Err(error) = pending.write(&mut writer) {
                    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
                }
            }
            let clean = text.replace('\x1b', "");
            let expected = if bracketed {
                format!("\x1b[200~{clean}\x1b[201~")
            } else {
                clean.replace('\n', "\r")
            };
            assert_eq!(writer.bytes, expected.as_bytes());
            drop(pending);
            assert_eq!(sender.budget.used.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn snapshot_is_rendered_text_without_control_sequences_or_hidden_content() {
        let mut term = Term::new(terminal_config(), &Size { cols: 40, rows: 3 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(
            &mut term,
            "old\rnew\x1b[31m 界e\u{301}\x1b[0m\x1b[8msecret\x1b[0m\x1b[6n\x1b]52;c;ZXZpbA==\x07"
                .as_bytes(),
        );
        let text = snapshot_grid(&term);
        assert_eq!(text, "new 界e\u{301}");
        assert!(!text.contains('\x1b'));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn snapshot_includes_history_regardless_of_scroll_position_and_bounds_lines() {
        let mut term = Term::new(terminal_config(), &Size { cols: 40, rows: 3 }, VoidListener);
        let mut parser: Processor = Processor::new();
        for index in 0..3_000 {
            parser.advance(&mut term, format!("line {index}\r\n").as_bytes());
        }
        let text = snapshot_grid(&term);
        assert_eq!(text.lines().count(), SNAPSHOT_MAX_LINES);
        assert!(text.starts_with("line 1000\n"));
        assert!(text.ends_with("line 2999"));
        term.scroll_display(Scroll::Top);
        assert_eq!(text, snapshot_grid(&term));
    }

    #[test]
    fn snapshot_bounds_utf8_bytes_and_retains_latest_output() {
        let mut term = Term::new(
            terminal_config(),
            &Size { cols: 500, rows: 3 },
            VoidListener,
        );
        let mut parser: Processor = Processor::new();
        for _ in 0..1_000 {
            parser.advance(&mut term, format!("{}\r\n", "界".repeat(240)).as_bytes());
        }
        parser.advance(&mut term, b"latest");
        let text = snapshot_grid(&term);
        assert!(text.len() <= SNAPSHOT_MAX_BYTES);
        assert!(text.len() > SNAPSHOT_MAX_BYTES - 4);
        assert!(text.ends_with("latest"));
    }

    #[test]
    fn parser_handles_alt_screen_utf8_and_resize() {
        let mut t = Term::new(Config::default(), &Size { cols: 20, rows: 5 }, VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut t, "hello 界".as_bytes());
        assert_eq!(t.grid()[Point::new(Line(0), Column(6))].c, '界');
        p.advance(&mut t, b"\x1b[?1049hother\x1b[?1049l");
        assert_eq!(t.grid()[Point::new(Line(0), Column(0))].c, 'h');
        t.resize(Size { cols: 30, rows: 8 });
        assert_eq!(t.columns(), 30);
    }
    #[test]
    fn typing_uses_a_beam_and_applications_can_change_or_reset_the_cursor() {
        let mut term = Term::new(terminal_config(), &Size { cols: 20, rows: 5 }, VoidListener);
        let mut parser: Processor = Processor::new();
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Beam);
        for (sequence, shape) in [
            (b"\x1b[2 q".as_slice(), CursorShape::Block),
            (b"\x1b[4 q".as_slice(), CursorShape::Underline),
            (b"\x1b[6 q".as_slice(), CursorShape::Beam),
            (b"\x1b[0 q".as_slice(), CursorShape::Beam),
            (b"\x1b[?25l".as_slice(), CursorShape::Hidden),
            (b"\x1b[?25h".as_slice(), CursorShape::Beam),
        ] {
            parser.advance(&mut term, sequence);
            assert_eq!(term.renderable_content().cursor.shape, shape);
        }
        selection::apply(&mut term, SelectionAction::Enter);
        assert_eq!(
            term.renderable_content().cursor.shape,
            CursorShape::HollowBlock
        );
        selection::apply(&mut term, SelectionAction::Exit);
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Beam);
    }
    #[test]
    fn vim_cursor_shapes_survive_fragmented_updates_wide_text_and_resize() {
        let mut term = Term::new(terminal_config(), &Size { cols: 20, rows: 5 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(
            &mut term,
            "\x1b[?1049h\x1b[2 q界\u{301}\x1b[1;2H".as_bytes(),
        );
        assert_eq!(
            term.renderable_content().cursor.point,
            Point::new(Line(0), Column(0))
        );
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Block);
        parser.advance(&mut term, b"\x1b[6");
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Block);
        parser.advance(&mut term, b" q");
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Beam);
        for (sequence, shape) in [
            (b"\x1b[1 q".as_slice(), CursorShape::Block),
            (b"\x1b[3 q".as_slice(), CursorShape::Underline),
            (b"\x1b[5 q".as_slice(), CursorShape::Beam),
        ] {
            parser.advance(&mut term, sequence);
            term.resize(Size { cols: 12, rows: 3 });
            assert_eq!(term.renderable_content().cursor.shape, shape);
        }
        parser.advance(&mut term, b"\x1b[?25l");
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Hidden);
        parser.advance(&mut term, b"\x1b[?25h\x1b[2 q");
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Block);
        parser.advance(&mut term, b"\x1b[?1049l\x1b[0 q");
        assert_eq!(term.renderable_content().cursor.shape, CursorShape::Beam);
    }
    #[test]
    fn terminal_color_queries_report_dark_background_and_current_dynamic_colors() {
        let replies = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener {
            replies: replies.clone(),
        };
        let mut term = Term::new(terminal_config(), &Size { cols: 20, rows: 5 }, listener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"\x1b]10;?\x07\x1b]11;?\x07");
        let responses: Vec<_> = std::mem::take(&mut *replies.lock().unwrap())
            .into_iter()
            .filter_map(|event| protocol_reply(&term, event))
            .collect();
        assert_eq!(
            responses,
            [
                b"\x1b]10;rgb:d8d8/dddd/e6e6\x07".to_vec(),
                b"\x1b]11;rgb:1818/1b1b/2222\x07".to_vec()
            ]
        );
        parser.advance(&mut term, b"\x1b]11;#123456\x07\x1b]11;?\x1b\\");
        let responses: Vec<_> = std::mem::take(&mut *replies.lock().unwrap())
            .into_iter()
            .filter_map(|event| protocol_reply(&term, event))
            .collect();
        assert_eq!(responses, [b"\x1b]11;rgb:1212/3434/5656\x1b\\".to_vec()]);
    }
    #[test]
    fn palette_truecolor_and_cube() {
        assert_eq!(color(Color::Indexed(196)), Color32::RED);
    }
}

#[cfg(test)]
mod pty_tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    #[ignore = "bounded terminal transport benchmark"]
    fn transport_benchmark() {
        use std::time::Instant;
        for bytes in [32 * 1024, 256 * 1024, 4 * 1024 * 1024] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("received");
            let mut command = CommandBuilder::new("/bin/sh");
            command.args(["-c", "stty raw -echo; printf READY; exec cat > received"]);
            let start = Instant::now();
            let terminal = Terminal::spawn_command(
                Uuid::new_v4(),
                root.path(),
                Path::new(""),
                Context::default(),
                command,
            )
            .unwrap();
            let deadline = start + Duration::from_secs(10);
            while !terminal.snapshot_output().contains("READY") {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            let startup = start.elapsed();
            let payload = "a界\n".repeat(bytes / 5);
            let expected = payload.replace('\n', "\r");
            let start = Instant::now();
            let result = terminal.paste(&payload);
            let submit = start.elapsed();
            if result.is_ok() {
                while !std::fs::metadata(&path).is_ok_and(|m| m.len() == expected.len() as u64) {
                    assert!(Instant::now() < deadline, "paste timed out");
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
            }
            eprintln!(
                "transport bytes={} startup_ms={:.3} submit_ms={:.3} completion_ms={:.3} result={:?}",
                payload.len(),
                startup.as_secs_f64() * 1000.0,
                submit.as_secs_f64() * 1000.0,
                start.elapsed().as_secs_f64() * 1000.0,
                result
            );
        }
        let root = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args([
            "-c",
            "stty raw -echo; printf READY; sleep 1; exec cat > received",
        ]);
        let mut terminal = Terminal::spawn_command(
            Uuid::new_v4(),
            root.path(),
            Path::new(""),
            Context::default(),
            command,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !terminal.snapshot_output().contains("READY") {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        terminal.input(vec![b'x'; 60 * 1024]).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let start = Instant::now();
        terminal.resize(Size {
            cols: 101,
            rows: 31,
        });
        while terminal.term.lock().unwrap().columns() != 101 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        eprintln!(
            "transport slow_consumer_resize_ms={:.3}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }

    fn capture_terminal(root: &Path, prefix: &str) -> Terminal {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args([
            "-c",
            &format!("stty raw -echo; printf 'READY'; {prefix} exec cat > received"),
        ]);
        let terminal = Terminal::spawn_command(
            Uuid::new_v4(),
            root,
            Path::new(""),
            Context::default(),
            command,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !terminal.snapshot_output().contains("READY") {
            assert!(
                Instant::now() < deadline,
                "capture process did not become ready"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        terminal
    }

    #[test]
    fn large_paste_arrives_exactly_once_before_following_input() {
        for bracketed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let terminal = capture_terminal(root.path(), "");
            if bracketed {
                // Freeze the application mode at admission, as a real bracketed-paste app does.
                terminal
                    .mode
                    .store(TermMode::BRACKETED_PASTE.bits(), Ordering::Release);
            }
            let text = "界é\n\x1b[201~".repeat(128 * 1024);
            let clean = text.replace('\x1b', "");
            let mut expected = if bracketed {
                format!("\x1b[200~{clean}\x1b[201~")
            } else {
                clean.replace('\n', "\r")
            };
            expected.push_str("TRAILER");
            terminal.paste(&text).unwrap();
            terminal.input(b"TRAILER".to_vec()).unwrap();
            let path = root.path().join("received");
            let deadline = Instant::now() + Duration::from_secs(10);
            while !std::fs::metadata(&path).is_ok_and(|m| m.len() == expected.len() as u64) {
                assert!(
                    Instant::now() < deadline,
                    "large paste did not finish: {:?}",
                    terminal.error.lock().unwrap()
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(std::fs::read(path).unwrap(), expected.as_bytes());
            assert_eq!(terminal.input_bytes(), expected.len() as u64);
            assert_eq!(terminal.output_bytes(), 5);
            assert_eq!(terminal.input_tx.budget.used.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn slow_input_consumer_does_not_block_resize_or_cancelled_teardown() {
        for _ in 0..3 {
            let root = tempfile::tempdir().unwrap();
            let mut terminal = capture_terminal(root.path(), "sleep 10;");
            let alive = terminal.alive.clone();
            let budget = terminal.input_tx.budget.clone();
            terminal.paste(&"x".repeat(2 * 1024 * 1024)).unwrap();
            let start = Instant::now();
            terminal.resize(Size {
                cols: 101,
                rows: 31,
            });
            while terminal.term.lock().unwrap().columns() != 101 {
                assert!(
                    start.elapsed() < Duration::from_millis(500),
                    "blocked writer delayed resize"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(budget.used.load(Ordering::Acquire) > 0);
            let start = Instant::now();
            drop(terminal);
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "cancelled write blocked teardown"
            );
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(budget.used.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn protocol_replies_have_reserved_capacity_while_user_input_is_full() {
        let root = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "stty raw -echo; printf READY; while [ ! -f go ]; do sleep 0.01; done; printf '\\033[6n'; sleep 0.1; exec cat > received"]);
        let terminal = Terminal::spawn_with_input_limit(
            Uuid::new_v4(),
            root.path(),
            Path::new(""),
            Context::default(),
            command,
            None,
            64 * 1024,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while terminal.output_bytes() < 5 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        terminal.input(vec![b'x'; 64 * 1024]).unwrap();
        assert_eq!(
            terminal.input_tx.budget.used.load(Ordering::Acquire),
            64 * 1024
        );
        assert!(terminal.input(vec![b'y']).is_err());
        std::fs::write(root.path().join("go"), "").unwrap();
        let mut expected = vec![b'x'; 64 * 1024];
        expected.extend_from_slice(b"\x1b[1;6R");
        while !std::fs::metadata(root.path().join("received"))
            .is_ok_and(|m| m.len() == expected.len() as u64)
        {
            assert!(
                terminal.error.lock().unwrap().is_none(),
                "reply failed while user queue full"
            );
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            std::fs::read(root.path().join("received")).unwrap(),
            expected
        );
        terminal.input(b"recovered".to_vec()).unwrap();
    }

    #[test]
    fn cancelling_backlogged_output_closes_pty_before_reaping() {
        for _ in 0..8 {
            let root = tempfile::tempdir().unwrap();
            let mut command = CommandBuilder::new("/bin/sh");
            command.args(["-c", "stty raw -echo; exec awk 'BEGIN { for (i=0; i<500000; i++) print \"pending-output\" }'"]);
            let terminal = Terminal::spawn_command(
                Uuid::new_v4(),
                root.path(),
                Path::new(""),
                Context::default(),
                command,
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while terminal.output_bytes() < 16 * 1024 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            let (done, completed) = std::sync::mpsc::channel();
            let teardown = std::thread::spawn(move || {
                drop(terminal);
                let _ = done.send(());
            });
            completed
                .recv_timeout(Duration::from_secs(2))
                .expect("PTY handles must close before reaping a child with pending output");
            teardown.join().unwrap();
        }
    }

    #[test]
    fn nonreading_protocol_consumer_fails_explicitly_without_deadlock() {
        let root = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "stty raw -echo; exec awk 'BEGIN { for (i=0; i<400000; i++) printf \"\\033[6n\"; fflush(); system(\"sleep 5\") }'"]);
        let terminal = Terminal::spawn_command(
            Uuid::new_v4(),
            root.path(),
            Path::new(""),
            Context::default(),
            command,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !terminal.has_exited() {
            assert!(
                Instant::now() < deadline,
                "protocol backlog deadlocked PTY workers"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            terminal
                .error
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|error| error.contains("protocol reply overflow"))
        );
        let start = Instant::now();
        drop(terminal);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn synchronized_output_flushes_on_timeout_and_process_exit() {
        for ending in ["sleep 10", "exit 0"] {
            let root = tempfile::tempdir().unwrap();
            let mut command = CommandBuilder::new("/bin/sh");
            command.args([
                "-c",
                &format!("printf '\\033[?2026hSYNC_VISIBLE'; {ending}"),
            ]);
            let terminal = Terminal::spawn_command(
                Uuid::new_v4(),
                root.path(),
                Path::new(""),
                Context::default(),
                command,
            )
            .unwrap();
            let start = Instant::now();
            while !terminal.snapshot_output().contains("SYNC_VISIBLE") {
                assert!(
                    start.elapsed() < Duration::from_secs(3),
                    "synchronized output stayed buffered after timeout or EOF"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    fn claude_fixture(root: &Path) -> PathBuf {
        let bin = root.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join("claude");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$#\" \"$TESSERA_PANE\" \"$TESSERA_SOCKET\" \"$TERM\" \"$COLORTERM\" > \"$TESSERA_TEST_OUTPUT\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        bin
    }

    #[test]
    fn claude_launch_preserves_literal_directory_and_hook_environment_without_arguments() {
        let root = tempfile::tempdir().unwrap();
        let bin = claude_fixture(root.path());
        let nested = root.path().join("project ' α $(touch INJECTED) ;");
        std::fs::create_dir(&nested).unwrap();
        let selected = root.path().join("selected ' α $(touch INJECTED) ; symlink");
        std::os::unix::fs::symlink(&nested, &selected).unwrap();
        let output = root.path().join("environment");
        let socket = root.path().join("hooks.sock");
        let id = Uuid::new_v4();
        let mut command = CommandBuilder::new("/bin/bash");
        command.args([
            "--noprofile",
            "--norc",
            "-ic",
            &format!("cd /; {}", agent_script(&selected, CLAUDE_LAUNCH).unwrap()),
        ]);
        command.env_clear();
        command.env("PATH", bin);
        command.env("HOME", root.path());
        command.env("TESSERA_TEST_OUTPUT", &output);
        let terminal =
            Terminal::spawn_command(id, &selected, &socket, Context::default(), command).unwrap();
        let expected = format!(
            "{}\n0\n{id}\n{}\nxterm-256color\ntruecolor\n",
            selected.canonicalize().unwrap().display(),
            socket.display()
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !terminal.has_exited() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(terminal.exit_code(), Some(0));
        assert_eq!(terminal.launch_directory, selected);
        assert_eq!(std::fs::read_to_string(output).unwrap(), expected);
        assert!(!root.path().join("INJECTED").exists());
        assert!(!nested.join("INJECTED").exists());
    }

    #[test]
    fn agent_launch_resolves_symlink_parent_paths_like_filesystem_validation() {
        let root = tempfile::tempdir().unwrap();
        let physical = root.path().join("target");
        std::fs::create_dir_all(physical.join("nested")).unwrap();
        std::fs::create_dir(physical.join("destination")).unwrap();
        std::fs::create_dir(root.path().join("destination")).unwrap();
        let link = root.path().join("selected symlink");
        std::os::unix::fs::symlink(physical.join("nested"), &link).unwrap();
        let selected = link.join("../destination");
        let expected = selected.canonicalize().unwrap();
        assert_eq!(
            expected,
            physical.join("destination").canonicalize().unwrap()
        );
        let script = agent_script(&selected, "exec /bin/pwd -P").unwrap();
        let result = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .current_dir(&selected)
            .env_clear()
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(
            String::from_utf8(result.stdout).unwrap().trim(),
            expected.to_str().unwrap()
        );
    }

    #[test]
    fn direct_claude_launch_rejects_non_utf8_paths_before_spawning() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join(std::ffi::OsString::from_vec(vec![0xff]));
        let result = Terminal::spawn_claude(
            Uuid::new_v4(),
            &selected,
            &root.path().join("hooks.sock"),
            Context::default(),
        );
        assert_eq!(
            result.err().unwrap().to_string(),
            "Working directory is not valid UTF-8"
        );
    }

    #[test]
    fn claude_launch_reports_missing_executable_and_disappeared_directory() {
        let root = tempfile::tempdir().unwrap();
        let script = agent_script(root.path(), CLAUDE_LAUNCH).unwrap();
        let result = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .env_clear()
            .env("PATH", root.path())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(127));
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("Claude Code was not found"));
        assert!(error.contains("usual terminal, then retry"));

        let missing = root.path().join("deleted folder");
        let script = agent_script(&missing, CLAUDE_LAUNCH).unwrap();
        let result = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .env_clear()
            .env("PATH", root.path())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("Cannot enter the selected folder")
        );
        assert!(!missing.exists());
    }

    #[test]
    fn direct_claude_launch_rejects_files_and_missing_folders_without_creating_them() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "keep").unwrap();
        let missing = root.path().join("missing/nested");
        for directory in [&file, &missing] {
            let result = Terminal::spawn_claude(
                Uuid::new_v4(),
                directory,
                &root.path().join("hooks.sock"),
                Context::default(),
            );
            assert!(result.is_err());
        }
        assert!(!missing.parent().unwrap().exists());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
    }

    #[test]
    fn agent_launch_uses_real_pty_directory_and_hook_environment() {
        let directory = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4();
        let terminal = super::Terminal::spawn_agent(
            id,
            directory.path(),
            &directory.path().join("hooks.sock"),
            eframe::egui::Context::default(),
            "printf 'READY:%s:%s' \"$TESSERA_PANE\" \"$PWD\"",
        )
        .unwrap();
        let expected = format!("READY:{id}:");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let output: String = terminal
                .term
                .lock()
                .unwrap()
                .grid()
                .display_iter()
                .map(|cell| cell.cell.c)
                .collect();
            if output.contains(&expected) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Agent launch output missing: {output}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn remote_launch_runs_grouped_pipeline_with_private_history_and_hook_environment() {
        let directory = tempfile::tempdir().unwrap();
        let directory = directory.path().canonicalize().unwrap();
        let id = Uuid::new_v4();
        let socket = directory.join("remote.sock");
        let terminal = Terminal::spawn_remote(
            id,
            &directory,
            &socket,
            Context::default(),
            "{ printf 'REMOTE_READY:%s:%s\\n' \"$TESSERA_PANE\" \"$HISTFILE\"; printf 'SOCKET:%s\\n' \"$TESSERA_SOCKET\"; printf 'DIRECTORY:%s\\n' \"$PWD\"; } | cat; exit 7",
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !terminal.has_exited() || terminal.alive.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "remote launch did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = terminal.snapshot_output();
        assert!(
            output.contains(&format!("REMOTE_READY:{id}:/dev/null")),
            "{output}"
        );
        assert!(
            output.contains(&format!("SOCKET:{}", socket.display())),
            "{output}"
        );
        assert!(
            output.contains(&format!("DIRECTORY:{}", directory.display())),
            "{output}"
        );
        assert_eq!(terminal.exit_code(), Some(7));
    }

    fn remote_with_cached_log() -> (tempfile::TempDir, Terminal, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let terminal = Terminal::spawn_remote(
            Uuid::new_v4(),
            directory.path(),
            &directory.path().join("remote.sock"),
            Context::default(),
            "printf '%s' \"${XDG_CACHE_HOME-}\" > cache-path; printf '%s' \"${GH_CONFIG_DIR-}\" > gh-config; printf '%s' \"${XDG_CONFIG_HOME-}\" > xdg-config; printf 'CACHE_WAITING\\n'; read action; mkdir -p \"$XDG_CACHE_HOME/gh\"; printf 'PRIVATE_CACHED_LOG' > \"$XDG_CACHE_HOME/gh/log.zip\"; printf 'CACHE_READY\\n'; read action",
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !terminal.snapshot_output().contains("CACHE_WAITING") {
            assert!(
                std::time::Instant::now() < deadline,
                "remote launch did not become ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let cache = std::fs::read_to_string(directory.path().join("cache-path")).unwrap();
        assert!(
            !cache.is_empty(),
            "remote logs need a private cache directory"
        );
        let cache = PathBuf::from(cache);
        assert!(cache.starts_with(std::env::temp_dir()));
        assert_ne!(
            Some(cache.clone()),
            std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for (file, variable) in [
            ("gh-config", "GH_CONFIG_DIR"),
            ("xdg-config", "XDG_CONFIG_HOME"),
        ] {
            assert_eq!(
                std::fs::read_to_string(directory.path().join(file)).unwrap(),
                std::env::var(variable).unwrap_or_default(),
                "remote launch changed credential configuration",
            );
        }
        terminal.input(b"continue\n".to_vec()).unwrap();
        while !terminal.snapshot_output().contains("CACHE_READY") {
            assert!(
                std::time::Instant::now() < deadline,
                "remote cache was not written"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(cache.join("gh/log.zip")).unwrap(),
            "PRIVATE_CACHED_LOG"
        );
        (directory, terminal, cache)
    }

    #[test]
    fn remote_cache_is_removed_after_process_exit_while_terminal_is_retained() {
        let (_directory, terminal, cache) = remote_with_cached_log();
        terminal.input(b"exit\n".to_vec()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !terminal.has_exited() || cache.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "exited remote process retained cached logs"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(terminal.snapshot_output().contains("CACHE_READY"));
    }

    #[test]
    fn remote_cache_is_removed_when_a_running_terminal_is_closed() {
        let (_directory, terminal, cache) = remote_with_cached_log();
        let (_second_directory, second_terminal, second_cache) = remote_with_cached_log();
        assert_ne!(cache, second_cache);
        assert!(!terminal.has_exited());
        drop(terminal);
        assert!(
            !cache.exists(),
            "closed remote terminal retained cached logs"
        );
        assert!(
            second_cache.exists(),
            "closing a pane removed another pane's cache"
        );
        drop(second_terminal);
        assert!(!second_cache.exists());
    }

    #[test]
    fn remote_cache_is_removed_when_process_launch_fails() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new().unwrap();
        let cache_path = cache.path.clone();
        let command = CommandBuilder::new(directory.path().join("missing-command"));
        let result = Terminal::spawn_command_with_cache(
            Uuid::new_v4(),
            directory.path(),
            &directory.path().join("remote.sock"),
            Context::default(),
            command,
            Some(cache),
        );
        assert!(result.is_err());
        assert!(
            !cache_path.exists(),
            "failed launch retained its private cache"
        );
    }

    use super::*;
    #[test]
    fn fixture_shell_accepts_immediate_input_and_preserves_pane_environment() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested folder α");
        std::fs::create_dir(&nested).unwrap();
        let nested = nested.canonicalize().unwrap();
        let output = directory.path().join("environment");
        let socket = directory.path().join("test.sock");
        let id = Uuid::new_v4();
        let terminal =
            Terminal::spawn_test(id, directory.path(), &socket, Context::default()).unwrap();
        let quote = |path: &Path| path.to_string_lossy().replace('\'', "'\\''");
        terminal
            .input(
                format!(
                    "cd -- '{}'; printf '%s\\n' \"$TESSERA_PANE\" \"$TESSERA_SOCKET\" \"$TERM\" \"$COLORTERM\" \"$PWD\" > '{}'\r",
                    quote(&nested),
                    quote(&output),
                )
                .into_bytes(),
            )
            .unwrap();
        let expected = format!(
            "{id}\n{}\nxterm-256color\ntruecolor\n{}\n",
            socket.display(),
            nested.display(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&output).is_ok_and(|text| text == expected) {
            assert!(
                std::time::Instant::now() < deadline,
                "first command did not execute with the expected pane environment"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(terminal.current_directory().unwrap(), nested);
    }

    #[test]
    fn reaped_shell_is_not_signalled_again_when_pane_closes() {
        let ctx = Context::default();
        let mut terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            Path::new("/tmp/unused-test.sock"),
            ctx.clone(),
        )
        .unwrap();
        terminal.input(b"exit\r".to_vec()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if terminal.lifecycle.lock().unwrap().exited {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            terminal.lifecycle.lock().unwrap().exited,
            "shell was not reaped"
        );
        terminal.host.take().unwrap().join().unwrap();
        assert!(!terminal.alive.load(Ordering::Acquire));
        terminal.selection_action(SelectionAction::Enter).unwrap();
        terminal
            .selection_action(SelectionAction::Move {
                motion: alacritty_terminal::vi_mode::ViMotion::First,
                extend: false,
            })
            .unwrap();
        terminal.selection_action(SelectionAction::Toggle).unwrap();
        terminal
            .selection_action(SelectionAction::Move {
                motion: alacritty_terminal::vi_mode::ViMotion::Last,
                extend: false,
            })
            .unwrap();
        let selected = terminal.term.lock().unwrap().selection_to_string().unwrap();
        terminal
            .selection_action(SelectionAction::Copy { exit: true })
            .unwrap();
        let out = ctx.run(Default::default(), |_| {});
        assert!(
            out.platform_output.commands.iter().any(|command| {
                matches!(command, eframe::egui::OutputCommand::CopyText(text) if *text == selected)
            }),
            "retained output did not reach the clipboard command queue"
        );
        assert!(!terminal.copy_mode);
        let launch = tempfile::tempdir().unwrap();
        terminal.launch_directory = launch.path().to_path_buf();
        terminal.shell_pid = Some(std::process::id());
        terminal.alive.store(true, Ordering::Release);
        assert_eq!(terminal.current_directory().unwrap(), launch.path());
        drop(terminal);
    }
    #[test]
    fn scrollback_movement_is_queued_while_terminal_state_is_busy() {
        let terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            Path::new(""),
            Context::default(),
        )
        .unwrap();
        let mut term = terminal.term.lock().unwrap();
        let mut parser: Processor = Processor::new();
        parser.advance(&mut *term, &b"history\r\n".repeat(100));
        assert_eq!(term.grid().display_offset(), 0);
        terminal.scroll(3).unwrap();
        drop(term);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if terminal.term.lock().unwrap().grid().display_offset() >= 3 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "scroll was lost under lock contention"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn typing_and_paste_return_to_live_input_even_under_contention() {
        let terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            Path::new(""),
            Context::default(),
        )
        .unwrap();
        for paste in [false, true] {
            let mut term = terminal.term.lock().unwrap();
            let mut parser: Processor = Processor::new();
            parser.advance(&mut *term, &b"history\r\n".repeat(100));
            term.scroll_display(Scroll::Delta(10));
            assert!(term.grid().display_offset() > 0);
            if paste {
                terminal.paste(" ").unwrap();
            } else {
                terminal.input_at_cursor(b" ".to_vec()).unwrap();
            }
            drop(term);
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while terminal.term.lock().unwrap().grid().display_offset() != 0 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "input did not return to bottom"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn zsh_prompt_remains_single_after_split_resize() {
        let mut command = CommandBuilder::new("/bin/zsh");
        command.args(["-f", "-i"]);
        command.env_clear();
        command.env("PATH", "/usr/bin:/bin");
        command.env("HOME", std::env::temp_dir());
        command.env("PS1", "%F{magenta}TESSERA_ZSH_PROMPT%f > ");
        command.env("RPROMPT", "%F{cyan}<aws:tooling-sso>%f");
        let mut terminal = Terminal::spawn_command(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            Path::new(""),
            Context::default(),
            command,
        )
        .unwrap();
        let output = || {
            terminal
                .term
                .lock()
                .unwrap()
                .grid()
                .display_iter()
                .map(|cell| cell.cell.c)
                .collect::<String>()
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !{
            let text = output();
            text.contains("TESSERA_ZSH_PROMPT") && text.contains("<aws:tooling-sso>")
        } {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        for _ in 0..100 {
            terminal.resize(Size { cols: 50, rows: 30 });
            terminal.resize(Size {
                cols: 100,
                rows: 30,
            });
        }
        std::thread::sleep(Duration::from_millis(200));
        let term = terminal.term.lock().unwrap();
        assert_eq!(term.columns(), 100);
        assert_eq!(term.screen_lines(), 30);
        let text = term
            .grid()
            .display_iter()
            .map(|cell| cell.cell.c)
            .collect::<String>();
        assert_eq!(text.matches("TESSERA_ZSH_PROMPT").count(), 1);
    }

    #[test]
    fn real_shell_input_resize_and_protocol_response() {
        let ctx = Context::default();
        let directory = std::env::current_dir().unwrap();
        let mut terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            &directory,
            Path::new("/tmp/unused-tessera-test.sock"),
            ctx,
        )
        .unwrap();
        terminal.resize(Size { cols: 83, rows: 17 });
        terminal
            .input(b"printf '\\033[6n'; printf 'TESSERA_PTY_OK\\n'; stty size\n".to_vec())
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            if let Ok(t) = terminal.term.lock() {
                let text: String = t.grid().display_iter().map(|c| c.cell.c).collect();
                if text.contains("17 83") && text.contains("TESSERA_PTY_OK") {
                    found = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            found,
            "shell did not respond with output and resized dimensions"
        );
    }
}
