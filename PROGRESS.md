# Progress

2026-10-04 — agent discovery and 0.3.1 release preparation.

Overview now discovers Codex and Claude executable processes descended from live pane shells in a background worker at most every two seconds. It reads only process IDs, parent IDs and executable names. Discovered rows remain Untracked until lifecycle hooks upgrade their identity/state in place, preserving selection. Only successfully observed process disappearance ends an untracked row; failed scans preserve state. Discovery is local, does not restore terminals, and does not infer turn or approval state.

The user's installed Codex 0.160.0 was reproduced with a private socket and app-server probe: SessionStart and UserPromptSubmit reached Tessera's helper when the first turn started, but idle CLI startup emitted neither. A shared Codex daemon also retained an expired Tessera socket. Documentation now recommends pane-local `codex --no-daemon`. No user agent was stopped; two current processes were registered as neutral Untracked observations in the running app.

Package/lockfile and release instructions are prepared for 0.3.1. Release notes include the merged workspace-directory and startup-theme/status-shortcut fixes since 0.3.0. No tag or release is published by this draft PR. Regression coverage includes process ancestry/cycles, deduplication, exit handling and first-hook selection-preserving upgrades. Validation before the version bump: 80 Rust tests including sockets, 14 Python tests, formatting, Clippy with warnings denied and release build passed using the installed macOS 26.5 SDK because the default 27.0 SDK is incompatible with the current linker. Native UI/VoiceOver and installed-updater acceptance remain pending.

2026-10-03 — first terminal/Overview increment.

The initial repository contained LICENSE and .gitignore only. Implemented a Rust desktop application using eframe/egui, Alacritty terminal state, and portable-pty. No production demo/fake session data is enabled. Run instructions are in README.md.

Presentation: reusable 2172 × 724 Tessera mosaic banner added at the top of README, with the same PNG and crop guidance in assets/ for a portfolio project card.

Implemented: login-shell PTYs, tabs, both split orientations/resizing, focus routing, maximize/restore, workspace picker, light/dark chrome and font size, structured Claude hook adapter and local socket, bounded session histories, explicit local task review/acceptance, session filters, virtualized Overview, atomic metadata persistence, fresh terminal startup, helper install/preview/uninstall, and separate Intel/Apple Silicon CI bundles.

Portable validation: 14 passing tests locally (one socket test reserved for unrestricted hosts), Rust check/build, formatting, Clippy, tests including a real shell with PTY resize and terminal-protocol reply, parser state, session transitions/deduplication/reordering, preserving unrelated hooks, split identities, and headless Overview geometry/rendering in both appearances/narrow width. A 100-session synthetic workload exists in test code only. See docs/performance.md for measurements and limits.

Local environment limitation: no macOS window server; local socket bind is denied in the execution sandbox, so Xvfb/interactive desktop and endpoint round-trip could not run here. PTY operations work and were exercised. Socket round-trip is an ignored test run explicitly by unrestricted CI. macOS CI compiles the actual application, exercises PTYs and the socket transport, and produces architecture-specific bundles. Interactive native GUI/Claude/IME/VoiceOver validation remains unperformed. Do not count a Linux/headless result as native acceptance.

Current architecture hosts PTYs in worker threads in the app process. UI navigation preserves them; app exit does not. Preferences/history restore is implemented; previous workspace layouts are discarded on startup; detached live-process recovery is planned. Specs/connector/review-evidence milestones remain planned, not partially represented by fake views.

Resume work: inspect CI, install the Intel app on the user's Mac, run the native checklist in docs/terminal-compatibility.md, fix functional blockers before starting the local specification board. Keep TODO.md and this file current per increment.

2026-10-03 — second increment: Codex integration.

