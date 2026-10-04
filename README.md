![Tessera — terminal panes gathered into a mosaic](assets/tessera-banner.png)

# Tessera

A desktop terminal workspace with a single Overview of real Claude Code and Codex sessions.

**0.2.6 adds direct terminal text selection, selected-text actions, and more compact terminal controls.** It combines real login-shell PTYs, resizable splits, keyboard navigation, saved preferences and session history, signed automatic macOS updates, and a hook-driven agent Overview. Specification editing, issue connectors, and detached session hosting follow in later increments.

## Quickstart

On macOS or Linux (Intel/AMD x86-64 and ARM64):

```sh
curl -fsSL https://github.com/JeremySomsouk/tessera/releases/latest/download/install.sh | sh
tessera
```

The command requires a release containing `install.sh` and `SHA256SUMS`; older releases do not provide these assets. It downloads a prebuilt release and verifies its SHA-256 checksum. No Rust toolchain or `sudo` is needed. macOS installs the complete app with automatic updates in `~/Applications/Tessera.app`; Linux installs the executable in `~/.local/bin`. Both platforms provide the `tessera` command there. If that directory is outside your `PATH`, the installer prints the command to add to your shell startup file:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

On macOS, you can also open Tessera from `~/Applications`. Quit Tessera before rerunning the installer. Bundles are ad hoc signed, not notarized; macOS may ask you to approve the app. Linux requires a graphical desktop with X11/Wayland, OpenGL/EGL and libxkbcommon runtime libraries (Ubuntu 22.04+ on x86-64, Ubuntu 24.04+ on ARM64, or compatible distributions). The installer does not install system packages or agent hooks.

Rerun the installer to update Linux. To install a specific release that includes installer assets:

```sh
curl -fsSL https://github.com/JeremySomsouk/tessera/releases/latest/download/install.sh | TESSERA_VERSION=0.2.7 sh
```

To remove Tessera, first uninstall any agent hooks using the commands below, then remove `~/.local/bin/tessera` and, on macOS, `~/Applications/Tessera.app`. Your saved preferences and history remain.

## Build from source

With Rust stable and Xcode Command Line Tools installed:

```sh
git clone https://github.com/JeremySomsouk/tessera.git
cd tessera
cargo install --path . --locked
tessera
```

This starts the native desktop application. It uses `$SHELL` as a login shell (defaults to `/bin/zsh` on macOS) and preserves shell startup files. Run ordinary commands, Vim, SSH, `claude`, or `codex` directly in a pane.

To build a Finder application and release DMG:

```sh
bash scripts/bundle-macos.sh
open dist/Tessera.app
```

CI produces separate Intel (`Tessera-x86_64.dmg`) and Apple Silicon (`Tessera-arm64.dmg`) artifacts in the **Build and verify** workflow. Open the DMG and drag Tessera to Applications before installing hooks. Bundles are ad-hoc signed, not notarized; macOS may require Open from the application's context menu.

To publish a version, tag the release commit containing this workflow and push the tag:

```sh
git tag v0.2.6
git push origin v0.2.6
```

