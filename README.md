![Tessera — tiled T identity and terminal pane mosaic](assets/tessera-banner.png)

# Tessera

A native terminal workspace for supervising Claude Code and Codex sessions.

Run your usual shell in resizable panes. Open **Work** to see which piece of work needs a decision, inspect its linked Claude Code or Codex sessions, and return to the same terminal to act.

**0.7.0** adds Command+Enter for multiline Claude Code prompts on macOS and middle-click closing of terminal tabs. [Release notes](docs/releases/0.7.0.md)

## Install

macOS or Linux, on x86-64 or ARM64:

```sh
curl -fsSL https://github.com/JeremySomsouk/tessera/releases/latest/download/install.sh | sh
tessera
```

No Rust toolchain or `sudo` needed. The installer verifies release checksums and installs the macOS app in `~/Applications` or the Linux executable in `~/.local/bin`. If needed, add that directory to your `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Installer assets are available from 0.3.0 onward. You can also [build from source](docs/development.md) or download a [macOS release](https://github.com/JeremySomsouk/tessera/releases). macOS bundles are ad hoc signed; you may need to approve the first launch. Linux needs an X11/Wayland desktop and OpenGL/EGL runtime libraries. [Installation details](docs/installation.md)

## Connect your agents

Install Claude Code or Codex separately and sign in normally. Preview Tessera's hooks, then install the ones you use:

```sh
# Claude Code
tessera hooks
tessera install-hooks

# Codex
tessera codex-hooks
tessera install-codex-hooks
```

Run `claude` or `codex --no-daemon` in a Tessera pane. Codex's shared daemon can retain another pane's environment or an expired Tessera socket; `--no-daemon` keeps hooks attached to the current pane. Review the integration in `/hooks`; Codex requires trusting the definitions. Restart existing agent sessions if needed. Hook installation backs up existing configuration. [Integration details and uninstall](docs/integration.md)

Open local agents also appear before hooks arrive, labelled **Untracked** until lifecycle tracking starts. Approvals stay in the real CLI. Tessera records lifecycle and tool names, not prompts or transcripts. A finished response never accepts a task automatically.

## Everyday use

Use **Terminal** and **Work** to switch views. **+ Workspace** opens a directory, creating it if needed. The optional name sets the workspace label. **Commands** searches workspaces and actions; Up/Down select, Enter opens, Escape closes. **Settings** holds appearance, terminal behavior, and updates.

On macOS, use Cmd below. On Linux, replace Cmd with Ctrl+Alt.

| Action | Shortcut |
| --- | --- |
| Commands / workspace picker | Cmd+Shift+P |
| Work / previous terminal | Cmd+Shift+O |
| New workspace in the current directory | Cmd+N or Cmd+T |
| Split side by side / stacked | Cmd+D / Cmd+Shift+D |
| Focus neighboring split pane | Cmd+← / → / ↑ / ↓ |
| Maximize / restore pane | Cmd+Shift+Enter |
| Find in terminal | Cmd+F |
| Workspace 1–9 | Cmd+1–9 |
| Next session needing attention | Cmd+Shift+N |
| Stop focused pane | Cmd+W |

On macOS, Command+Enter sends a line feed (the same as Ctrl+J) to insert a newline in Claude Code without submitting. Plain Enter still submits. Middle-click a workspace tab to stop its focused terminal using the existing close-confirmation setting.

Drag terminal text to select it; right-click for copy, paste, new-tab, or browser-search actions. Drop a saved file to paste its path. Neither action submits a command for you. [Full controls and keyboard reference](docs/usage.md)

## Work and specifications

Work keeps the developer stage separate from agent activity. Choose Define, Plan, Build, Verify, Review or Deliver explicitly. Completed and archived work moves to History; changing a stage does not certify tests, review or delivery. Attention prioritizes observed permission/input requests and uninspected response endings. Raw hook events remain available under tracking details.

Ordinary Claude Code and Codex sessions receive lightweight work context. Attach related sessions to the same work item when they share a purpose; sessions in a single terminal workspace can belong to different work. Work titles, directories, stages, specification links and session associations survive terminal closure and application restart. An observed response ending never advances a stage or completes the work automatically.

Select a work item and use **Add specification** or **Open specification and revisions** to prepare local Markdown scope. Edit the title, working directory, and scope; **Save revision** retains an immutable snapshot. Revision history remains available after later edits. Cmd+Shift+S (Ctrl+Alt+Shift+S on Linux) opens specification editing through Work.

Choose Claude or Codex and inspect the exact launch context. **Launch implementation** saves the current revision and starts the agent in a new PTY in the working directory. This launcher explicitly requests implementation; use the terminal for investigation, planning, verification and review. Launching does not change the work stage. Codex uses `--no-daemon`. The command is passed as a quoted literal through an interactive login shell (`$SHELL` for sh/bash/zsh, `/bin/sh` otherwise); the agent executable must be available in that shell's PATH. Hooks and approvals behave as in ordinary agent terminals.

The editor lists launches, pinned revisions and exit results. Agent terminals retain their output after exit until explicitly closed; failed launches show an error. Work links sessions back to their launch specification and pinned revision. Agent completion never accepts the specification automatically. Specifications, revision text, and launch links are stored locally with preferences; they are sent to the selected agent only on launch. Launch context is limited to 8000 bytes, with up to 256 revisions and launches per specification. GitHub/Jira import and agent-proposed specification changes are future increments.

## What persists

Preferences, bounded work items, specifications and session histories are saved locally. Every launch opens a fresh terminal; previous tabs and splits are not restored. Retained work does not reconnect old processes. Quitting stops hosted shells.

The macOS app downloads signed updates and installs them when you quit. On Linux, rerun the installer. Tessera is still an alpha: one window, local sessions, and an initial terminal renderer. [Terminal compatibility](docs/terminal-compatibility.md) · [Updates](docs/updates.md)

## Development

```sh
git clone https://github.com/JeremySomsouk/tessera.git
cd tessera
cargo run --locked
```

Rust stable is required; macOS needs Xcode Command Line Tools and Linux needs native GUI development libraries. [Build, validation, and release guide](docs/development.md)

[Architecture](docs/architecture.md) · [Progress](PROGRESS.md) · [Roadmap](TODO.md) · [MIT license](LICENSE)