Added Codex lifecycle hooks and preview/install/uninstall commands, respecting CODEX_HOME and preserving existing JSON handlers plus config.toml. Hooks require the user's review/trust via Codex /hooks. Metadata-only transport shares existing bounds. Provider-aware identity keeps same-ID Claude/Codex histories separate; missing provider fields restore as Claude with unchanged selection keys. Mixed-agent Overview labels each card. Interrupt marks attention; task acceptance remains explicit. CI push trigger follows the renamed main branch.

Validation adds provider transitions/history migration, Codex metadata privacy and malformed/oversized rejection, installer preservation/idempotence/exact removal/symlink refusal, and mixed-provider Overview fixtures. Local validation: 18 tests pass; two socket tests are reserved for unrestricted CI (20 total), formatting, Clippy with warnings denied, and build pass. No real inference session is launched. Native interactive Mac/Codex validation remains outstanding.

2026-10-03 — Mosaic application icon.

Selected Mosaic artwork exported as a 1024px transparent PNG and a 256px native viewport icon. macOS bundles generate the full 1×/2× ICNS family and declare CFBundleIconFile, verify icon decoding and bundle signatures, then archive. The same asset supports Intel and Apple Silicon. This increment is independent of the Codex integration PR. CI push follows the repository's renamed main branch.

Validation: source PNG dimensions/alpha and small-size visual inspection, shell syntax, Rust formatting/build/Clippy, existing tests, and native ICNS/bundle checks in macOS CI. Interactive Dock/Finder appearance remains to be checked on a physical Mac.

2026-10-03 — Release DMG packaging.

The existing macOS packaging script now stages the signed release application and an Applications shortcut in a compressed, checksum-verified DMG. CI uploads separate Intel and Apple Silicon DMGs instead of application ZIPs. The same script builds local DMGs without additional packaging dependencies.

2026-10-03 — GitHub Release publishing.

Version tag pushes now run the existing Linux/macOS verification matrix, then publish both architecture DMGs to GitHub Releases. Only the publishing job receives contents write permission. It verifies both artifacts exist, creates a draft with generated notes, uploads both files, and publishes; reruns can resume a partially completed release. Branch and pull-request builds remain artifact-only. README includes the tag/push commands. Actual release publication requires pushing a version tag containing this workflow change.

2026-10-03 — Pane-local terminal search.

Implemented literal, case-sensitive search across each pane's active screen and retained scrollback. Cmd+F (Ctrl+Alt+F on Linux) and Find open compact controls; Enter/Shift+Enter or Next/Previous navigate with wraparound, highlight the current match and scroll it into view. Escape returns keyboard input to the terminal. Queries are bounded to 256 characters, held only in memory and never sent to the shell or persisted.

Search uses the existing Alacritty engine on a background worker, without copying history. Only one search runs per pane at a time; new requests coalesce and obsolete query results are discarded. Output and resize revisions invalidate match coordinates, including changes between worker dispatch and execution. Enter searches again after output changes. Wide text and soft wrapping work; Alacritty's search omits combining marks and excludes the hidden primary/alternate screen. Native terminal search and accessibility acceptance remain outstanding.

Validation: all 26 tests pass including the normally ignored Unix socket round trips. New coverage includes literal/case-sensitive navigation and wraparound, scrollback/wide/wrapped text, alternate-screen isolation, empty/missing/stale results, outdated worker completion and disconnect, and controls at 180/320/640px in both themes. The real-shell input test verifies search cannot execute typed commands and Escape restores terminal input. Formatting and Clippy with warnings denied pass. macOS debug build verified; no native interactive GUI acceptance is claimed.


2026-10-03 — Keyboard selection and native feedback fixes.

Added pane-local copy mode using Alacritty's vi cursor/selection engine. Cmd+Shift+Space (Ctrl+Alt+Shift+Space on Linux) or Select enters it; arrows/HJKL, Home/End, Page Up/Down and Ctrl+Left/Right navigate; Shift extends, Space/V toggles selection, the normal copy shortcut copies, Enter copies and returns, and Escape cancels. The shortcut avoids egui-winit's interception of Cmd+Shift+C as a clipboard event. Actions share the existing bounded terminal control queue. Text, paste and IME commits are suppressed throughout copy mode, including the frame that exits it. Retained output remains selectable after shell exit. Find and copy mode are mutually exclusive.

