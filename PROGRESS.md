# Progress

2026-10-03 — first terminal/Overview increment.

The initial repository contained LICENSE and .gitignore only. Implemented a Rust desktop application using eframe/egui, Alacritty terminal state, and portable-pty. No production demo/fake session data is enabled. Run instructions are in README.md.

Presentation: reusable 2172 × 724 Tessera mosaic banner added at the top of README, with the same PNG and crop guidance in assets/ for a portfolio project card.

Implemented: login-shell PTYs, tabs, both split orientations/resizing, focus routing, maximize/restore, workspace picker, light/dark chrome and font size, structured Claude hook adapter and local socket, bounded session histories, explicit local task review/acceptance, session filters, virtualized Overview, atomic metadata persistence, fresh-shell workspace resume, helper install/preview/uninstall, and separate Intel/Apple Silicon CI bundles.

Portable validation: 14 passing tests locally (one socket test reserved for unrestricted hosts), Rust check/build, formatting, Clippy, tests including a real shell with PTY resize and terminal-protocol reply, parser state, session transitions/deduplication/reordering, preserving unrelated hooks, split identities, and headless Overview geometry/rendering in both appearances/narrow width. A 100-session synthetic workload exists in test code only. See docs/performance.md for measurements and limits.

Local environment limitation: no macOS window server; local socket bind is denied in the execution sandbox, so Xvfb/interactive desktop and endpoint round-trip could not run here. PTY operations work and were exercised. Socket round-trip is an ignored test run explicitly by unrestricted CI. macOS CI compiles the actual application, exercises PTYs and the socket transport, and produces architecture-specific bundles. Interactive native GUI/Claude/IME/VoiceOver validation remains unperformed. Do not count a Linux/headless result as native acceptance.

Current architecture hosts PTYs in worker threads in the app process. UI navigation preserves them; app exit does not. Metadata/history restore is implemented; detached live-process recovery is planned. Specs/connector/review-evidence milestones remain planned, not partially represented by fake views.

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
