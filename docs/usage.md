# Using Tessera

## Keyboard


On macOS, `Command` below is Cmd. On Linux, use Ctrl+Alt so ordinary terminal Ctrl shortcuts remain available.

| Action | Shortcut |
| --- | --- |
| Work / previous terminal | Command+Shift+O |
| New terminal workspace in the active pane’s current directory | Command+N or Command+T |
| New terminal window in the active pane’s current directory | Command+Shift+T |
| Rename current workspace | Command+Shift+R |
| Side-by-side split | Command+D |
| Stacked split | Command+Shift+D |
| Maximize / restore pane | Command+Shift+Enter |
| Next pane | Command+Alt+Right |
| Find in the focused terminal | Command+F |
| Keyboard copy mode | Command+Shift+Space |
| Workspace 1–9 | Command+1–9 |
| Workspace picker / commands | Command+Shift+P |
| Search folders and launch Claude Code | Command+Shift+L |
| Settings | Command+, |
| Session needing attention | Command+Shift+N |
| Stop pane | Command+W |

The desktop shell uses a workspace rail at wider widths and horizontally scrolling tabs in smaller windows. The rail shows readable directory paths, using `~/` for your home folder, and marks workspaces with sessions needing attention. The selected workspace has an accent marker; the focused pane has a stronger border. Middle-click a workspace to stop its focused terminal using the existing close-confirmation setting. **Terminal** and **Work** switch views; **+ Workspace** opens a dedicated creation form. **Commands** (Command+Shift+P) searches workspace names, directories, and actions. Up/Down move through results, Enter opens the selected result, and Escape closes the command center. The **Pane** menu groups both split directions and maximize/restore in wide and narrow layouts. **Settings** is available in the header or with Command+, and contains appearance, folder search roots, terminal behavior, updates, and keyboard reference.

**Claude in folder:** choose the header action, the matching Commands action, or Command+Shift+L (Ctrl+Alt+Shift+L on Linux). Type part of a folder name. Matching ignores case and searches substrings; exact names and prefixes appear first. Folder results have their own page and show the name and full path, with long paths expanded below the selected row. Use Up/Down to select, Enter or a click to launch, and Escape to close. Searching alone never launches anything. The selected existing folder opens in a new focused terminal running plain `claude`, without project registration, a specification, or a manual work item. You can also paste an absolute path, `~/` path, or relative path such as `./my-folder`.

Configure **Search roots** on the **Claude in folder** page or under **Settings → Folder search**. Folder-name searches stay inside those folders and their subfolders, including Spotlight on macOS. A bounded filesystem search supplements Spotlight; Linux uses this filesystem search directly. Use up to 16 roots, entered one per line. For example, enter `/path/to/projects` to search only that tree. Exact pasted paths can still open folders outside the configured roots. An empty list uses your home folder. The fallback searches up to six levels and 20,000 entries, with at most 80 results. Hidden folders, common dependency/build trees, and `Library` are skipped; child symlinks can match but are not traversed. To reach an excluded or deeply nested location, paste its path or add a specific root inside it. **Search coverage and limits** explains indexing gaps, access failures, and truncated searches. No match does not establish that a folder is absent.

Loading, empty, and error states stay in the picker. Changed queries discard stale results. Missing, deleted, inaccessible, or non-directory targets show an error and are never created. Launch uses the supported interactive login shell's PATH and restores the selected physical working directory after shell startup; spaces, apostrophes, and Unicode are quoted safely. Existing hooks and normal Claude Code approvals remain in effect. If `claude` is unavailable, its terminal retains an error explaining how to check the command in your usual terminal and retry. Tessera does not install Claude Code or change its permissions or settings.

On macOS, Command+Enter inserts a newline in Claude Code without submitting by sending the same line feed as Ctrl+J. Plain Enter still submits. This mapping applies to terminal input, so other applications handle it according to their Ctrl+J binding. Search, copy mode, and dialogs retain their own keyboard controls.

Tessera detects the terminal font at startup from the default iTerm2 or Terminal.app profile, or an explicit `font_family` in kitty configuration. When launched from a recognized terminal, that terminal takes precedence; otherwise Tessera checks iTerm2, Terminal.app, then kitty. The font must be installed and monospace. An installed Nerd Font supplies missing prompt icons, followed by bundled Unicode and monochrome emoji fallbacks. Settings shows the selected font. Restart Tessera after changing a terminal profile or installing fonts. Other terminal configurations and included kitty configuration files are not read yet.