Pushing a version tag (`v` followed by a digit) runs all checks and builds both DMGs, then publishes them on the [GitHub Releases page](https://github.com/JeremySomsouk/tessera/releases) with checked-in release notes when available, otherwise generated notes. New releases stay in draft until both macOS DMGs, signed update feeds, Linux x86-64/ARM64 archives, `SHA256SUMS`, and `install.sh` upload successfully. Linux x86-64 builds use Ubuntu 22.04; ARM64 builds use Ubuntu 24.04. Release tags require the `SPARKLE_ED25519_PRIVATE_KEY` Actions secret; see [update signing](docs/updates.md). A failed publishing job can be rerun to finish the release. Branch pushes and pull requests upload Actions artifacts only.

## Connect Codex CLI

Install Codex separately and use its normal login. Preview and install Tessera's observational lifecycle hooks:

```sh
tessera codex-hooks           # preview definitions
tessera install-codex-hooks   # merge with backup into $CODEX_HOME/hooks.json
codex                        # run inside a Tessera pane; open /hooks to review and trust
```

When `CODEX_HOME` is unset, the default is `~/.codex/hooks.json`. A custom file can be passed explicitly to the install/uninstall commands. Use a Codex release with lifecycle hooks, review and trust Tessera's exact definitions in `/hooks`, and restart the session if needed. Hooks disabled by user or administrator policy remain disabled. Existing inline hooks in `config.toml` remain untouched; Codex may warn about both representations in one config layer.

Codex and Claude sessions share **Cmd+Shift+O** Overview, with an agent label and independent identities even when session IDs match. Start/prompt/tool events show running activity, permission requests need attention, Stop marks response completion, Interrupt needs input, and SessionEnd ends tracking. Approval decisions stay in Codex's terminal; task review/acceptance stays explicit.

```sh
tessera uninstall-codex-hooks # removes only commands from this executable location
```

No API key, paid session, transcript scanning, `notify` replacement, or hook-trust bypass is introduced. The existing 64 KiB per-hook input limit also applies to Codex; larger tool payloads cannot be tracked. Older `notify`-only Codex versions, remote/Cloud sessions, and separately identified subagent cards are outside this increment. See [integration details](docs/integration.md).

## Connect Claude Code

Tessera does not replace Claude, use an API key, or start paid sessions automatically. Install Claude Code separately and use its normal login.

```sh
tessera hooks          # preview the exact settings additions
tessera install-hooks  # merge into ~/.claude/settings.json, retaining a backup
```

Then run `claude` in a Tessera pane. Press **Cmd+Shift+O** to see sessions, inspect their recorded activity, and return to the existing terminal. Run `/hooks` in Claude to check the integration against your installed version. Existing Claude sessions may need a restart to load configuration changes.

If using the bundle rather than a CLI install, run these commands with `"/Applications/Tessera.app/Contents/MacOS/tessera"` instead of `tessera`.

The helper runs only when `TESSERA_SOCKET` is inherited from a Tessera shell. Outside Tessera it is a no-op. Permission and input responses happen in the real CLI. `Stop` means a response finished; it never accepts a task automatically. Task titles and explicit review/acceptance actions are in the Overview inspector.

Uninstall using the same executable location:

```sh
tessera uninstall-hooks
```

See [integration details](docs/integration.md), including custom settings paths and stale executable paths.

## macOS updates

Starting with 0.2.2, the Finder application checks for new releases at launch and daily, downloads signed updates in the background, and installs them when Tessera quits. Running terminals are not stopped to apply an update. In **Workspace & commands** (Cmd+Shift+P), choose **Check for updates…**, or change automatic checking/downloading. Preferences are saved by Sparkle. Plain Cargo binaries and Linux builds do not update themselves.

Install 0.2.2 manually once to get the updater. Subsequent updates use the installed application rather than another browser download. Releases remain ad hoc signed: initial installation can require macOS approval, and Developer ID signing/notarization is required for Apple's normal trusted distribution. See [release and signing setup](docs/updates.md).

## Keyboard

On macOS, `Command` below is Cmd. On Linux, use Ctrl+Alt so ordinary terminal Ctrl shortcuts remain available.

| Action | Shortcut |
| --- | --- |
| Overview / previous terminal | Command+Shift+O |
| New terminal workspace in the active pane’s current directory | Command+N or Command+T |
| Rename current workspace | Command+Shift+R |
| Side-by-side split | Command+D |
| Stacked split | Command+Shift+D |
| Maximize / restore pane | Command+Shift+Enter |
| Next pane | Command+Alt+Right |
| Find in the focused terminal | Command+F |
| Keyboard copy mode | Command+Shift+Space |
| Workspace 1–9 | Command+1–9 |
| Workspace picker / commands | Command+Shift+P |
| Session needing attention | Command+Shift+N |
| Stop pane | Command+W |

New terminals opened with Command+N, Command+T, or either split shortcut inherit the focused shell’s current working directory, including changes made with `cd`. Workspaces whose shell has exited use their launch directory; the workspace dialog uses an existing working directory and an optional separate name. Enter in either field creates the workspace. Escape or a click outside Workspace & commands closes the panel.

Closing a terminal asks for confirmation. Check “Don’t ask again for any terminal” and confirm to skip future prompts across all workspaces, including after restarting Tessera. Re-enable prompts with “Confirm before stopping terminals” in Workspace & commands. Shell exit (including Ctrl+D at an empty shell prompt) closes the pane without confirmation. Stopping or exiting the last pane removes its workspace; task history is retained. Renaming selects the current name; Enter saves and Escape cancels. The rename dialog rejects blank names.

In Overview, Up/Down select a session, Enter opens its terminal, and Escape returns. These keys remain normal terminal input in a pane. Drag split separators to resize. Drag terminal text to select immediately, including inside Codex; no mode change or modifier is needed. Cmd+C copies, Cmd+V pastes. Right-click a selection to copy it, paste it into the current terminal, open it in a new terminal tab, or search it in the default browser. Terminal actions paste as one line without submitting; a new tab uses the source terminal’s current directory. Option/Alt-click forwards mouse clicks to applications that request them. Shift bypasses terminal mouse reporting for native scrollback. Trackpad and wheel gestures scroll terminal history or are forwarded to applications that request mouse input; alternate-screen programs that enable alternate scrolling receive arrow input. Use the font slider and Light/Dark button in the toolbar. Right-click a workspace tab for Find, keyboard selection, and stopping its focused pane. The bottom bar shows common shortcuts. Hover actions and workspace tabs to see their shortcuts. Workspace number shortcuts follow tab position and support the physical number row on layouts such as AZERTY. In the workspace dialog, Enter in Working directory creates a workspace; Enter in Find workspace opens the first match. Invalid directories show an error inside the dialog and keep it open for correction.

Normal terminal typing uses a slim beam caret; applications can request block, underline, beam or hidden cursors. Terminal color queries report the actual palette and application overrides, so dark-theme detection uses the real background.

**Terminal search:** use Command+F (Ctrl+Alt+F on Linux) or Find in the workspace tab’s context menu. Search is literal and case-sensitive across the current screen and retained scrollback. Enter / Shift+Enter and Next / Previous navigate matches, wrapping at the ends; Escape closes search and restores terminal input. Only the current match is highlighted. Output changes or resizing clear the highlight; press Enter to search again. Queries stay in memory and are never sent to the shell or saved. Search follows Alacritty’s base-cell text semantics: wide characters and wrapped lines work, but combining marks are omitted and text hidden in the other terminal screen is excluded.

**Keyboard selection:** Command+Shift+Space (Ctrl+Alt+Shift+Space on Linux) or Select in the workspace tab’s context menu enters copy mode. Arrows (or H/J/K/L), Home/End and Page Up/Down move the selection cursor; Ctrl+Left/Right move by word. Shift extends a selection, and Space or V toggles selection at the cursor. The normal copy shortcut copies without leaving; Enter copies and returns to the live terminal, while Escape cancels. Typed text, paste and IME commits are suppressed during copy mode. Selection uses the active screen and retained scrollback, including after the shell exits. Opening Find leaves copy mode; entering copy mode closes Find.

**File drops:** drop a saved screenshot or other file onto a running terminal pane to paste its escaped path. Codex recognizes image paths as attachments in its prompt; Enter remains explicit submission. Close Find or copy mode first. If a screenshot preview does not provide a file path, save it and drop the saved file. Tessera does not upload or retain dropped image contents.

## Recovery and current boundaries

Every startup opens one fresh login-shell terminal in the application’s starting directory. Previous tabs and split layouts are discarded. Themes, font sizes, terminal preferences, and bounded session histories are saved locally. Previous sessions are marked disconnected unless already ended; their histories remain available in Overview. Closing a pane stops its shell; quitting the app stops hosted shells. Independently detached `nohup`/daemon processes are outside this lifecycle.

This increment has one application window, no external session daemon, no remote SSH tracking, no specification editor, and no Jira/GitHub connector. Hook histories contain event/tool names, not prompts, arguments, results, transcripts, or permission decisions. Ordinary terminal output remains in memory only. The terminal renderer is an initial implementation; see [compatibility](docs/terminal-compatibility.md) before treating it as a replacement for a mature terminal.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
# Includes the local socket transport test on an unrestricted host:
cargo test --locked -- --include-ignored
python3 -m unittest discover -s tests -p 'test_*.py'
sh -n install.sh scripts/package-linux.sh
shellcheck install.sh scripts/package-linux.sh
```

Linux source builds need `libxkbcommon-dev`, `libwayland-dev`, and `libegl1-mesa-dev`. Releases include macOS bundles and Linux binaries. No Windows frontend is implemented.

[Architecture](docs/architecture.md) · [Progress](PROGRESS.md) · [Next increments](TODO.md) · [Performance](docs/performance.md)
