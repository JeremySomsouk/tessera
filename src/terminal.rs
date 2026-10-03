use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    term::{Config, TermMode},
    vte::ansi::{Color, Processor},
};
use anyhow::{Result, bail};
use eframe::egui::{Color32, Context};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    time::Duration,
};
use uuid::Uuid;
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
enum Command {
    Input(Vec<u8>),
    Resize(Size),
    Stop,
}
#[derive(Clone)]
pub struct Listener {
    replies: Arc<Mutex<Vec<Vec<u8>>>>,
    ctx: Context,
}
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(s) => {
                if let Ok(mut replies) = self.replies.lock() {
                    replies.push(s.into_bytes());
                }
            }
            Event::ColorRequest(index, format) => {
                let c = color(Color::Indexed(index.min(255) as u8));
                if let Ok(mut replies) = self.replies.lock() {
                    replies.push(
                        format(alacritty_terminal::vte::ansi::Rgb {
                            r: c.r(),
                            g: c.g(),
                            b: c.b(),
                        })
                        .into_bytes(),
                    );
                }
            }
            _ => {} // OSC 52 clipboard access is denied, including reads.
        }
        self.ctx.request_repaint();
    }
}
struct ProcessLifecycle {
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    exited: bool,
}
pub struct Terminal {
    pub term: Arc<Mutex<Term<Listener>>>,
    tx: SyncSender<Command>,
    pub alive: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    pub size: Size,
    mode: Arc<AtomicU32>,
    stopping: Arc<AtomicBool>,
    lifecycle: Arc<Mutex<ProcessLifecycle>>,
    host: Option<std::thread::JoinHandle<()>>,
}
impl Terminal {
    pub fn spawn(id: Uuid, directory: &Path, socket: &Path, ctx: Context) -> Result<Self> {
        let size = Size {
            cols: 100,
            rows: 30,
        };
        let pair = native_pty_system().openpty(size.pty())?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/sh".into()
            }
        });
        let mut command = CommandBuilder::new(shell);
        command.arg("-l");
        command.cwd(directory);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TESSERA_PANE", id.to_string());
        command.env("TESSERA_SOCKET", socket);
        let mut child = pair.slave.spawn_command(command)?;
        let lifecycle = Arc::new(Mutex::new(ProcessLifecycle {
            killer: child.clone_killer(),
            exited: false,
        }));
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let (tx, rx) = sync_channel(256);
        let replies = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener {
            replies: replies.clone(),
            ctx: ctx.clone(),
        };
        let term = Arc::new(Mutex::new(Term::new(
            Config {
                scrolling_history: 10_000,
                ..Default::default()
            },
            &size,
            listener,
        )));
        let mode = Arc::new(AtomicU32::new(term.lock().unwrap().mode().bits()));
        let parser_mode = mode.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let error = Arc::new(Mutex::new(None));
        let response_tx = tx.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let host_stopping = stopping.clone();
        let parser_term = term.clone();
        let read_alive = alive.clone();
        let read_ctx = ctx.clone();
        let read_error = error.clone();
        std::thread::spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut t) = parser_term.lock() {
                            parser.advance(&mut *t, &buf[..n]);
                            parser_mode.store(t.mode().bits(), Ordering::Release);
                        }
                        let pending = std::mem::take(&mut *replies.lock().unwrap());
                        for bytes in pending {
                            if response_tx.send(Command::Input(bytes)).is_err() {
                                break;
                            }
                        }
                        read_ctx.request_repaint();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        if let Ok(mut err) = read_error.lock() {
                            *err = Some(e.to_string());
                        }
                        break;
                    }
                }
            }
            read_alive.store(false, Ordering::Release);
            read_ctx.request_repaint();
        });
        let host_term = term.clone();
        let host_alive = alive.clone();
        let host_error = error.clone();
        let host_lifecycle = lifecycle.clone();
        let host = std::thread::spawn(move || {
            loop {
                if host_stopping.load(Ordering::Acquire) {
                    let mut life = host_lifecycle.lock().unwrap();
                    let _ = child.kill();
                    let _ = child.wait();
                    life.exited = true;
                    break;
                }
                let result = match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(Command::Input(bytes)) => writer.write_all(&bytes),
                    Ok(Command::Resize(s)) => {
                        if let Ok(mut t) = host_term.lock() {
                            t.resize(s);
                        }
                        pair.master.resize(s.pty()).map_err(std::io::Error::other)
                    }
                    Ok(Command::Stop) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        let mut life = host_lifecycle.lock().unwrap();
                        let _ = child.kill();
                        let _ = child.wait();
                        life.exited = true;
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
                    if matches!(child.try_wait(), Ok(Some(_))) {
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
            host_alive.store(false, Ordering::Release);
            ctx.request_repaint();
        });
        Ok(Self {
            term,
            tx,
            alive,
            error,
            size,
            mode,
            stopping,
            lifecycle,
            host: Some(host),
        })
    }
    pub fn mode(&self) -> TermMode {
        TermMode::from_bits_retain(self.mode.load(Ordering::Acquire))
    }
    pub fn input(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > 64 * 1024 {
            bail!("terminal input exceeds 64 KiB; split the paste into smaller parts");
        }
        if !self.alive.load(Ordering::Acquire) {
            bail!("shell has exited");
        }
        self.tx
            .try_send(Command::Input(bytes))
            .map_err(|_| anyhow::anyhow!("terminal input queue full; retry input"))
    }
    pub fn resize(&mut self, size: Size) {
        if (self.size.cols != size.cols || self.size.rows != size.rows)
            && self.tx.try_send(Command::Resize(size)).is_ok()
        {
            self.size = size;
        }
    }
    pub fn paste(&self, s: &str) -> Result<()> {
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        let clean = s.replace('\u{1b}', "");
        self.input(if bracketed {
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else {
            clean.replace('\n', "\r").into_bytes()
        })
    }
    pub fn scroll(&self, lines: i32) {
        if let Ok(mut t) = self.term.try_lock() {
            t.scroll_display(Scroll::Delta(lines));
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Ok(mut life) = self.lifecycle.lock()
            && !life.exited
        {
            let _ = life.killer.kill();
        }
        let _ = self.tx.try_send(Command::Stop);
        if let Some(host) = self.host.take() {
            let _ = host.join();
        }
    }
}
pub fn color(c: Color) -> Color32 {
    let index = match c {
        Color::Spec(rgb) => return Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => i as usize,
        Color::Named(n) => n as usize,
    };
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
    } else if index == 257 {
        Color32::from_rgb(24, 27, 34)
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
    fn palette_truecolor_and_cube() {
        assert_eq!(color(Color::Indexed(196)), Color32::RED);
    }
}

#[cfg(test)]
mod pty_tests {
    use super::*;
    #[test]
    fn reaped_shell_is_not_signalled_again_when_pane_closes() {
        let terminal = Terminal::spawn(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            Path::new("/tmp/unused-test.sock"),
            Context::default(),
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
        drop(terminal);
    }
    #[test]
    fn real_shell_input_resize_and_protocol_response() {
        let ctx = Context::default();
        let directory = std::env::current_dir().unwrap();
        let mut terminal = Terminal::spawn(
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
