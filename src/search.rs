use crate::terminal::Terminal;
use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Boundary, Column, Direction, Line, Point, Side},
    term::search::{Match, RegexSearch},
};
use eframe::egui::{self, Key, Modifiers};
use std::{
    sync::{
        atomic::Ordering,
        mpsc::{Receiver, TryRecvError, channel},
    },
    time::Duration,
};

struct SearchResult {
    generation: u64,
    regex: RegexSearch,
    current: Option<Match>,
    revision: u64,
}

/// Search state belongs to a live pane, never to persisted workspace metadata.
#[derive(Default)]
pub struct TerminalSearch {
    open: bool,
    focus: bool,
    query: String,
    regex: Option<RegexSearch>,
    current: Option<Match>,
    revision: Option<u64>,
    pending: Option<Direction>,
    stale: bool,
    error: Option<String>,
    generation: u64,
    running: Option<Receiver<SearchResult>>,
    jump: bool,
}

impl TerminalSearch {
    pub fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.pending = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn contains(&self, point: Point) -> bool {
        self.open
            && self
                .current
                .as_ref()
                .is_some_and(|range| range.contains(&point))
    }

    pub fn invalidate(&mut self, revision: u64) {
        if self.revision.is_some_and(|previous| previous != revision) {
            self.current = None;
            self.stale = self.regex.is_some();
            self.revision = None;
        }
    }

    fn set_query(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.current = None;
        self.revision = None;
        self.error = None;
        self.stale = false;
        self.regex = None;
        self.pending = None;
        if self.query.is_empty() {
            return;
        }
        // Literal, case-sensitive search using the terminal engine's Unicode/wrap handling.
        let mut pattern = String::from("(?-i:");
        for c in self.query.chars() {
            if "\\.+*?()|[]{}^$#&-~".contains(c) {
                pattern.push('\\');
            }
            pattern.push(c);
        }
        pattern.push(')');
        match RegexSearch::new(&pattern) {
            Ok(regex) => {
                self.regex = Some(regex);
                self.pending = Some(Direction::Right);
            }
            Err(_) => self.error = Some("Search text is too complex".into()),
        }
    }

    fn find<T: EventListener>(&mut self, term: &mut Term<T>, direction: Direction) {
        let Some(regex) = &mut self.regex else {
            return;
        };
        let origin = match (&self.current, direction) {
            (Some(range), Direction::Right) => range.end().add(term, Boundary::None, 1),
            (Some(range), Direction::Left) => range.start().sub(term, Boundary::None, 1),
            (None, Direction::Right) => {
                Point::new(Line(-(term.grid().display_offset() as i32)), Column(0))
            }
            (None, Direction::Left) => Point::new(
                Line(term.screen_lines() as i32 - 1 - term.grid().display_offset() as i32),
                term.last_column(),
            ),
        };
        let side = match direction {
            Direction::Right => Side::Left,
            Direction::Left => Side::Right,
        };
        self.current = term.search_next(regex, origin, direction, side, None);
        self.stale = false;
    }