Addressed user screenshots: default typing caret is a slim beam; requested block/underline/beam/hidden shapes and cursor colors are rendered. Fixed OSC 10/11 palette replies: background index 257 was incorrectly clamped to near-white index 255, causing dark terminal clients such as Codex to infer a light background. Queries now resolve actual default or application-set colors after parsing without recursive locking. Fixed Shift+Cmd+D being swallowed by the less-specific side-by-side shortcut. Workspace number shortcuts use physical digit positions as well as logical digits, independently of task titles, and close the picker/reset maximize when switching. Enter in directory/filter fields creates or opens the first matching workspace; invalid directories retain the dialog and create no pane. Toolbar/pane/tab hover tips expose platform-appropriate shortcuts without missing shift glyphs. Buttons use 6px corners and modest padding in both appearances.

Validation: 35 tests pass including normally ignored Unix socket tests. Added coverage for selection of wide/combining/wrapped text, paging bounds, post-exit clipboard commands, text/paste/IME input isolation under grid contention, color query responses and overrides, cursor shape requests/geometry, physical-key/custom-title switching, stacked split identity, and successful/invalid directory plus filter Enter submission. Formatting, Clippy with warnings denied and debug build pass. Physical keyboard, native GUI/Codex contrast, cursor blinking and VoiceOver acceptance remain outstanding.

2026-10-03 — 0.2.0 release preparation.

Package and lockfile are 0.2.0. CLI help/version and macOS bundle release metadata derive from Cargo; bundle build numbering preserves an increasing value from the original build 1. Added CLI version regression and checked bundle plist metadata. The release workflow verifies tag/package agreement, uses the checked-in 0.2.0 notes, and publishes only after the Linux/Intel/Apple Silicon matrix and both DMG builds succeed. All terminal search/selection increments and screenshot fixes remain together on feat/terminal-search. Local release checks: 36 tests including socket round trips, formatting, Clippy with warnings denied, build, bundle/workflow shell syntax and plist assertions pass. Native interactive acceptance remains outstanding.

2026-10-03 — Terminal close shortcut and workspace cleanup.

Changed Stop terminal to Cmd+W (Ctrl+Alt+W on Linux), including hover help and README. The existing stop confirmation remains. Closing a single-pane workspace now removes the workspace instead of leaving a resumable placeholder. Split panes retain their surviving layout and valid focus; tab selection stays valid after removal, maximize resets, and retained sessions immediately become disconnected. The history inspector explains when a terminal has been removed instead of suggesting resuming a deleted workspace.

Validation: 39 tests pass, with two existing Unix socket tests ignored under the default test command. Four close regressions cover the shortcut/confirmation boundary, real-shell shutdown and saved workspace removal, tab selection after removal, and nested split collapse/focus. Formatting, Clippy with warnings denied, debug build and diff checks pass. Native interactive shortcut acceptance remains unperformed.

Workspace dialog follow-up: Enter submission is captured from the focused directory/filter before rendering other views, rather than relying on a text field losing focus later in the frame. Invalid-directory errors are displayed inside the dialog. Added a regression with live terminals and Overview under the dialog, including invalid-directory retry and successful creation. Native interactive Enter acceptance remains unperformed.

2026-10-03 — 0.2.1 fix release preparation.

Package and lockfile are 0.2.1. Added focused release notes for Cmd+W, removal of the last-pane workspace, dialog Enter submission and inline invalid-directory feedback. README release commands and current-version description match. The existing tag workflow validates Linux and both Mac architectures before publishing their DMGs.

Release validation: all 41 tests pass including both Unix socket round trips when run outside the sandbox. Formatting, Clippy with warnings denied, debug build, bundle script syntax and diff checks pass. Native interactive acceptance remains outstanding.

