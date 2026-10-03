# Terminal compatibility

## Implemented

Real login shell in a PTY, inherited environment/startup files, PTY resizing, parser-driven ANSI colors (16/256/truecolor), alternate screen, beam typing caret and application-requested cursor shapes, bounded scrollback, Unicode/wide/combining character state, pane-local literal search with scrollback navigation, drag and keyboard selection/copy, paste/bracketed paste, control keys, application cursor mode, arrow/home/end/edit keys, basic mouse button and wheel reporting (SGR and legacy), accumulated trackpad scrolling, return to live input on typing/paste, opt-in clickable Codex question choices, alternate-screen wheel-to-arrow input, IME commit input, splits and preserved live pane identity. OSC 52 clipboard reads/writes are denied. TERM is xterm-256color.

## Checked during implementation

Portable tests: real shell input/output and resize (`stty size`), cursor-position query while output continues, alternate-screen restoration, a wide UTF-8 glyph, Ctrl+C encoding, application cursor encoding, split/remove identity preservation. Additional regressions cover OSC foreground/background replies (including dynamic overrides), cursor-style requests, keyboard selection of wide/combining/wrapped text, copy after shell exit, copy-mode input isolation, custom-title number shortcuts with physical keys, stacked split routing, and workspace dialog Enter actions. Wheel regressions cover small trackpad deltas, mouse/alternate-screen/history routing, modifiers and legacy/SGR encoding. UI geometry/rendering tested headlessly in both appearances and at 640px width.

## Native acceptance checklist (not yet performed)

- macOS Intel + Apple Silicon launch, quit, reopen/resume, Unicode text and font fallback.
- Vim/Neovim editing, tmux navigation, less paging, shell job control/Ctrl+C/Ctrl+Z, SSH.
- Real Claude login, a permission prompt, an input wait, Stop, SessionEnd, and jumping back to the same pane.
- Terminal search in scrollback/alternate screen, search input isolation, narrow split search controls, keyboard copy-mode navigation/input isolation, selection/copy/paste, IME composition, Option-modified text, Retina scaling, drag split resizing.
- Keyboard-only Overview, system accessibility/VoiceOver, narrow window chrome, reduced motion.

## Known gaps

Search omits combining marks under Alacritty’s base-cell semantics; output changes invalidate the current match until the next search action. No full IME preedit, configurable system font/fallback, full mouse-motion protocol, Kitty keyboard protocol, all function/keypad encodings, cursor blinking, bold/italic rendering, OSC title in pane chrome, OSC hyperlinks, terminfo auditing, accessibility grid, or daemon-hosted reconnect. Ordinary terminal painting stays dark when application chrome is light. These gaps mean the app is runnable but should remain an alpha alongside your existing terminal.

## Codex question clicks

Enable **Clickable Codex questions** in Workspace & commands. In Codex's numbered question prompt, clicking an option selects it; press Enter to submit. Shift/drag retains text selection. Native mouse-reporting applications keep their own input handling. Compatibility only recognizes the visible question prompt with its selection marker and keyboard footer; ordinary numbered output, scrollback, search, copy mode, and notes entry do not receive synthetic keys. This is checked against Codex 0.160.0; future prompt layouts may require an update.

Typing, keyboard navigation, IME commits, and pasting return scrollback to the live input. Scrolling, copying, and terminal protocol replies preserve the current view.
