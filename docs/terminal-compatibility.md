# Terminal compatibility

## Implemented

Real login shell in a PTY, inherited environment/startup files, PTY resizing, parser-driven ANSI colors (16/256/truecolor), alternate screen, cursor, bounded scrollback, Unicode/wide/combining character state, drag selection/copy, paste/bracketed paste, control keys, application cursor mode, arrow/home/end/edit keys, basic mouse button reporting, IME commit input, splits and preserved live pane identity. OSC 52 clipboard reads/writes are denied. TERM is xterm-256color.

## Checked during implementation

Portable tests: real shell input/output and resize (`stty size`), cursor-position query while output continues, alternate-screen restoration, a wide UTF-8 glyph, Ctrl+C encoding, application cursor encoding, split/remove identity preservation. UI geometry/rendering tested headlessly in both appearances and at 640px width.

## Native acceptance checklist (not yet performed)

- macOS Intel + Apple Silicon launch, quit, reopen/resume, Unicode text and font fallback.
- Vim/Neovim editing, tmux navigation, less paging, shell job control/Ctrl+C/Ctrl+Z, SSH.
- Real Claude login, a permission prompt, an input wait, Stop, SessionEnd, and jumping back to the same pane.
- Selection/copy/paste, IME composition, Option-modified text, Retina scaling, drag split resizing.
- Keyboard-only Overview, system accessibility/VoiceOver, narrow window chrome, reduced motion.

## Known gaps

No terminal search, full IME preedit, configurable system font/fallback, full mouse-motion/wheel protocol, Kitty keyboard protocol, all function/keypad encodings, cursor-shape fidelity, bold/italic rendering, OSC title in pane chrome, OSC hyperlinks, terminfo auditing, accessibility grid, or daemon-hosted reconnect. Ordinary terminal painting stays dark when application chrome is light. These gaps mean the app is runnable but should remain an alpha alongside your existing terminal.
