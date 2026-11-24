use super::*;
use alacritty_terminal::grid::Dimensions;
use egui::{Event, FullOutput, PointerButton};
use std::path::Path;

struct Screen {
    ctx: egui::Context,
    size: Vec2,
    grid: Rect,
    cell: Vec2,
}

impl Screen {
    fn new() -> Self {
        Self {
            ctx: egui::Context::default(),
            size: Vec2::new(640.0, 360.0),
            grid: Rect::NOTHING,
            cell: Vec2::ZERO,
        }
    }

    fn frame(&mut self, term: &mut Terminal, events: Vec<Event>) -> FullOutput {
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    self.grid = ui.available_rect_before_wrap().shrink(8.0);
                    let font = FontId::monospace(15.0);
                    self.cell =
                        ui.fonts_mut(|f| Vec2::new(f.glyph_width(&font, 'M'), f.row_height(&font)));
                    terminal_view(ui, term, &mut TerminalSearch::default(), true, 15.0, true)
                        .1
                        .unwrap();
                });
            },
        )
    }

    fn cell_at(&self, col: usize, row: usize) -> Pos2 {
        self.grid.min
            + Vec2::new(
                (col as f32 + 0.25) * self.cell.x,
                (row as f32 + 0.5) * self.cell.y,
            )
    }
}

#[track_caller]
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "terminal did not reach the expected state"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        pressed,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn label(output: &FullOutput, label: &str) -> Option<Pos2> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.text() == label => {
            Some(text.pos + text.galley.rect.center().to_vec2())
        }
        _ => None,
    })
}

fn pane(ctx: &egui::Context, dir: &Path) -> Terminal {
    Terminal::spawn_test(Uuid::new_v4(), dir, Path::new(""), ctx.clone()).unwrap()
}

fn viewport_line(terminal: &Terminal, row: i32) -> String {
    let term = terminal.term.lock().unwrap();
    let line = Line(row - term.grid().display_offset() as i32);
    term.bounds_to_string(
        Point::new(line, Column(0)),
        Point::new(line, term.last_column()),
    )
}

#[test]
fn streaming_preserves_press_anchor_reading_position_and_pane_identity() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("feed"), "for i in {1..120}; do printf 'row %03d read me\\n' \"$i\"; done\nwhile [ ! -f advance ]; do sleep 0.02; done\nfor i in {121..300}; do printf 'row %03d read me\\n' \"$i\"; done\n").unwrap();
    let mut screen = Screen::new();
    let mut first = pane(&screen.ctx, dir.path());
    let mut other = pane(&screen.ctx, dir.path());
    screen.frame(&mut first, vec![]);
    first.input(b"bash feed\n".to_vec()).unwrap();
    wait(|| first.snapshot_output().contains("row 120 read me"));
    screen.frame(&mut first, vec![]);
    let top = viewport_line(&first, 0);
    let expected = viewport_line(&first, 2);
    let start = screen.cell_at(0, 2);
    let end = screen.cell_at(expected.len() - 1, 2);
    screen.frame(
        &mut first,
        vec![Event::PointerMoved(start), button(start, true)],
    );
    std::fs::write(dir.path().join("advance"), "").unwrap();
    wait(|| first.snapshot_output().contains("row 300 read me"));
    assert_eq!(viewport_line(&first, 0), top);
    screen.frame(&mut first, vec![Event::PointerMoved(end)]);
    screen.frame(&mut first, vec![button(end, false)]);
    assert_eq!(
        first.term.lock().unwrap().selection_to_string(),
        Some(expected.clone())
    );
    let mut output = screen.frame(&mut first, vec![Event::Copy]);
    wait(|| {
        if output
            .platform_output
            .commands
            .iter()
            .any(|cmd| matches!(cmd, egui::OutputCommand::CopyText(text) if *text == expected))
        {
            true
        } else {
            output = screen.frame(&mut first, vec![]);
            false
        }
    });
    let peek = label(&screen.frame(&mut first, vec![]), "Peek").unwrap();
    let bytes = first.input_bytes();
    screen.frame(
        &mut first,
        vec![Event::PointerMoved(peek), button(peek, true)],
    );
    screen.frame(&mut first, vec![button(peek, false)]);
    let preview = screen.frame(&mut first, vec![]);
    assert!(
        label(&preview, "row 300 read me").is_some(),
        "latest row visible in preview"
    );
    assert_eq!(viewport_line(&first, 0), top);
    assert_eq!(first.input_bytes(), bytes);
    assert_eq!(
        first.term.lock().unwrap().selection_to_string().as_deref(),
        Some(expected.as_str())
    );
    screen.frame(&mut first, vec![button(peek, true)]);
    screen.frame(&mut first, vec![button(peek, false)]);
    for _ in 0..6 {
        let other_frame = screen.frame(&mut other, vec![]);
        assert!(label(&other_frame, "Return to live").is_none());
        assert!(label(&screen.frame(&mut first, vec![]), "Return to live").is_some());
        assert_eq!(viewport_line(&first, 0), top);
    }
    screen.size.x -= 80.0;
    screen.frame(&mut first, vec![]);
    wait(|| first.term.lock().unwrap().columns() == first.size.cols);
    assert_eq!(viewport_line(&first, 0), top);
    let pos = label(&screen.frame(&mut first, vec![]), "Return to live").unwrap();
    screen.frame(
        &mut first,
        vec![Event::PointerMoved(pos), button(pos, true)],
    );
    screen.frame(&mut first, vec![button(pos, false)]);
    wait(|| first.term.lock().unwrap().grid().display_offset() == 0);
    assert!(label(&screen.frame(&mut first, vec![]), "Return to live").is_none());
}