2026-10-03 — Immediate terminal stop.

Removed stop confirmation following user feedback. Cmd+W immediately stops the focused terminal and removes its pane/workspace; the × button performs the same action in the current frame after pane rendering. No persistent pending-close state or confirmation window remains. Task/session history retention is unchanged. Shortcut regression now asserts immediate removal and disconnected history.


2026-10-03 — Workspace rename and terminal close preference.

Cmd+Shift+R (Ctrl+Alt+Shift+R on Linux) renames the active workspace through a compact modal, selects the existing name, saves with Enter, and cancels with Escape. Blank names are rejected, surrounding whitespace is trimmed, and the existing workspace persistence stores changes. Workspace & commands also exposes Rename workspace. Dialog identity uses task UUIDs so a rename cannot target another tab accidentally; modal input is withheld from the terminal.

Cmd+W and the pane close button now share a confirmation with “Don’t ask again for any terminal.” Confirming with that checkbox stores the preference globally across workspaces and restarts; cancellation leaves both the terminal and preference unchanged. Existing state files default to confirmation. “Confirm before stopping terminals” in Workspace & commands can restore prompts. Last-pane workspace removal and retained session history use the existing close path.


2026-10-03 — Trackpad and application wheel scrolling.

Terminal scrolling now accumulates fractional trackpad deltas instead of rounding each frame to zero. Mouse-reporting applications receive SGR or legacy wheel events at the pointer's terminal coordinates; alternate-screen applications with alternate-scroll enabled receive cursor keys. Shift and copy mode keep scrolling native, and exited shells retain scrollback. Wheel input is batched and bounded per frame. Native scrollback operations use the existing control queue while a shell is live, avoiding dropped movement when the parser holds the terminal lock, and request a repaint.

Regressions cover fractional movement, mode transitions, mouse protocol coordinates/modifiers, native versus alternate-screen routing, and a real PTY receiving a wheel event from four small gesture frames. Native macOS trackpad acceptance with Codex remains to be performed with the rebuilt application.


2026-10-03 — 0.2.2 release and native automatic updates.

Added Sparkle 2.10.0 with a pinned framework archive checksum, bundled native loading, automatic launch/daily checks and background downloads, install-on-quit behavior, manual checks and Sparkle-owned persisted preferences. Plain Cargo and non-macOS builds do not start an updater. Architecture-specific signed feeds and signed DMGs are published atomically within the existing draft release flow. Feeds require Ed25519 verification without expiry; archives require verification before extraction. Release signing is gated on version tags and requires SPARKLE_ED25519_PRIVATE_KEY, matched to the committed public key. The local private key stays in the macOS login Keychain. Current bundles remain ad hoc signed; initial manual installation/approval and later native update acceptance are documented.

Bumped package/lockfile to 0.2.2 and added release notes covering automatic updates, rename, close preference and trackpad scrolling. Updated release setup and terminal compatibility documentation.

Validation: all 50 Rust tests pass including Unix socket and real PTY cases; the portable appcast test passes. Formatting, Clippy with warnings denied, debug/release builds, shell syntax and diff checks pass. The Intel DMG, nested application signature and signed archive/feed verify. The bundled framework loads and exports the native updater class. Both Keychain and CI-secret signing paths pass, and a tampered archive is rejected. Apple Silicon/Linux CI and end-to-end native updater acceptance remain pending. Repository-publication authorization was given so release feeds can be fetched without credentials.


2026-10-04 — terminal input visibility and Codex question clicks.

Keyboard input and paste now return to the bottom through the existing terminal control queue before writing, even under parser contention. Mouse/wheel and protocol responses preserve scrollback. Added an opt-in persisted “Clickable Codex questions” preference, recognizing the visible numbered question prompt and sending only arrow keys to select an option. Enter remains explicit confirmation. Native mouse reporting, copy/search/history, modifiers and stale/dragged clicks are excluded; parsing is cached by terminal output revision. Codex 0.160.0 prompt layout is the compatibility target. Native acceptance with a live Codex question remains pending.