    fn poll(&mut self, revision: u64) {
        if let Some(received) = self.running.as_ref().map(|rx| rx.try_recv()) {
            match received {
                Ok(result) => {
                    self.running = None;
                    if result.generation == self.generation {
                        self.regex = Some(result.regex);
                        self.current = result.current;
                        self.revision = Some(result.revision);
                        self.stale = false;
                        self.invalidate(revision);
                        self.jump = self.current.is_some();
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.running = None;
                    self.pending = None;
                    self.current = None;
                    self.error = Some("Terminal state unavailable".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, terminal: &Terminal, enabled: bool) {
        if !self.open {
            return;
        }
        self.invalidate(terminal.revision());
        let query_id = ui.id().with("terminal-search-query");
        let query_focused = ui.memory(|memory| memory.has_focus(query_id));
        let mut close = false;
        if enabled {
            close = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            if query_focused && ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Enter)) {
                self.pending = Some(Direction::Left);
            } else if query_focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
            {
                self.pending = Some(Direction::Right);
            }
        }
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(query_id)
                        .hint_text("Find in terminal")
                        .desired_width((ui.available_width() - 180.0).clamp(80.0, 280.0))
                        .char_limit(256),
                );
                if self.focus && enabled {
                    response.request_focus();
                    self.focus = false;
                }
                if response.changed() {
                    self.set_query();
                }
                if ui.button("Previous").on_hover_text("Shift+Enter").clicked() {
                    self.pending = Some(Direction::Left);
                }
                if ui.button("Next").on_hover_text("Enter").clicked() {
                    self.pending = Some(Direction::Right);
                }
                close |= ui.button("Close").on_hover_text("Escape").clicked();
            });
        });
        if close {
            self.open = false;
            self.pending = None;
            return;
        }
        self.poll(terminal.revision());
        if self.running.is_none()
            && let Some(direction) = self.pending.take()
            && let Some(regex) = self.regex.take()
        {
            let (tx, rx) = channel();
            let term = terminal.term.clone();
            let revision = terminal.revision.clone();
            let ctx = ui.ctx().clone();
            let generation = self.generation;
            let current = self.current.clone();
            let expected_revision = self.revision;
            // At most one search worker per pane; edits/navigation coalesce while it runs.
            // Search reads the existing grid without copying history or blocking the UI.
            std::thread::spawn(move || {
                let mut search = Self {
                    regex: Some(regex),
                    current,
                    ..Default::default()
                };
                if let Ok(mut term) = term.lock() {
                    if expected_revision != Some(revision.load(Ordering::Acquire)) {
                        search.current = None;
                    }
                    search.find(&mut term, direction);
                    if let Some(regex) = search.regex.take() {
                        let _ = tx.send(SearchResult {
                            generation,
                            regex,
                            current: search.current,
                            revision: revision.load(Ordering::Acquire),
                        });
                    }
                }
                ctx.request_repaint();
            });
            self.running = Some(rx);
        }
        if self.jump {
            if let Ok(mut term) = terminal.term.try_lock() {
                self.invalidate(terminal.revision());
                if let Some(range) = &self.current {
                    term.scroll_to_point(*range.start());
                }
                self.jump = false;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
        let status = if let Some(error) = &self.error {
            error.as_str()
        } else if self.query.is_empty() {
            "Case-sensitive text · Enter / Shift+Enter · Escape to close"
        } else if self.running.is_some() || self.pending.is_some() {
            "Searching…"
        } else if self.stale {
            "Output changed · Enter to search again"
        } else if self.current.is_some() {
            "Match highlighted · Navigation wraps"
        } else {
            "No match in retained output"
        };
        ui.small(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Size;
    use alacritty_terminal::{event::VoidListener, term::Config, vte::ansi::Processor};

    fn terminal(text: &str, cols: usize, rows: usize) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size { cols, rows }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    fn search(query: &str) -> TerminalSearch {
        let mut search = TerminalSearch {
            query: query.into(),
            ..Default::default()
        };
        search.open();
        search.set_query();
        search
    }

    #[test]
    fn search_controls_fit_narrow_panes_in_both_themes() {
        let ctx = egui::Context::default();
        let terminal = Terminal::spawn_test(
            uuid::Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            std::path::Path::new("/tmp/unused-search-test.sock"),
            ctx.clone(),
        )
        .unwrap();
        for light in [false, true] {
            ctx.set_visuals(if light {
                egui::Visuals::light()
            } else {
                egui::Visuals::dark()
            });
            for width in [180.0, 320.0, 640.0] {
                let mut search = search("literal text");
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::Vec2::new(width, 240.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            search.show(ui, &terminal, true);
                            assert!(
                                ui.min_rect().right() <= width,
                                "search overflows a {width}px pane"
                            );
                            assert!(ui.min_rect().bottom() <= 240.0);
                        });
                    },
                );
            }
        }
    }

    #[test]
    fn literal_case_sensitive_navigation_wraps_both_directions() {
        let mut term = terminal("a.b A.B a.b", 20, 3);
        let mut search = search("a.b");
        search.find(&mut term, Direction::Right);
        assert_eq!(
            search.current,
            Some(Point::new(Line(0), Column(0))..=Point::new(Line(0), Column(2)))
        );
        search.find(&mut term, Direction::Right);
        assert_eq!(search.current.as_ref().unwrap().start().column, Column(8));
        search.find(&mut term, Direction::Right);
        assert_eq!(search.current.as_ref().unwrap().start().column, Column(0));
        search.find(&mut term, Direction::Left);
        assert_eq!(search.current.as_ref().unwrap().start().column, Column(8));
    }

    #[test]
    fn searches_scrollback_wrapped_wide_text() {
        let mut term = terminal("123界éword\r\nsecond\r\nthird\r\nfourth", 8, 2);
        let mut search = search("界éword");
        search.find(&mut term, Direction::Right);
        let range = search.current.as_ref().expect("wrapped Unicode match");
        assert!(range.start().line.0 < 0);
        assert_eq!(range.start().column, Column(3));
        assert!(range.end().line > range.start().line);
        term.scroll_to_point(*range.start());
        assert!(term.grid().display_offset() > 0);
    }

    #[test]
    fn empty_missing_and_mutated_output_do_not_leave_stale_highlights() {
        let mut term = terminal("match", 10, 2);
        let mut search = search("match");
        search.find(&mut term, Direction::Right);
        search.revision = Some(1);
        search.invalidate(2);
        assert!(search.current.is_none());
        assert!(search.stale);
        search.query = "absent".into();
        search.set_query();
        search.find(&mut term, Direction::Right);
        assert!(search.current.is_none());
        search.query.clear();
        search.set_query();
        assert!(search.regex.is_none());
        assert!(search.pending.is_none());
    }

    #[test]
    fn obsolete_worker_results_cannot_replace_new_queries_or_changed_output() {
        let mut search = search("old");
        let generation = search.generation;
        let (tx, rx) = channel();
        search.running = Some(rx);
        let regex = search.regex.take().unwrap();
        search.query = "new".into();
        search.set_query();
        tx.send(SearchResult {
            generation,
            regex,
            current: Some(Point::new(Line(0), Column(0))..=Point::new(Line(0), Column(2))),
            revision: 1,
        })
        .unwrap();
        search.poll(1);
        assert!(search.current.is_none());
        assert!(search.regex.is_some());
        assert!(search.pending.is_some());

        let (tx, rx) = channel();
        search.running = Some(rx);
        tx.send(SearchResult {
            generation: search.generation,
            regex: search.regex.take().unwrap(),
            current: Some(Point::new(Line(-10), Column(0))..=Point::new(Line(-10), Column(2))),
            revision: 1,
        })
        .unwrap();
        search.poll(2);
        assert!(search.current.is_none());
        assert!(search.stale);

        let (tx, rx) = channel();
        search.running = Some(rx);
        drop(tx);
        search.poll(2);
        assert!(search.running.is_none());
        assert!(search.error.is_some());
    }

    #[test]
    fn alternate_screen_does_not_search_hidden_primary_history() {
        let mut term = terminal("primary\x1b[?1049halternate", 20, 3);
        let mut search = search("primary");
        search.find(&mut term, Direction::Right);
        assert!(search.current.is_none());
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"\x1b[?1049l");
        search.find(&mut term, Direction::Right);
        assert!(search.current.is_some());
    }
}
