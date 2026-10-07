# Using Tessera

## Keyboard


On macOS, `Command` below is Cmd. On Linux, use Ctrl+Alt so ordinary terminal Ctrl shortcuts remain available.

| Action | Shortcut |
| --- | --- |
| Work / previous terminal | Command+Shift+O |
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

The desktop shell uses a workspace rail at wider widths and horizontally scrolling tabs in smaller windows. Middle-click a workspace to stop its focused terminal using the existing close-confirmation setting. **Terminal** and **Work** switch views; **+ Workspace** opens a dedicated creation form. **Commands** (Command+Shift+P) searches workspace names, directories, and actions. Up/Down move through results, Enter opens the selected result, and Escape closes the command center. **Settings** contains theme, terminal text size, terminal behavior, updates, and keyboard reference. The workspace header offers both split directions and maximize/restore.

On macOS, Command+Enter inserts a newline in Claude Code without submitting by sending the same line feed as Ctrl+J. Plain Enter still submits. This mapping applies to terminal input, so other applications handle it according to their Ctrl+J binding. Search, copy mode, and dialogs retain their own keyboard controls.

Tessera detects the terminal font at startup from the default iTerm2 or Terminal.app profile, or an explicit `font_family` in kitty configuration. When launched from a recognized terminal, that terminal takes precedence; otherwise Tessera checks iTerm2, Terminal.app, then kitty. The font must be installed and monospace. An installed Nerd Font supplies missing prompt icons, followed by bundled Unicode and monochrome emoji fallbacks. Settings shows the selected font. Restart Tessera after changing a terminal profile or installing fonts. Other terminal configurations and included kitty configuration files are not read yet.

Hover the workspace title to see its launch directory. The compact header leaves more room for terminal output.

New terminals opened with Command+N, Command+T, or either split shortcut inherit the focused shell’s current working directory, including changes made with `cd`. Workspaces whose shell has exited use their launch directory. New workspace accepts an existing or new directory and an optional separate name. Missing folders are created on submission. `~/` expands to your home directory; relative paths resolve against the application's starting directory (your home directory when started at `/`). The resolved path is shown before creation. A leading `/` denotes an absolute path: for a missing root-level folder, the form offers an explicit home-directory alternative. Read-only and permission failures include recovery guidance. Enter in either field creates the workspace. Escape or a click outside the command center closes it.

Closing a terminal asks for confirmation. Check “Don’t ask again for any terminal” and confirm to skip future prompts across all workspaces, including after restarting Tessera. Re-enable prompts with “Confirm before stopping terminals” in Workspace & commands → Settings. Shell exit (including Ctrl+D at an empty shell prompt) closes the pane without confirmation. Stopping or exiting the last pane removes its workspace; work context and session history are retained. Renaming selects the current name; Enter saves and Escape cancels. The rename dialog rejects blank names.

In Work, Up/Down select an item, Enter opens its actionable terminal when available, and Escape returns. Wide layouts show work and details side by side; smaller layouts switch between the list and its details. Filters keep the selected context within the visible results. These keys remain normal terminal input in a pane. Drag split separators to resize. Drag terminal text to select immediately, including inside Codex; no mode change or modifier is needed. Cmd+C copies, Cmd+V pastes. Right-click a selection to copy it, paste it into the current terminal, open it in a new terminal tab, or search it in the default browser. Terminal actions paste as one line without submitting; a new tab uses the source terminal’s current directory. Option/Alt-click forwards mouse clicks to applications that request them. Shift bypasses terminal mouse reporting for native scrollback. Trackpad and wheel gestures scroll terminal history or are forwarded to applications that request mouse input; alternate-screen programs that enable alternate scrolling receive arrow input. Change terminal text size and Light/Dark appearance in Settings. Right-click a workspace in the rail or tab strip for Find, keyboard selection, and stopping its focused pane. The bottom bar groups common actions with outlined keyboard shortcuts. Hover actions and workspaces to see their shortcuts. Workspace number shortcuts follow tab position and support the physical number row on layouts such as AZERTY. On the Create page, Enter in either field creates a workspace; on the Commands page, Enter opens the selected match. Invalid directories show an error inside the dialog and keep it open for correction.

Double-click terminal text to select the full token between spaces or tabs, including punctuation in paths and URLs. Selection follows wrapped lines and includes wide and combining characters.

Normal terminal typing uses a slim beam caret; applications can request block, underline, beam or hidden cursors. Terminal color queries report the actual palette and application overrides, so dark-theme detection uses the real background.

**Terminal search:** use Command+F (Ctrl+Alt+F on Linux) or Find in the workspace tab’s context menu. Search is literal and case-sensitive across the current screen and retained scrollback. Enter / Shift+Enter and Next / Previous navigate matches, wrapping at the ends; Escape closes search and restores terminal input. Only the current match is highlighted. Output changes or resizing clear the highlight; press Enter to search again. Queries stay in memory and are never sent to the shell or saved. Search follows Alacritty’s base-cell text semantics: wide characters and wrapped lines work, but combining marks are omitted and text hidden in the other terminal screen is excluded.