Validation: all 54 Rust tests pass, including a real PTY receiving an arrow from a UI click and typing/paste returning from scrollback under lock contention. Formatting, Clippy with warnings denied, release build, appcast test, shell syntax and diff checks pass.


2026-10-04 — new terminals inherit the focused shell directory.

Cmd+N and Cmd+T now open a workspace using the focused shell's live working directory rather than the workspace dialog's last configured directory. Reads use the child shell PID with macOS proc_pidinfo or Linux /proc, without injecting commands or changing shell configuration. Restored/exited shells use their saved/launch directory; live lookup errors are shown instead of silently opening elsewhere. The workspace dialog still honors its explicit directory. Regression covers a focused split pane after cd into a directory with spaces and Unicode and verifies the new real shell directory.

Validation: all 55 Rust tests pass, including the real-shell Cmd+N regression; formatting, Clippy with warnings denied, release build, appcast test, shell syntax and diff checks pass locally. Linux/Apple Silicon CI remains pending.


2026-10-04 — 0.2.2 published; 0.2.3 cursor rendering.

Published v0.2.2 at a77e1f4 with Intel/Apple Silicon DMGs and signed architecture feeds after all three CI jobs passed. Repository is public, signing secret is configured, and anonymous latest-feed/archive downloads and signatures verify for both architectures. No private key remains in a temporary export.

0.2.3 uses opaque block cursors with contrasting glyph repaint, wide-character width, terminal clipping and visible-row guards. Search and copy-mode transitions suppress conflicting cursor rendering. Application shape/visibility requests remain authoritative; blink remains steady. Added rendering and fragmented-protocol regressions, and bundled Sparkle's license/notices. Native Vim PTY mode requests checked; interactive visual acceptance and a real installed 0.2.2 → 0.2.3 update remain pending.

Validation: 57 Rust tests including real PTY/Unix socket cases pass. Formatting, Clippy with warnings denied, release build, appcast test, shell syntax and diff checks pass. Local signed Intel packaging verifies and includes Sparkle notices. Native visual and installed update acceptance remain pending.

2026-10-04 — 0.2.4 file drops and update diagnostics.

0.2.3 was published from main by another session while these fixes were being prepared. Merged main into the existing release branch, preserving its cursor fixes and published release notes. Package and lockfile are 0.2.4; its release contains the following additions.

Missing update controls follow-up: Workspace & commands now scrolls and retains updater startup failures independently of general terminal errors, with a disabled check button and the failure reason. Cargo/non-macOS builds explain the application-bundle requirement. The installed 0.2.2 bundle contains Sparkle and its feed metadata and passes signature verification; its running updater state has not been observed. A regression verifies the diagnostic survives unrelated errors. Validation: 58 Rust tests including Unix socket round trips pass outside the sandbox; formatting, Clippy with warnings denied, release build, appcast test, shell syntax and diff checks pass. Native GUI and installed update acceptance remain pending.

Screenshot/file-drop follow-up: file drops now paste escaped paths into the pane under the pointer, or the focused pane when no pointer location is supplied. Multiple paths are individually bracketed for Codex image recognition and queued together; no Enter is added. Search/copy mode and dialogs block input. Missing paths explain how to save and drop a screenshot; control characters, non-UTF-8 paths and oversized input are rejected. No image contents are read, uploaded or persisted by Tessera. Regression coverage includes quoting, paste framing, limits and a real PTY receiving a drop in an unfocused split while search prevents input. Final validation: all 60 Rust tests including Unix socket tests pass, as do formatting, Clippy with warnings denied, release build, appcast test, shell syntax and diff checks. Native screenshot-preview and updater acceptance remain pending.

