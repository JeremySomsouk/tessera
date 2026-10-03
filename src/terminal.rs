use crate::selection::{self, SelectionAction};
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    term::{Config, TermMode},
    vte::ansi::{Color, CursorShape, CursorStyle, NamedColor, Processor, Rgb},
};
use anyhow::{Result, bail};
use eframe::egui::{Color32, Context};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
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
enum Command {
    Input(Vec<u8>),
    UserInput(Vec<u8>),
    Resize(Size),
    Selection(SelectionAction),
    Scroll(i32),
    Stop,
}
#[derive(Clone)]
pub struct Listener {
    replies: Arc<Mutex<Vec<Event>>>,
    ctx: Context,
}
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if matches!(event, Event::PtyWrite(_) | Event::ColorRequest(_, _))
            && let Ok(mut replies) = self.replies.lock()
        {
            replies.push(event);
        }
        // OSC 52 clipboard access is denied, including reads.
        self.ctx.request_repaint();
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
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    exited: bool,
}
pub struct Terminal {
    pub term: Arc<Mutex<Term<Listener>>>,
    tx: SyncSender<Command>,
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
        let term = Arc::new(Mutex::new(Term::new(terminal_config(), &size, listener)));
        let mode = Arc::new(AtomicU32::new(term.lock().unwrap().mode().bits()));
        let parser_mode = mode.clone();
        let revision = Arc::new(AtomicU64::new(0));
        let parser_revision = revision.clone();
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
                        let pending = if let Ok(mut t) = parser_term.lock() {
                            parser.advance(&mut *t, &buf[..n]);
                            parser_mode.store(t.mode().bits(), Ordering::Release);
                            parser_revision.fetch_add(1, Ordering::Release);
                            let events = std::mem::take(&mut *replies.lock().unwrap());
                            events
                                .into_iter()
                                .filter_map(|event| protocol_reply(&t, event))
                                .collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        };
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
        let host_revision = revision.clone();
        let host_mode = mode.clone();
        let host_alive = alive.clone();
        let host_error = error.clone();
        let host_lifecycle = lifecycle.clone();
        let selection_ctx = ctx.clone();
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
                    Ok(Command::UserInput(bytes)) => host_term
                        .lock()
                        .map_err(|_| std::io::Error::other("terminal state unavailable"))
                        .and_then(|mut term| {
                            term.scroll_display(Scroll::Bottom);
                            ctx.request_repaint();
                            drop(term);
                            writer.write_all(&bytes)
                        }),
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
                                ctx.request_repaint();
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
                            ctx.request_repaint();
                        }),
                    Ok(Command::Resize(s)) => {
                        if let Ok(mut t) = host_term.lock() {
                            t.resize(s);
                            host_revision.fetch_add(1, Ordering::Release);
                            ctx.request_repaint();
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
            copy_mode: false,
            ctx: selection_ctx,
            mode,
            revision,
            stopping,
            lifecycle,
            host: Some(host),
        })
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
            self.ctx.request_repaint();
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
        if bytes.len() > 64 * 1024 {
            bail!("terminal input exceeds 64 KiB; split the paste into smaller parts");
        }
        if !self.alive.load(Ordering::Acquire) {
            bail!("shell has exited");
        }
        self.tx
            .try_send(if follow_cursor {
                Command::UserInput(bytes)
            } else {
                Command::Input(bytes)
            })
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
        self.input_at_cursor(if bracketed {
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else {
            clean.replace('\n', "\r").into_bytes()
        })
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
            self.ctx.request_repaint();
        }
        Ok(())
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
    fn terminal_color_queries_report_dark_background_and_current_dynamic_colors() {
        let replies = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener {
            replies: replies.clone(),
            ctx: Context::default(),
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
    use super::*;
    #[test]
    fn reaped_shell_is_not_signalled_again_when_pane_closes() {
        let ctx = Context::default();
        let mut terminal = Terminal::spawn(
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
        drop(terminal);
    }
    #[test]
    fn scrollback_movement_is_queued_while_terminal_state_is_busy() {
        let terminal = Terminal::spawn(
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
        let terminal = Terminal::spawn(
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
