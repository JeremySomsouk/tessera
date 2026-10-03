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
