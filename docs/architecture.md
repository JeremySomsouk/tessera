# Architecture / ADR 001

## Initial choice

The repository started with only an MIT license and Cargo-oriented gitignore. Tessera uses `alacritty_terminal 0.25.1` for terminal state and ANSI parsing, `portable-pty 0.9.0` for PTY ownership, and `eframe/egui 0.33.3` with the Glow renderer for its desktop shell. Versions are locked in Cargo.lock. These established compatible versions were chosen for a small vertical slice, rather than chasing the newest GUI release before input/rendering validation. License identifiers are Apache-2.0 (Alacritty), MIT (portable-pty), and MIT OR Apache-2.0 (egui/eframe); inspect transitive dependencies separately before distributing.

The spike demonstrated real shell I/O and resizing, ANSI/alternate-screen parsing, a headless-rendered Overview, and a bounded 100-session workload. It does **not** establish native macOS performance or complete terminal compatibility. The GUI choice should be revisited if native text/input/accessibility checks fail.

## Boundaries

- `terminal.rs`: one Alacritty state per live PTY, reader/parser thread, separate writer/control thread. UI accesses the grid with `try_lock` and reuses the pane's last painted content while the grid is busy; parsing and PTY writes never execute on the UI thread. Parser-generated replies are sent after releasing the terminal-state lock to prevent a resize/reply deadlock.
- `model.rs`: serializable session/task state, stable identities, bounded event history and duplicate/reordering handling. Task acceptance is always independent of session idleness.
- `integration.rs`: user-private Unix endpoint and the same executable's `hook` command. Inherited pane/endpoint identity associates manually launched Claude sessions with the correct pane. Only structured lifecycle metadata is retained.
- `search.rs`: ephemeral pane-local literal search using Alacritty’s search engine. One background search at a time per pane, with coalesced input and no history copy. Query generations discard obsolete results; terminal output/resize revisions invalidate match coordinates. Queries are limited to 256 characters and never persisted or sent to the PTY.
- `selection.rs`: keyboard copy mode built on Alacritty’s vi cursor and selection engine. Live-pane actions share the bounded terminal control queue so navigation/copy stay ordered without blocking the UI; exited shells retain selection through nonblocking grid access. Selection and clipboard text are ephemeral.
- `ui.rs`: workspace split tree, terminal painting/input, Overview, task inspector, and background persistence.
- `main.rs`: native app and integration CLI. No agent can invoke application actions through terminal escape sequences.

The current host is independent of view selection but **not of the application process**. A daemon/protocol boundary must be introduced before claiming window-restart survival or detach/reconnect. macOS paths and packaging are small adapters; PTY/model/layout code is usable on Unix. Credentials/connectors are intentionally absent.

## Resource limits

Local agent discovery scans process IDs, parent IDs, and executable names in a background worker at most every two seconds. It is scoped to descendants of live pane shells. Untracked process observations are upgraded in place by lifecycle hooks; they never claim turn activity or approval state. Failed scans do not mean the process exited.

32 workspaces, 32 simultaneously live panes, 256 recorded Claude sessions, 128 activity entries/session, 10,000 scrollback lines/pane, 16 KiB parser chunks, 256 queued terminal commands (64 KiB maximum per input/paste), 256 queued hook events, 64 KiB maximum hook input. Event summaries are bounded to 4 KiB. Overview rows are virtualized and do not paint terminal previews. Idle UI does not request a perpetual frame loop. Persistence is queued and rate-limited to roughly once every two seconds.

Input queue saturation is reported rather than silently accepted. Terminal output bytes are not discarded to improve apparent responsiveness. Clipboard OSC 52 access is denied. Hook payload strings never become shell commands. Hook executable paths are single-quote escaped; installer preserves unrelated groups and refuses malformed/symlink settings files.

## Persistence

macOS: `~/Library/Application Support/Tessera/workspace.json`.
Linux: `~/.config/tessera/workspace.json`.

Every launch discards saved tabs/splits and opens one new terminal in the application’s starting directory. Preferences and session histories survive; previous non-ended sessions become disconnected.

Writes are background atomic replacements with mode 0600. On parse errors, the original is preserved and new state goes to `workspace.recovered.json`. The shutdown path drains the persistence worker. A multi-process locking policy is still needed: run a single Tessera instance for this increment.
