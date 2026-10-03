use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::{Dimensions, Scroll},
    index::{Boundary, Side},
    selection::{Selection, SelectionType},
    term::TermMode,
    vi_mode::ViMotion,
};
use eframe::egui::{Key, Modifiers};

#[derive(Clone, Copy)]
pub enum SelectionAction {
    Enter,
    Exit,
    Move { motion: ViMotion, extend: bool },
    Page { up: bool, extend: bool },
    Toggle,
    Copy { exit: bool },
}

/// Copy-mode keys never become terminal input, including unrecognized keys.
pub fn key_action(key: Key, modifiers: Modifiers) -> Option<SelectionAction> {
    let extend = modifiers.shift;
    let motion = match key {
        Key::ArrowLeft | Key::H => {
            if modifiers.ctrl {
                ViMotion::SemanticLeft
            } else {
                ViMotion::Left
            }
        }
        Key::ArrowRight | Key::L => {
            if modifiers.ctrl {
                ViMotion::SemanticRight
            } else {
                ViMotion::Right
            }
        }
        Key::ArrowUp | Key::K => ViMotion::Up,
        Key::ArrowDown | Key::J => ViMotion::Down,
        Key::Home => ViMotion::First,
        Key::End => ViMotion::Last,
        Key::Escape => return Some(SelectionAction::Exit),
        Key::Space | Key::V => return Some(SelectionAction::Toggle),
        Key::PageUp => return Some(SelectionAction::Page { up: true, extend }),
        Key::PageDown => return Some(SelectionAction::Page { up: false, extend }),
        Key::Enter => return Some(SelectionAction::Copy { exit: true }),
        Key::C if modifiers.ctrl || modifiers.command => {
            return Some(SelectionAction::Copy { exit: false });
        }
        _ => return None,
    };
    Some(SelectionAction::Move { motion, extend })
}

fn start_selection<T: EventListener>(term: &mut Term<T>) {
    if term.selection.is_none() {
        let mut selection =
            Selection::new(SelectionType::Simple, term.vi_mode_cursor.point, Side::Left);
        selection.include_all();
        term.selection = Some(selection);
    }
}

/// Called by the existing terminal control worker, preserving ordering with input/resize.
pub fn apply<T: EventListener>(term: &mut Term<T>, action: SelectionAction) -> Option<String> {
    if !term.mode().contains(TermMode::VI) && !matches!(action, SelectionAction::Exit) {
        term.toggle_vi_mode();
    }
    match action {
        SelectionAction::Enter => term.selection = None,
        SelectionAction::Exit => {
            if term.mode().contains(TermMode::VI) {
                term.toggle_vi_mode();
            }
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
        }
        SelectionAction::Move { motion, extend } => {
            if extend {
                start_selection(term);
            }
            term.vi_motion(motion);
            term.scroll_to_point(term.vi_mode_cursor.point);
        }
        SelectionAction::Page { up, extend } => {
            if extend {
                start_selection(term);
            }
            let mut point = term.vi_mode_cursor.point;
            let lines = term.screen_lines().saturating_sub(1).max(1) as i32;
            point.line =
                (point.line + if up { -lines } else { lines }).grid_clamp(term, Boundary::Grid);
            term.vi_goto_point(point);
        }
        SelectionAction::Toggle => {
            if term.selection.is_some() {
                term.selection = None;
            } else {
                start_selection(term);
            }
        }
        SelectionAction::Copy { exit } => {
            let text = term.selection_to_string();
            if exit {
                apply(term, SelectionAction::Exit);
            }
            return text;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Size;
    use alacritty_terminal::{event::VoidListener, term::Config, vte::ansi::Processor};

    fn terminal(text: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size { cols: 10, rows: 3 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    #[test]
    fn keyboard_selection_copies_wide_combining_and_wrapped_text() {
        let mut term = terminal("1234567界e\u{301}word\r\n");
        apply(&mut term, SelectionAction::Enter);
        term.vi_goto_point(alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(0),
            alacritty_terminal::index::Column(7),
        ));
        apply(&mut term, SelectionAction::Toggle);
        for _ in 0..5 {
            apply(
                &mut term,
                SelectionAction::Move {
                    motion: ViMotion::Right,
                    extend: false,
                },
            );
        }
        assert_eq!(
            apply(&mut term, SelectionAction::Copy { exit: true }).as_deref(),
            Some("界e\u{301}word")
        );
        assert!(!term.mode().contains(TermMode::VI));
        assert!(term.selection.is_none());
        assert_eq!(term.grid().display_offset(), 0);
    }

    #[test]
    fn shift_navigation_starts_selection_and_escape_clears_it() {
        let mut term = terminal("hello\r");
        apply(&mut term, SelectionAction::Enter);
        apply(
            &mut term,
            SelectionAction::Move {
                motion: ViMotion::Right,
                extend: true,
            },
        );
        assert_eq!(term.selection_to_string().as_deref(), Some("he"));
        apply(&mut term, SelectionAction::Toggle);
        assert!(term.selection.is_none());
        apply(&mut term, SelectionAction::Exit);
        apply(&mut term, SelectionAction::Exit);
        assert!(!term.mode().contains(TermMode::VI));
    }

    #[test]
    fn page_navigation_clamps_to_history_and_live_screen() {
        let mut term = terminal("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
        apply(&mut term, SelectionAction::Enter);
        for _ in 0..20 {
            apply(
                &mut term,
                SelectionAction::Page {
                    up: true,
                    extend: false,
                },
            );
        }
        assert_eq!(term.vi_mode_cursor.point.line, term.topmost_line());
        assert!(term.grid().display_offset() > 0);
        for _ in 0..20 {
            apply(
                &mut term,
                SelectionAction::Page {
                    up: false,
                    extend: true,
                },
            );
        }
        assert_eq!(term.vi_mode_cursor.point.line, term.bottommost_line());
        assert!(term.selection_to_string().is_some());
        apply(&mut term, SelectionAction::Exit);
        assert_eq!(term.grid().display_offset(), 0);
    }
}