2026-10-04 — 0.2.5 terminal and workspace lifecycle fixes.

Every launch discards previous tabs/splits and opens one fresh login shell in the application’s starting directory. Workspace resume is removed. Preferences and bounded session histories remain persisted; previous non-ended sessions become disconnected. Legacy state files remain readable, including layouts with missing directories. Workspace creation has a separate optional name; Enter submits either field, blank names use the directory name, and invalid directories preserve input for retry.

New tabs and both split directions share the focused shell’s current-directory lookup, including after `cd`. Lookup failures report an error without changing the layout. Confirmed shell exits automatically remove their pane and the final pane’s workspace, without a stop confirmation. Surviving splits/tabs keep valid focus and session history remains. Stale close/rename dialogs are cleared; PTY read failures alone do not close a pane.

A bundled Noto Sans Symbols 2 fallback covers `✗`, related crosses/checkmarks and the supplied shell prompt without changing normal Hack text or cell metrics. The font’s OFL license is included in the repository and application bundle. Workspace & commands is a scrollable modal dismissed by Escape or backdrop clicks; opening and inside clicks keep it open. Escape is consumed before Overview or terminal input. Package/lockfile, README and release notes are 0.2.5.

Validation: all 67 Rust tests pass, including Unix socket tests outside the sandbox. New regressions cover startup/history/preferences, invalid-directory recovery, named workspaces, both split directions after `cd`, exact prompt glyph coverage/cell metrics, Ctrl+D split/last-pane cleanup, and panel dismissal in both themes. Formatting, Clippy with warnings denied, release build, Python appcast test, shell syntax and diff checks pass. The local Intel macOS app/DMG passes signature/checksum and 0.2.5/1.2.5 bundle metadata checks; the symbol font license is packaged. Native interactive acceptance remains pending.

2026-10-04 — compact terminal chrome and readable shortcuts.

Removed the permanent per-pane Terminal/Select/Find/close row. Find, keyboard selection and stopping the focused pane remain available through shortcuts and workspace-tab context menus. Tabs have wider spacing, padded targets, muted numeric shortcuts and a mint active underline in both themes. The bottom shortcut bar uses a distinct surface and readable action/key pairs that wrap together at the minimum window width. Errors retain a dismiss action and theme-appropriate contrast. The existing render harness also exercises active terminal tabs in light and narrow dark views.

Validation: formatting, Clippy with warnings denied, the full Rust suite including socket tests outside the sandbox, debug build, Python appcast test, shell syntax and diff checks pass. Reviewed rendered light/dark and 640-point layouts; native interactive acceptance remains pending. The installed macOS 27 SDK is incompatible with the available linker, so local checks use the installed macOS 15.4 SDK.

2026-10-04 — immediate terminal selection and text actions.

Primary drags select terminal text directly, including Codex's mouse-reporting screen, without Shift or keyboard copy mode. Selection anchors at the original press; text hover uses the text cursor. Option/Alt-click remains available for application mouse input, and wheel/trackpad routing is unchanged.

Right-click a selection for Copy, Paste into this terminal, Open in new terminal tab, or Search in browser. Actions retain a snapshot while output changes. Browser searches use a percent-encoded Google query in the default browser. Terminal actions flatten line breaks, reject control characters/oversized input, and add no Enter; new tabs inherit the source terminal's current directory. Paste is disabled while Find/copy mode is active or the shell has exited.

Validation: all 71 Rust tests pass, including local PTYs and Unix sockets. Regressions cover ordinary drag/copy without application input leakage, press anchoring and modifier changes, Option/Alt mouse input, context-menu actions at 640px, selection snapshots, encoded Unicode searches, safe paste limits, and source-directory new tabs without command execution. Formatting, Clippy with warnings denied, native debug build, Python appcast test, shell syntax and diff checks pass. Local compilation uses the installed macOS 15.4 SDK because the default 27.0 SDK is incompatible with the installed linker. Native interactive acceptance remains pending.