Drag a workspace tab or rail entry outside its window and release to move it into a new terminal window. Its running shells, scrollback, split panes, and agent links move together. The new window starts at the screen drop point. Moving the last tab out closes the source window. Releasing inside the source window leaves it there; Escape cancels the drag. You can also right-click a tab and choose **Move to new window**. **New terminal window** in Commands, or Command+Shift+T, starts a fresh shell in a separate window. Workspace number shortcuts apply to the current window; Commands and Work can find terminals in other windows and bring their window forward. Closing a window stops its hosted shells, while the other windows keep running. Closing the final window quits Tessera.

Hover the workspace title to see its launch directory. The compact header leaves more room for terminal output.

New terminals opened with Command+N, Command+T, or either split shortcut inherit the focused shell’s current working directory, including changes made with `cd`. Workspaces whose shell has exited use their launch directory. New workspace accepts an existing or new directory and an optional separate name. Missing folders are created on submission. `~/` expands to your home directory; relative paths resolve against the application's starting directory (your home directory when started at `/`). The resolved path is shown before creation. A leading `/` denotes an absolute path: for a missing root-level folder, the form offers an explicit home-directory alternative. Read-only and permission failures include recovery guidance. Enter in either field creates the workspace. Escape or a click outside the command center closes it.

Closing a terminal asks for confirmation. Check “Don’t ask again for any terminal” and confirm to skip future prompts across all workspaces, including after restarting Tessera. Re-enable prompts with “Confirm before stopping terminals” in **Settings**. Ordinary shell exit (including Ctrl+D at an empty shell prompt) closes the pane without confirmation; Claude terminals opened by **Claude in folder** retain their output until closed explicitly. Stopping or exiting the last ordinary shell pane removes its workspace; work context and session history are retained. Renaming selects the current name; Enter saves and Escape cancels. The rename dialog rejects blank names.

In Work, Up/Down select an item, Enter opens its actionable terminal when available, and Escape returns. Wide layouts show work and details side by side; smaller layouts switch between the list and its details. Filters keep the selected context within the visible results. These keys remain normal terminal input in a pane. Drag split separators to resize. Drag terminal text to select immediately, including inside Codex; no mode change or modifier is needed. Cmd+C copies, Cmd+V pastes. Right-click a selection to copy it, paste it into the current terminal, open it in a new terminal tab, or search it in the default browser. Terminal actions paste as one line without submitting; a new tab uses the source terminal’s current directory. Option/Alt-click forwards mouse clicks to applications that request them. Shift bypasses terminal mouse reporting for native scrollback. Trackpad and wheel gestures scroll terminal history or are forwarded to applications that request mouse input; alternate-screen programs that enable alternate scrolling receive arrow input. Change terminal text size and Light/Dark appearance in Settings. Right-click a workspace in the rail or tab strip for Find, keyboard selection, and stopping its focused pane. The compact bottom bar shows context, Find (or Return to terminal in Work), Claude in folder shortcuts, or an actionable error. Hover actions and workspaces to see their shortcuts. Workspace number shortcuts follow tab position and support the physical number row on layouts such as AZERTY. On the Create page, Enter in either field creates a workspace; on the Commands page, Enter opens the selected match. Invalid directories show an error inside the dialog and keep it open for correction.

Double-click terminal text to select the full token between spaces or tabs, including punctuation in paths and URLs. Selection follows wrapped lines and includes wide and combining characters.

Normal terminal typing uses a slim beam caret; applications can request block, underline, beam or hidden cursors. Terminal color queries report the actual palette and application overrides, so dark-theme detection uses the real background.

**Terminal search:** use Command+F (Ctrl+Alt+F on Linux) or Find in the workspace tab’s context menu. Search is literal and case-sensitive across the current screen and retained scrollback. Enter / Shift+Enter and Next / Previous navigate matches, wrapping at the ends; Escape closes search and restores terminal input. Only the current match is highlighted. Output changes or resizing clear the highlight; press Enter to search again. Queries stay in memory and are never sent to the shell or saved. Search follows Alacritty’s base-cell text semantics: wide characters and wrapped lines work, but combining marks are omitted and text hidden in the other terminal screen is excluded.

