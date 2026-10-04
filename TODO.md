# Tessera increments

## First usable slice (0.1)
- [x] Real PTY + mature terminal-state engine.
- [x] Workspace tabs, split tree, draggable resizing, focus, maximize, picker.
- [x] Hook-driven Overview, bounded histories, task titles and explicit review/acceptance.
- [x] Integration preview/install/uninstall preserving existing configuration.
- [x] Preference/history persistence and a fresh terminal on each startup.
- [x] Portable tests, headless appearance checks, macOS bundle script and CI.
- [x] Codex lifecycle adapter, explicit install/uninstall, mixed-agent Overview and backward-compatible history.
- [ ] Validate on Intel and Apple Silicon Macs with real Claude and Codex sessions (native acceptance gate).
- [ ] Validate CI artifacts on Apple Silicon and finish native terminal smoke checklist.

## Next: terminal and Overview polish
- [x] Discover local agents before hooks arrive, with explicit untracked state and in-place hook upgrades.
- [x] Pane-local literal terminal search, scrollback navigation and keyboard controls.
- [x] Keyboard selection/copy mode with scrollback navigation and input isolation.
- [x] App-wide appearance, responsive workspace navigation, searchable commands and grouped settings.
- [ ] Additional font/key configuration and native font fallback.
- [ ] Mouse wheel/motion/keypad/function-key coverage, IME preedit and accessibility.
- [ ] Saved layout recipes, pane moves, workspace close/archive, layout undo.
- [ ] Durable host process, detach/move/stop choices, reconnect protocol and multiple windows.
- [ ] Better per-task session grouping, branch/worktree metadata, bookmark/excerpt inspector.
- [ ] Explicit unknown/stale tracking freshness and optional remote helper.
- [ ] Native performance instrumentation on Intel 16 GB and Apple Silicon.

## Then: specifications
- [x] Local Markdown board/editor, stable spec revisions, pinned task-launch context preview and Claude/Codex launches.
- [x] Local proposal against a base revision, side-by-side review, stale conflicts, field-selective acceptance.
- [x] Compare immutable revisions and restore while preserving unsaved drafts.
- [ ] Automatic agent proposal ingestion and line-level selective acceptance.
- [ ] Scope amendments with acknowledgement, independent worktrees and collision warnings.

## Connectors and release
- [ ] Jira/GitHub read/import, project configuration and Keychain credential adapter.
- [ ] Reviewed publish with conflict re-fetch and rich-content preservation.
- [ ] Verification receipts, staleness, criterion-linked change requests and handoff.
- [x] One-command macOS/Linux installer and x86-64/ARM64 release packaging.
- [ ] Developer ID signing/notarization, native signed-updater acceptance, Linux package smoke testing.