**Keyboard selection:** Command+Shift+Space (Ctrl+Alt+Shift+Space on Linux) or Select in the workspace tab’s context menu enters copy mode. Arrows (or H/J/K/L), Home/End and Page Up/Down move the selection cursor; Ctrl+Left/Right move by word. Shift extends a selection, and Space or V toggles selection at the cursor. The normal copy shortcut copies without leaving; Enter copies and returns to the live terminal, while Escape cancels. Typed text, paste and IME commits are suppressed during copy mode. Selection uses the active screen and retained scrollback, including after the shell exits. Opening Find leaves copy mode; entering copy mode closes Find.

**File drops:** drop a saved screenshot or other file onto a running terminal pane to paste its escaped path. Codex recognizes image paths as attachments in its prompt; Enter remains explicit submission. Close Find or copy mode first. If a screenshot preview does not provide a file path, save it and drop the saved file. Tessera does not upload or retain dropped image contents.

## Recovery and current boundaries

Every startup opens one fresh login-shell terminal in the application's starting directory, using your home directory when the application starts at the filesystem root. Previous tabs and split layouts are discarded. Themes, font sizes, terminal preferences, work items, specifications and bounded session histories are saved locally. The saved Light/Dark appearance applies from the first frame, independently of the system theme. Previous sessions are marked disconnected unless already ended; their work context and histories remain available in Work. Closing a pane stops its shell; quitting the app stops hosted shells. Independently detached `nohup`/daemon processes are outside this lifecycle.

This increment has one application window, no external session daemon, no remote SSH tracking, no Jira/GitHub connector. Hook histories contain event/tool names, not prompts, arguments, results, transcripts, or permission decisions. Ordinary terminal output remains in memory only. The terminal renderer is an initial implementation; see [compatibility](terminal-compatibility.md) before treating it as a replacement for a mature terminal.

[Agent hooks](integration.md) · [Terminal compatibility](terminal-compatibility.md)

## Work and specification review

Work opens on Attention. Active lists ongoing work; History contains completed, archived and retained disconnected work. **New work** creates a lightweight item without requiring a specification. Edit its title and directory, choose a stage (Define, Plan, Build, Verify, Review or Deliver), and set its disposition explicitly. These controls record developer intent; they do not certify tests or change agent permissions.

Ordinary work with only closed sessions moves out of the default view. Choose **Active** in its details to reopen it explicitly; this intent survives another restart and diagnostic cleanup. New work stops at 256 items without deleting completed or archived context. At capacity, observed sessions remain available for manual attachment to existing work.

Each item lists its linked Claude Code and Codex sessions. **Attach a session to this work** moves another recorded session into the selected item. Workspace membership does not determine its stage. Observed CLI requests take priority over uninspected responses, followed by explicit verification/review/delivery work. Open the associated terminal to inspect the actual request or result. Raw hook events are collapsed under Diagnostics. Observation age describes the last event received, not proof that tracking is complete or that a quiet agent has stopped.

Use **Add specification** or **Open specification and revisions** inside a work item. **Launch implementation** saves a revision and requests implementation from Claude or Codex; use ordinary terminal prompts for planning, verification and review. Launch, Stop and process exit never complete the work or advance its stage.

The specification editor accepts a directory that does not yet exist. **Launch implementation** creates missing folders before starting Claude or Codex, using the same path resolution as New workspace. The resolved directory is saved with the launched revision. Editing or saving a draft creates no folders. If directory creation fails, the draft and launch history stay intact.

Workspace configuration also keeps a `created_directories` registry with the canonical path and creation time of each new workspace target folder. On macOS it is saved in `~/Library/Application Support/Tessera/workspace.json`; on Linux, `~/.config/tessera/workspace.json`. Existing folders are excluded. Records survive closing tabs and restarting the application, including when a shell fails to launch after creating its folder. The registry is intended for later inspection and cleanup; it does not authorize automatic deletion or track intermediate parent folders.

In the specification editor, choose **Propose a change**, edit or paste the proposed title, directory and Markdown scope, and compare the original with the proposal. **Accept changes** saves a new immutable revision; **Cancel proposal** leaves your draft unchanged. Field-only acceptance is tucked under Advanced and discards the remaining proposed fields. Pending proposals survive restart. If the draft or saved revision changes during review, acceptance is blocked: cancel and prepare a fresh proposal. Proposals are entered locally; agents do not update them automatically.

Expand Revision history to compare any saved version with your draft. **Restore this version** first saves your current draft, then creates a new revision from the selected snapshot. Existing launch links still point to their original revision. A pending proposal must be accepted or cancelled before restoring.