**Keyboard selection:** Command+Shift+Space (Ctrl+Alt+Shift+Space on Linux) or Select in the workspace tab’s context menu enters copy mode. Arrows (or H/J/K/L), Home/End and Page Up/Down move the selection cursor; Ctrl+Left/Right move by word. Shift extends a selection, and Space or V toggles selection at the cursor. The normal copy shortcut copies without leaving; Enter copies and returns to the live terminal, while Escape cancels. Typed text, paste and IME commits are suppressed during copy mode. Selection uses the active screen and retained scrollback, including after the shell exits. Opening Find leaves copy mode; entering copy mode closes Find.

**File drops:** drop a saved screenshot or other file onto a running terminal pane to paste its escaped path. Codex recognizes image paths as attachments in its prompt; Enter remains explicit submission. Close Find or copy mode first. If a screenshot preview does not provide a file path, save it and drop the saved file. Tessera does not upload or retain dropped image contents.

## Recovery and current boundaries

Startup restores saved workspace tabs, splits, and windows. Ordinary terminals reopen as fresh login shells; agent panes resume only their verified saved conversation identities. With no saved workspace, Tessera opens a fresh shell in the application's starting directory, using your home directory when that directory is the filesystem root. Themes, font sizes, terminal preferences, work items, specifications and bounded session histories are saved locally. The saved Light/Dark appearance applies from the first frame, independently of the system theme. Previous sessions are marked disconnected unless already ended; their work context and histories remain available in Work. Closing a pane stops its shell; quitting the app stops hosted shells. Independently detached `nohup`/daemon processes are outside this lifecycle.

Terminal windows share one application process. There is no external session daemon, remote SSH tracking, or issue-tracker import. The optional GitHub Autobot page is described below. Moving a tab to a new window preserves its live terminals within that process. Quitting saves the layout and bounded rendered terminal output. Reopening restores fresh shells or verified agent conversations, without replaying arbitrary commands. Hook histories contain event/tool names, not prompts, arguments, results, transcripts, or permission decisions. Remote-run pane output is excluded from saved snapshots. The terminal renderer is an initial implementation; see [compatibility](terminal-compatibility.md) before treating it as a replacement for a mature terminal.

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

## Merge Radar

Open **Merge Radar** in the top navigation or Commands. The native page follows the queues in the standalone Merge Radar project, using your installed, authenticated GitHub CLI (`gh auth login`). It works independently of Autobot configuration and needs no local web server.

- **Needs my review** collects direct review requests, or reviews requested from you and your configured teams in selected repositories.
- **Needs me** surfaces conflicts, failed checks, unresolved review threads, and requested changes.
- **Waiting on others** shows pending checks, required reviews, and unconfirmed merge readiness.
- **Ready to merge** means no known blockers in the fetched data. Confirm current repository rules and checks on GitHub before merging.
- **Drafts** and **Snoozed** stay collapsed. Bot-authored PRs have a separate section; human-authored PRs include those assigned to you.

PRs appear once, with review requests taking priority over authored/assigned grouping. Click a title, blocker, **Review changes**, **Open failing check**, or **View feedback** to open GitHub. **Copy review request** and lane-level **Copy links** copy text locally. **Snooze 24h** hides a PR from active counts and lanes across Tessera windows and restarts; **Restore** brings it back early. Snoozes expire automatically while the page is open. No action writes to GitHub or sends a message.

**Open Claude** opens the folder picker for a PR. The path is filled from the most recently used existing folder with the same name as the repository. If none matches, the picker searches by repository name. Confirm the folder to start Claude Code in a new terminal tab. Matching uses folder names, not Git remotes, and does not check out the PR branch or send a prompt.

**Radar settings** saves the CLI path, title keyword, additional GitHub search filters, ignored check-name fragments, section order, idle threshold, and refresh interval. Direct-review search uses the same filters as authored/assigned PRs. Repository mode uses its own comma-separated `owner/repository` and optional `owner/team` scope, excluded title prefixes, and maximum age in complete days since creation. It excludes your own PRs and draft review requests. Settings and snoozes are stored under `merge_radar` in Tessera's workspace file. Standalone Merge Radar settings are not imported automatically.