#[test]
fn unbracketed_clipboard_waits_for_the_chosen_action_in_the_originating_pane() {
    let dir = tempfile::tempdir().unwrap();
    let mut screen = Screen::new();
    let mut other = pane(&screen.ctx, dir.path());
    let command = "printf first > marker\r\nprintf second >> marker\n";
    for action in ["Cancel", "Paste as one line", "Send line breaks"] {
        let mut terminal = pane(&screen.ctx, dir.path());
        screen.frame(&mut terminal, vec![]);
        let before = terminal.input_bytes();
        screen.frame(&mut terminal, vec![Event::Paste(command.into())]);
        assert_eq!(terminal.pending_paste.as_deref(), Some(command));
        assert!(label(&screen.frame(&mut other, vec![]), "Paste multiple lines?").is_none());
        screen.frame(&mut terminal, vec![]);
        let preview = screen.frame(&mut terminal, vec![]);
        assert!(label(&preview, "The clipboard ends with a line break.").is_some());
        let pos = label(&preview, action).unwrap();
        assert_eq!(terminal.input_bytes(), before);
        assert!(!dir.path().join("marker").exists());
        screen.frame(
            &mut terminal,
            vec![Event::PointerMoved(pos), button(pos, true)],
        );
        screen.frame(&mut terminal, vec![button(pos, false)]);
        assert!(terminal.pending_paste.is_none());
        match action {
            "Cancel" => assert_eq!(terminal.input_bytes(), before),
            "Paste as one line" => {
                wait(|| terminal.input_bytes() > before);
                assert!(!dir.path().join("marker").exists());
            }
            _ => {
                wait(|| std::fs::read(dir.path().join("marker")).is_ok_and(|b| b == b"firstsecond"))
            }
        }
        assert_eq!(other.input_bytes(), 0);
    }
}

#[test]
fn fullscreen_redraw_during_drag_keeps_original_text_until_return_to_live() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("screen"), "printf '\\033[?1049h\\033[2J\\033[Horiginal text'; while [ ! -f repaint ]; do sleep 0.02; done; printf '\\033[2J\\033[Hupdated text'; sleep 5").unwrap();
    let mut screen = Screen::new();
    let mut terminal = pane(&screen.ctx, dir.path());
    screen.frame(&mut terminal, vec![]);
    terminal.input(b"sh screen\n".to_vec()).unwrap();
    wait(|| {
        terminal.mode().contains(TermMode::ALT_SCREEN)
            && viewport_line(&terminal, 0) == "original text"
    });
    screen.frame(&mut terminal, vec![]);
    // Drag backwards across a repaint that replaces the application-owned screen.
    let start = screen.cell_at(7, 0);
    let end = screen.cell_at(0, 0);
    screen.frame(
        &mut terminal,
        vec![Event::PointerMoved(start), button(start, true)],
    );
    std::fs::write(dir.path().join("repaint"), "").unwrap();
    wait(|| viewport_line(&terminal, 0) == "updated text");
    screen.frame(&mut terminal, vec![Event::PointerMoved(end)]);
    screen.frame(&mut terminal, vec![button(end, false)]);
    let output = screen.frame(&mut terminal, vec![Event::Copy]);
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c,egui::OutputCommand::CopyText(text) if text=="original"))
    );
    let live = label(&output, "Return to live").unwrap();
    screen.frame(
        &mut terminal,
        vec![Event::PointerMoved(live), button(live, true)],
    );
    screen.frame(&mut terminal, vec![button(live, false)]);
    assert!(terminal.held_screen.is_none());
    assert_eq!(viewport_line(&terminal, 0), "updated text");
}