Conflict follow-up: merged the compact workspace chrome from main, preserving the new tab action dispatch and all selection actions. Updated Find/Select documentation to point to workspace-tab context menus.

2026-10-04 — 0.2.6 release preparation.

Package and lockfile are 0.2.6. Release notes cover the merged compact terminal chrome and direct selection/text actions; README version and publishing commands match. No runtime behavior changes are introduced by the release bump.

Validation: all 71 Rust tests pass, including PTY and Unix socket cases. Formatting, Clippy with warnings denied, optimized release build, Python appcast test, shell syntax and diff checks pass locally using the installed macOS 15.4 SDK. Main’s Linux/Intel/Apple Silicon CI passed before preparation; the version tag runs the complete verification and signed packaging matrix before publishing. Native interactive acceptance remains pending.


2026-10-04 — one-command release installation.

Added a POSIX `install.sh` published as a GitHub release asset. It resolves the latest tag once (or uses `TESSERA_VERSION`), downloads architecture-specific assets over HTTPS, verifies SHA-256 checksums, and installs without Rust or administrator privileges. macOS installs the complete Sparkle-enabled bundle in `~/Applications` and a stable `~/.local/bin/tessera` symlink. Linux installs a prebuilt executable in `~/.local/bin`. The installer prints PATH setup when needed, refuses unsupported platforms and unrelated symlinks, requires quitting Tessera on macOS, stages replacements, restores the previous app on installation failures, and cleans temporary files and mounted DMGs. It leaves shell configuration and agent hooks untouched.

Release CI now builds Linux x86-64 on Ubuntu 22.04 and ARM64 on Ubuntu 24.04, alongside existing macOS DMGs. Publication includes both Linux archives, the installer, and a SHA-256 manifest before releasing the draft. Quickstart and updater documentation describe installation, updates, runtime dependencies and removal. The public curl command requires a new release with these assets; existing releases do not include them.

Validation: all 14 Python tests pass, including offline fixtures for all four target combinations, repeated installs, pinned releases, checksum/network failures, unsupported targets, symlink rejection, macOS code-signature failures and rollback. ShellCheck, POSIX/Bash syntax and diff checks pass. The committed application source passes formatting, Clippy with warnings denied, all 71 Rust tests including Unix socket cases, and a debug build in an isolated temporary checkout using the macOS 15.4 SDK. Concurrent `src/ui.rs` changes in the shared checkout caused a build failure during validation; those unrelated edits were preserved. Native Linux builds and real release-download/DMG installation remain CI/native acceptance checks.

2026-10-04 — deterministic PTY test fixtures for Ubuntu CI failures.

The reported mouse-input and split-directory tests sent commands to newly launched, runner-configured login shells before confirming readiness. Reproduced UTF-8 directory-input corruption with a shell line editor in the C locale. PTY fixtures now use Bash without startup files or line editing, clear inherited environment configuration, disable history persistence, and wait for a fixed first prompt before returning. UI-created test panes, search fixtures and terminal regressions use the same fixture; production terminals retain their configured login shell and inherited environment. The shared PTY/parser/control-worker implementation remains common. Added coverage for immediate Unicode directory changes and pane/socket/terminal environment propagation.

Validation: all 69 Rust unit tests and 3 CLI integration tests pass on the isolated fix branch, including Unix sockets outside the sandbox, under LC_ALL=C with an invalid inherited SHELL. Earlier validation in the shared checkout also passed 30 parallel UI-suite runs with 32 test threads (1,140 test executions). Formatting, Clippy with warnings denied, debug build, all 14 Python tests and diff checks pass locally using the macOS 15.4 SDK. A native Ubuntu CI rerun remains pending. Existing UI and installer work in the checkout was preserved.

2026-10-04 — desktop UI and command center redesign.