The page refreshes when first opened, manually, and every three minutes by default while visible. Opening another page stops scheduled refreshes; an existing request can finish. Errors preserve the last successful results and display a warning. Changing the scope clears old results and rejects any refresh still using the previous settings. PR content stays in memory; only settings and snooze URLs/expiry times are saved. Background review notifications are not included.

Search paginates in batches of 50 and requires narrower filters above 1,000 results per scope. Checks and review threads are limited to 100 each per PR; truncated or unknown readiness stays out of **Ready to merge**. Large repository scopes require more requests and may take longer to refresh.

## Autobot

Autobot follows GitHub Actions runs for configured sources. It appears alongside Terminal and Work when at least one source is configured and the feature is enabled. A bot means a configured source: Tessera does not assume any particular account, label, workflow, or naming convention. Each ticket is matched against every applicable source. Tessera uses your installed and authenticated `gh` command; credentials stay with GitHub CLI. Jira and issue APIs are outside this feature.

### Set up sources

Open **Settings**, then **Configure Autobot sources** under **AUTOBOT SOURCES**. Configure the GitHub CLI path, run lookup limit, and sources with their names, repositories, ticket patterns, run title patterns, optional event/PR-label filters, output selectors, and run-kind rules. Use **Matching and output options** for advanced filters and output choices. **Save Autobot sources** validates before changing configuration and preserves unrelated preferences and unknown configuration fields. **Cancel source edits** discards the draft. If another window saved configuration during editing, reopen the editor before saving. Configuration errors stay in Settings and Autobot without preventing Terminal or Work from opening. For malformed saved configuration, the editor retains readable fields and warns that saving replaces malformed fields with the form values; cancelling keeps the original configuration. Disabling the feature or removing all sources hides the page.

For example, a source can watch `acme/api` and `acme/web` for tickets matching `ACME-\d+`. The ticket pattern must match the complete key. The run title pattern must include `{key}`, which is replaced by the escaped ticket key. An empty event list includes any event. Run-kind rules apply in order; the first matching title expression supplies the displayed kind. The fictional [remote-runs.json](examples/remote-runs.json) shows every configuration field.

Configuration still uses the top-level `remote_runs` object in `workspace.json`: `~/Library/Application Support/Tessera/workspace.json` on macOS or `~/.config/tessera/workspace.json` on Linux. `TESSERA_STATE_PATH` can override that path for isolated development. Routine setup uses Settings. If you edit the file manually, quit Tessera first and preserve its other fields.

### Follow tickets and inspect runs

Open **Autobot** and add a ticket key such as `ACME-42`. Follow up to 32 ticket keys together; remove a key to stop tracking it. Work items retain their optional **Ticket key** field and can open Autobot with that ticket selected. Tracked keys and tracking preferences survive restart. Fetched run and PR metadata stay in memory across page navigation and clear on exit.

The overview groups results by source and ticket, with repository, latest run kind, GitHub status/conclusion, elapsed time, last successful refresh, and linked pull requests. Elapsed time uses GitHub's start time; for completed runs, the endpoint is the last update time and is labelled accordingly. Missing timestamps show unavailable elapsed time. Failed runs and actionable errors get priority. Source/repository errors remain independent, so one inaccessible repository does not hide the others. A queued or running workflow is different from a completed workflow; a completed run never means the task is accepted, merged, or waiting for a person.

Select an overview item to inspect its newest-first run timeline. Active runs show available job/step metadata and completed steps out of the known total. These counts describe workflow steps, not a completion percentage or an agent's reasoning. Browser links open the original run or pull request for more context.

Lookup is bounded: `run_limit` defaults to 30 per configured event, with a maximum of 1,000. Pull request lookup scans the latest 1,000 PRs in each repository and matches the ticket in the title or branch. A separate title search can retain older title matches. Older branch-only matches can be missed; reaching the scan limit shows a warning. Within a ticket, repeated matches for the same repository/run are shown once under the first matching source in configuration order. Other matching sources link to that group with **See shared runs**; their repository errors and PR links remain independent.

