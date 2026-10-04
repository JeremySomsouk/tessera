![Tessera — tiled T identity and terminal pane mosaic](assets/tessera-banner.png)

# Tessera

A native terminal workspace for supervising Claude Code and Codex sessions.

Run your usual shell in resizable panes. Switch to **Overview** to see what is running, what needs attention, and what is ready to review. Return to the same live terminal when you need to act.

**0.3.0** redesigns the desktop interface, command center, and app identity. [Release notes](docs/releases/0.3.0.md)

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

The command requires a release with installer assets, introduced for 0.3.0. Until it is published, [build from source](docs/development.md) or download a [macOS release](https://github.com/JeremySomsouk/tessera/releases). macOS bundles are ad hoc signed; you may need to approve the first launch. Linux needs an X11/Wayland desktop and OpenGL/EGL runtime libraries. [Installation details](docs/installation.md)

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

Run `claude` or `codex` in a Tessera pane. Review the integration in `/hooks`; Codex requires trusting the definitions. Restart existing agent sessions if needed. Hook installation backs up existing configuration. [Integration details and uninstall](docs/integration.md)

Approvals stay in the real CLI. Tessera records lifecycle and tool names, not prompts or transcripts. A finished response never accepts a task automatically.

## Everyday use

Use **Terminal** and **Overview** to switch views. **+ Workspace** opens a directory and optional name. **Commands** searches workspaces and actions; Up/Down select, Enter opens, Escape closes. **Settings** holds appearance, terminal behavior, and updates.

On macOS, use Cmd below. On Linux, replace Cmd with Ctrl+Alt.

| Action | Shortcut |
| --- | --- |
| Commands / workspace picker | Cmd+Shift+P |
| Overview / previous terminal | Cmd+Shift+O |
| New workspace in the current directory | Cmd+N or Cmd+T |
| Split side by side / stacked | Cmd+D / Cmd+Shift+D |
| Maximize / restore pane | Cmd+Shift+Enter |
| Find in terminal | Cmd+F |
| Workspace 1–9 | Cmd+1–9 |
| Next session needing attention | Cmd+Shift+N |
| Stop focused pane | Cmd+W |

Drag terminal text to select it; right-click for copy, paste, new-tab, or browser-search actions. Drop a saved file to paste its path. Neither action submits a command for you. [Full controls and keyboard reference](docs/usage.md)

## What persists

Preferences and bounded session histories are saved locally. Every launch opens a fresh terminal; previous tabs and splits are not restored. Quitting stops hosted shells.

The macOS app downloads signed updates and installs them when you quit. On Linux, rerun the installer. Tessera is still an alpha: one window, local sessions, and an initial terminal renderer. [Terminal compatibility](docs/terminal-compatibility.md) · [Updates](docs/updates.md)

## Development

```sh
git clone https://github.com/JeremySomsouk/tessera.git
cd tessera
cargo run --locked
```

Rust stable is required; macOS needs Xcode Command Line Tools and Linux needs native GUI development libraries. [Build, validation, and release guide](docs/development.md)

[Architecture](docs/architecture.md) · [Progress](PROGRESS.md) · [Roadmap](TODO.md) · [MIT license](LICENSE)
