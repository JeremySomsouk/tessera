# Using Tessera

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

The desktop shell uses a workspace rail at wider widths and horizontally scrolling tabs in smaller windows. **Terminal** and **Overview** switch views; **+ Workspace** opens a dedicated creation form. **Commands** (Command+Shift+P) searches workspace names, directories, and actions. Up/Down move through results, Enter opens the selected result, and Escape closes the command center. **Settings** contains theme, terminal text size, terminal behavior, updates, and keyboard reference. The workspace header offers both split directions and maximize/restore.

New terminals opened with Command+N, Command+T, or either split shortcut inherit the focused shell’s current working directory, including changes made with `cd`. Workspaces whose shell has exited use their launch directory; the workspace dialog uses an existing working directory and an optional separate name. Enter in either field creates the workspace. Escape or a click outside the command center closes it.

Closing a terminal asks for confirmation. Check “Don’t ask again for any terminal” and confirm to skip future prompts across all workspaces, including after restarting Tessera. Re-enable prompts with “Confirm before stopping terminals” in Workspace & commands → Settings. Shell exit (including Ctrl+D at an empty shell prompt) closes the pane without confirmation. Stopping or exiting the last pane removes its workspace; task history is retained. Renaming selects the current name; Enter saves and Escape cancels. The rename dialog rejects blank names.

In Overview, Up/Down select a session, Enter opens its terminal, and Escape returns. Wide layouts show sessions and activity side by side; smaller layouts use Sessions/Activity views. Click a session to inspect it, or double-click to open its live terminal. Filters keep the selected context within the visible results. These keys remain normal terminal input in a pane. Drag split separators to resize. Drag terminal text to select immediately, including inside Codex; no mode change or modifier is needed. Cmd+C copies, Cmd+V pastes. Right-click a selection to copy it, paste it into the current terminal, open it in a new terminal tab, or search it in the default browser. Terminal actions paste as one line without submitting; a new tab uses the source terminal’s current directory. Option/Alt-click forwards mouse clicks to applications that request them. Shift bypasses terminal mouse reporting for native scrollback. Trackpad and wheel gestures scroll terminal history or are forwarded to applications that request mouse input; alternate-screen programs that enable alternate scrolling receive arrow input. Change terminal text size and Light/Dark appearance in Settings. Right-click a workspace in the rail or tab strip for Find, keyboard selection, and stopping its focused pane. The bottom bar shows common shortcuts. Hover actions and workspaces to see their shortcuts. Workspace number shortcuts follow tab position and support the physical number row on layouts such as AZERTY. On the Create page, Enter in either field creates a workspace; on the Commands page, Enter opens the selected match. Invalid directories show an error inside the dialog and keep it open for correction.

Normal terminal typing uses a slim beam caret; applications can request block, underline, beam or hidden cursors. Terminal color queries report the actual palette and application overrides, so dark-theme detection uses the real background.

**Terminal search:** use Command+F (Ctrl+Alt+F on Linux) or Find in the workspace tab’s context menu. Search is literal and case-sensitive across the current screen and retained scrollback. Enter / Shift+Enter and Next / Previous navigate matches, wrapping at the ends; Escape closes search and restores terminal input. Only the current match is highlighted. Output changes or resizing clear the highlight; press Enter to search again. Queries stay in memory and are never sent to the shell or saved. Search follows Alacritty’s base-cell text semantics: wide characters and wrapped lines work, but combining marks are omitted and text hidden in the other terminal screen is excluded.

**Keyboard selection:** Command+Shift+Space (Ctrl+Alt+Shift+Space on Linux) or Select in the workspace tab’s context menu enters copy mode. Arrows (or H/J/K/L), Home/End and Page Up/Down move the selection cursor; Ctrl+Left/Right move by word. Shift extends a selection, and Space or V toggles selection at the cursor. The normal copy shortcut copies without leaving; Enter copies and returns to the live terminal, while Escape cancels. Typed text, paste and IME commits are suppressed during copy mode. Selection uses the active screen and retained scrollback, including after the shell exits. Opening Find leaves copy mode; entering copy mode closes Find.

**File drops:** drop a saved screenshot or other file onto a running terminal pane to paste its escaped path. Codex recognizes image paths as attachments in its prompt; Enter remains explicit submission. Close Find or copy mode first. If a screenshot preview does not provide a file path, save it and drop the saved file. Tessera does not upload or retain dropped image contents.

## Recovery and current boundaries

Every startup opens one fresh login-shell terminal in the application’s starting directory. Previous tabs and split layouts are discarded. Themes, font sizes, terminal preferences, and bounded session histories are saved locally. Previous sessions are marked disconnected unless already ended; their histories remain available in Overview. Closing a pane stops its shell; quitting the app stops hosted shells. Independently detached `nohup`/daemon processes are outside this lifecycle.

This increment has one application window, no external session daemon, no remote SSH tracking, no specification editor, and no Jira/GitHub connector. Hook histories contain event/tool names, not prompts, arguments, results, transcripts, or permission decisions. Ordinary terminal output remains in memory only. The terminal renderer is an initial implementation; see [compatibility](terminal-compatibility.md) before treating it as a replacement for a mature terminal.

[Agent hooks](integration.md) · [Terminal compatibility](terminal-compatibility.md)
