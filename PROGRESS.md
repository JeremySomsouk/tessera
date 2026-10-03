# Progress

2026-10-03 — first terminal/Overview increment.

The initial repository contained LICENSE and .gitignore only. Implemented a Rust desktop application using eframe/egui, Alacritty terminal state, and portable-pty. No production demo/fake session data is enabled. Run instructions are in README.md.

Presentation: reusable 2172 × 724 Tessera mosaic banner added at the top of README, with the same PNG and crop guidance in assets/ for a portfolio project card.

Implemented: login-shell PTYs, tabs, both split orientations/resizing, focus routing, maximize/restore, workspace picker, light/dark chrome and font size, structured Claude hook adapter and local socket, bounded session histories, explicit local task review/acceptance, session filters, virtualized Overview, atomic metadata persistence, fresh-shell workspace resume, helper install/preview/uninstall, and separate Intel/Apple Silicon CI bundles.

Portable validation: 14 passing tests locally (one socket test reserved for unrestricted hosts), Rust check/build, formatting, Clippy, tests including a real shell with PTY resize and terminal-protocol reply, parser state, session transitions/deduplication/reordering, preserving unrelated hooks, split identities, and headless Overview geometry/rendering in both appearances/narrow width. A 100-session synthetic workload exists in test code only. See docs/performance.md for measurements and limits.

Local environment limitation: no macOS window server; local socket bind is denied in the execution sandbox, so Xvfb/interactive desktop and endpoint round-trip could not run here. PTY operations work and were exercised. Socket round-trip is an ignored test run explicitly by unrestricted CI. macOS CI compiles the actual application, exercises PTYs and the socket transport, and produces architecture-specific bundles. Interactive native GUI/Claude/IME/VoiceOver validation remains unperformed. Do not count a Linux/headless result as native acceptance.

Current architecture hosts PTYs in worker threads in the app process. UI navigation preserves them; app exit does not. Metadata/history restore is implemented; detached live-process recovery is planned. Specs/connector/review-evidence milestones remain planned, not partially represented by fake views.

Resume work: inspect CI, install the Intel app on the user's Mac, run the native checklist in docs/terminal-compatibility.md, fix functional blockers before starting the local specification board. Keep TODO.md and this file current per increment.