Reworked the native egui shell with a slate/mint appearance in both themes, compact Terminal/Overview navigation, a workspace rail above 900 points and scrolling tabs below it, workspace-level split/maximize controls, and a quieter footer. Workspace & commands separates searchable workspace/action navigation, workspace creation, and Settings. Search and navigation hints stay fixed while results scroll; Up/Down wrap, Enter dispatches the selected action, and unavailable pane commands remain disabled. Creation keeps its primary action visible at minimum height, labels both fields, focuses directory input, and retains retry/Enter behavior. Settings groups appearance, terminal behavior, updates and an expandable shortcut reference. Rename and terminal-stop dialogs share the spacing and action treatment.

Overview uses denser virtualized session rows and a readable activity timeline with relative timestamps. Narrow windows switch between Sessions and Activity instead of compressing both. Filters clear unrelated selected context; double-click opens an existing live terminal. Split membership checks avoid allocating pane-ID vectors during session filtering/rendering. No dependencies or persisted domain schema were added; terminal rendering, input, lifecycle, hooks and history remain on the existing paths.

Validation: expanded rendered light/dark, wide/narrow and 640×400 fixtures; regressions cover command search by name/directory, wrapped keyboard navigation, action dispatch, disabled commands, modal geometry with 32 workspaces/long titles, persistent creation action, compact inspection and filter selection. The real-PTY input regression also checks text, paste and IME isolation while command search is open. All 74 Rust tests (including PTY and Unix socket cases), all 14 Python tests, formatting, Clippy with warnings denied, optimized build, script syntax and diff checks pass locally using the installed macOS 15.4 SDK. Rendered screens were visually reviewed in both themes and at minimum window dimensions. Native interactive and assistive-technology acceptance remain pending. Concurrent installer/release changes were preserved.

Branding follow-up: replaced the native icon and README/portfolio banner with a coordinated mint/amber tiled T identity. Both PNG exports preserve icon alpha; macOS packaging derives its complete ICNS family from the 1024px source. The toolbar mark follows the same geometry. Generation prompts and crop guidance are recorded in assets/README.md.

2026-10-04 — 0.3.0 release preparation.

Package and lockfile are 0.3.0. The README now focuses on installation, agent setup, everyday controls and persistence; detailed installation, keyboard/terminal behavior, source builds, checks and publishing instructions live in dedicated guides. Added 0.3.0 release notes covering the UI redesign, coordinated icon/banner and the merged one-command installer. The existing version-tag workflow derives native bundle and update-feed versions from Cargo; no release tag or publication is performed by this draft PR.

Final 0.3.0 validation after incorporating main’s PTY fixture fix: all 75 Rust tests (72 unit and 3 CLI), including Unix sockets, pass with LC_ALL=C and an invalid inherited SHELL. Formatting and Clippy with warnings denied pass. The Apple Silicon release app and DMG build successfully; bundle metadata is 0.3.0 / 1.3.0, the complete ICNS family decodes, the nested code signature verifies, and the DMG checksum is valid. All 14 Python tests, shell syntax, asset dimensions/alpha, documentation links and diff checks pass. ShellCheck is unavailable locally and remains covered by Linux CI. Native interactive, VoiceOver and installed automatic-update acceptance remain pending.

2026-10-04 — create workspaces in new directories.

The explicit New workspace form now creates missing directory trees before starting the shell, trims surrounding path whitespace, and keeps the optional workspace label separate from the directory. Empty input, filesystem failures and workspace limits retain the entered name and leave workspace/pane state intact; a successful retry clears the error. Startup and terminal-derived workspace creation still require existing directories. The form and README describe the new behavior.

Validation: 76 Rust tests pass, including real PTY, Unix socket, keyboard submission, directory creation and retry regressions. Formatting, Clippy with warnings denied, debug build and diff checks pass using the installed macOS 15.4 SDK. Existing rendered geometry checks cover both themes and the minimum 640×400 window. The Unix socket test requires running outside the filesystem sandbox. Native interactive acceptance remains pending.