#[test]
fn focused_editor_owns_clipboard_and_ctrl_c_interrupts_foreground_job() {
    let dir = tempfile::tempdir().unwrap();
    let mut screen = Screen::new();
    let mut terminal = pane(&screen.ctx, dir.path());
    let mut editor = String::new();
    let mut search = TerminalSearch::default();
    for events in [vec![], vec![Event::Paste("editor only".into())]] {
        let _ = screen.ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.text_edit_singleline(&mut editor).request_focus();
                    terminal_view(ui, &mut terminal, &mut search, true, 15.0, false)
                        .1
                        .unwrap();
                });
            },
        );
    }
    assert_eq!(editor, "editor only");
    assert_eq!(terminal.input_bytes(), 0);
    terminal
        .input(b"touch started; sleep 10\n".to_vec())
        .unwrap();
    screen.frame(&mut terminal, vec![]);
    wait(|| dir.path().join("started").exists());
    // Click the grid to explicitly transfer focus from the editor.
    let pos = screen.cell_at(2, 2);
    screen.frame(
        &mut terminal,
        vec![Event::PointerMoved(pos), button(pos, true)],
    );
    screen.frame(&mut terminal, vec![button(pos, false)]);
    screen.frame(
        &mut terminal,
        vec![Event::Key {
            key: Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::CTRL,
        }],
    );
    terminal.input(b"printf ready > ready\n".to_vec()).unwrap();
    wait(|| dir.path().join("ready").exists());
}

#[test]
fn shift_wheel_enters_history_and_normal_wheel_stays_there_until_live() {
    let dir = tempfile::tempdir().unwrap();
    let mut screen = Screen::new();
    let mut terminal = pane(&screen.ctx, dir.path());
    screen.frame(&mut terminal, vec![]);
    terminal
        .input(b"seq 1 150; printf '\\033[?1000h\\033[?1006h'\n".to_vec())
        .unwrap();
    wait(|| terminal.mode().contains(TermMode::SGR_MOUSE));
    let pos = screen.cell_at(1, 2);
    screen.frame(&mut terminal, vec![Event::PointerMoved(pos)]);
    let before = terminal.input_bytes();
    // macOS can rotate Shift+wheel into the horizontal axis.
    let wheel = |x, y, modifiers| Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: Vec2::new(x, y),
        modifiers,
    };
    screen.frame(&mut terminal, vec![wheel(3.0, 0.0, Modifiers::SHIFT)]);
    wait(|| terminal.term.lock().unwrap().grid().display_offset() == 3);
    screen.frame(&mut terminal, vec![wheel(0.0, -3.0, Modifiers::NONE)]);
    wait(|| terminal.term.lock().unwrap().grid().display_offset() == 0);
    assert_eq!(terminal.input_bytes(), before);
    screen.frame(&mut terminal, vec![wheel(0.0, 1.0, Modifiers::NONE)]);
    wait(|| terminal.input_bytes() > before);
    assert_eq!(terminal.term.lock().unwrap().grid().display_offset(), 0);
}

#[test]
fn quick_drag_in_one_or_two_frames_selects_the_whole_span() {
    let dir = tempfile::tempdir().unwrap();
    let mut screen = Screen::new();
    let mut terminal = pane(&screen.ctx, dir.path());
    screen.frame(&mut terminal, vec![]);
    terminal
        .input(b"printf '\\033[2J\\033[Habcdefghijk\\n'\n".to_vec())
        .unwrap();
    wait(|| viewport_line(&terminal, 0) == "abcdefghijk");
    screen.frame(&mut terminal, vec![]);
    for single_frame in [true, false] {
        terminal.term.lock().unwrap().selection = None;
        let start = screen.cell_at(1, 0);
        let end = screen.cell_at(7, 0);
        let mut events = vec![Event::PointerMoved(start), button(start, true)];
        if !single_frame {
            screen.frame(&mut terminal, std::mem::take(&mut events));
        }
        events.extend([Event::PointerMoved(end), button(end, false)]);
        screen.frame(&mut terminal, events);
        assert_eq!(
            terminal
                .term
                .lock()
                .unwrap()
                .selection_to_string()
                .as_deref(),
            Some("bcdefgh"),
            "single frame: {single_frame}"
        );
    }
}
