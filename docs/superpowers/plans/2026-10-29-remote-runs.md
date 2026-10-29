# Remote runs implementation plan

Goal: Follow GitHub Actions agent runs by a configured ticket key and send explicitly confirmed PR instructions.

Architecture: Keep provider/configuration logic in `src/remote_runs.rs`, transient window UI in `src/ui/remote_runs.rs`, and existing terminal/persistence adapters. Store only raw configuration and optional Work ticket keys in existing JSON preferences. Regex is the only new direct dependency, needed for user patterns and escaped templates.

Constraints: All examples fictional; no credentials, logs, or comment bodies persisted. Every post requires a frozen preview and a distinct confirmation. No issue tracker APIs, merge actions, or polling. Existing configuration errors must not prevent workspace recovery.

- [x] Backend: test configuration validation and visibility, all matching sources, escaped title templates, server event filters, kinds, partial errors, PR title/branch lookup, job metadata, shell/pager arguments, and confirmation-only comment posting with private temporary file cleanup. Implement behind a CLI trait.
- [x] Persistence: prove legacy history compatibility, malformed feature-config isolation and optional ticket-key roundtrip. Add raw config and optional key fields.
- [x] Terminal: prove remote panes never enter output snapshots. Add command launch using the existing login shell; retain exited remote panes for inspection without replaying commands on restoration.
- [x] UI: command action and Work entry, asynchronous parallel repository results, newest-first merged list and links, job/step/full-log/watch actions, editor/preview/explicit-confirm state, loading/errors/manual refresh. Verify narrow/light/dark headless rendering and editor keyboard isolation.
- [x] Review: check quoting, duplicate sources, stale workers, resource limits, errors, cleanup, privacy and existing module conventions. Run formatter, Clippy, all Rust/Python tests and headless checks.
- [x] Documentation: usage and exact JSON example, partial connector roadmap, unreleased notes. Keep documentation in a separate commit after code.

Review focus: malformed config types and regex; keys containing regex/shell syntax; sources overlapping on one repo/run; changed lookups while workers run; remote output retained by the normal snapshot path; confirmation data changing after preview; CLI failures or missing output selection.

Implementation follows the user's supplied task description. Work is split between backend, terminal privacy, documentation, and UI integration, with shared interfaces agreed before editing.

Validation: 259 Rust unit tests, three CLI integration tests, 18 Python tests, formatting, Clippy with warnings denied, shell syntax, ShellCheck and the optimized 0.12.0 build passed. Review fixes isolate and clean GitHub CLI caches, cancel obsolete metadata work, share an eight-request limit and prevent output launch while jobs are loading. Native interaction and live authenticated GitHub acceptance remain manual.