### Refresh and stale data

**Refresh** updates tracked tickets in one cycle. **Live refresh** is optional and off by default. When enabled, it runs only while the page is visible, 60 seconds after a successful cycle. If every metadata stream fails, consecutive failures extend the delay to 120, 240, 480, then at most 600 seconds. A healthy stream keeps the regular refresh interval for the other data. Refresh cycles do not overlap. Leaving the page cancels queued metadata requests and stops running GitHub CLI lookups. Obsolete results are discarded. All GitHub metadata requests share a limit of four concurrent CLI calls.

Refresh preserves the selected item. If a source/repository refresh fails, its prior metadata remains visible as stale, with the last successful refresh timestamp and the current error. Run and PR freshness are tracked separately, so a failed PR query does not make successfully refreshed runs stale. A stale running row is an old observation, not proof that the run is still active. Lookup-limit warnings describe bounded coverage without marking a successful refresh stale or delaying the next refresh. Retry with **Refresh** after correcting authentication, permissions, configuration, or network problems.

### Output and pull request comments

Completed runs retain **Full log** and **Output…** actions. Output opens job/step selectors with configured choices preselected when they match. `output_job` matches a substring of the job name; `output_step` matches the step name exactly. Choose **Open selected output** to open that selection. A missing configured job or step shows a warning and lets you choose another job or the full log. **Full log** uses `PAGER` when set, or `less -R` by default. Active runs offer **Watch run** in a terminal pane.

Remote output never enters saved terminal snapshots. GitHub log caches use private temporary directories cleaned up on command exit, pane closure, or launch failure. Closing a remote terminal discards its retained output. Metadata and tracking views do not store full logs.

The optional PR comment editor keeps the full body in memory. Choose **Preview comment** to inspect the target repository, pull request, full body, and warnings, then separately choose **Confirm and post comment** to publish. Posting can start a paid agent run, create commits, or cause a merge when auto-merge is armed. The preview warns about armed auto-merge and closed PRs. Leaving Autobot discards unposted drafts and previews. Comment bodies use private temporary files removed after normal success or failure; normal exit waits for any confirmed post or pending log launch to finish cleanup. An in-flight GitHub CLI post can delay exit by up to its 20-second timeout. Force-quitting during posting can leave a temporary file behind. Tessera refreshes after a successful post.

GitHub CLI requests reuse the login-shell environment resolved once per process. Run and PR list responses stay in memory for 45 seconds. Each refresh bypasses data from earlier cycles; tickets in the same cycle share identical requests, including concurrent requests. Configured lookup limits remain unchanged. Secondary rate limits and HTTP 429 responses stop new requests for at least 60 seconds, without automatic retries. Primary quota failures remain visible without pausing unrelated requests.

Merge Radar searches review requests on GitHub instead of downloading every open PR. Repository review cards retain checks, conflicts, and unresolved threads. Their overall review decision and branch readiness remain unconfirmed unless the same PR was also loaded as assigned or authored; those calculations can exceed GitHub's resource limits on large repositories. Failed scopes appear alongside successful results. A complete refresh failure retains the previous result.

Start Tessera with `RUST_LOG=tessera=debug` to print CLI-call and refresh timings to stderr. Merge Radar queries always fetch fresh data.

### GitHub CLI diagnostics

Request errors identify the source, repository, and command. Rate limits, GitHub server errors, SSO authorization, and incompatible CLI versions have specific recovery messages. Unclassified failures include the exit code and up to 200 characters from the first useful stderr line, with shell noise removed and GitHub token prefixes redacted. Other repositories and sources continue to render.

Failed CLI requests save command arguments, the exit code, and full token-redacted stderr in private diagnostic files:

- macOS: `~/Library/Logs/Tessera/remote-runs/`
- Linux: `~/.local/state/tessera/remote-runs/`

Attach the relevant `.log` files to a bug report. Successful requests discard their diagnostic files. Logging is best effort if the directory cannot be written. Diagnostic files do not contain workflow log output or comment bodies; review stderr before sharing because it may contain repository names or other account details. You can delete these files after troubleshooting.

Autobot cannot approve or merge a pull request or change its auto-merge settings. A comment is an explicit external action, even when the text looks like an ordinary conversation reply.
