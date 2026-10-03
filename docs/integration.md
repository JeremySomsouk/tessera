# Agent integrations

Reference: https://code.claude.com/docs/en/hooks (consulted 2026-10-03).

The adapter registers command hooks for SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, Notification, Stop, and SessionEnd. Schemas use session_id, hook_event_name, notification_type, and tool_name. Extra fields are ignored; missing identity is rejected. The helper rejects events outside its provider allowlist; the model preserves state for unknown events arriving on the socket. Unsupported/newer events are not required. Check `/hooks` with your installed CLI; a real Claude session was not launched in the implementation environment.

The socket lives inside a random mode-0700 directory below `/tmp`, with mode 0600 on the socket. The short path also avoids macOS Unix socket pathname limits. It is a local user boundary, not isolation from processes running as that user. The app only accepts events for its live panes. No TCP endpoint is exposed.

The helper reads at most 64 KiB, strips content down to lifecycle metadata, and connects using TESSERA_SOCKET and TESSERA_PANE inherited from the PTY. Reads/writes are timed and the hook timeout is two seconds. Failure is observational: the command does not block or approve Claude actions, and exits successfully even if tracking is unavailable.

SessionStart/Prompt/tool events indicate running activity. PermissionRequest and permission_prompt notifications indicate attention. idle_prompt indicates input. Stop indicates idle response completion. SessionEnd indicates ended. Tool failures are recorded without declaring task failure. Human review/acceptance are separate local actions.

Each helper event receives a UUID and a local nanosecond sequence timestamp. Duplicate/older events cannot rewind a pane/session. This is local observation ordering, not a guarantee of the source CLI's causal order. Missing events can leave stale states; there is no synthesized completion, remote process inspection, or invented progress.

`hooks` previews additions only. `install-hooks [settings.json]` merges groups, keeps all other settings and hook commands, makes a unique backup, verifies the source did not change before replacement, and atomically replaces the settings file. It is idempotent for the current executable path. `uninstall-hooks` removes only matching helper commands. Use the same binary location to uninstall. If the binary was moved, remove its old exact command via `/hooks` or the settings editor, then reinstall from the final location. There is no implicit editing of dotfiles on app launch.

SSH does not forward the local endpoint. Remote agents are untracked in v0.1. A local pane continuing to run SSH does not imply remote agent tracking.

## Codex CLI

Release behavior reference: https://developers.openai.com/codex/hooks (consulted 2026-10-03). This integration uses native lifecycle command hooks, not the legacy `notify` callback or transcript files.

`codex-hooks` previews SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PermissionRequest, Stop, Interrupt, and SessionEnd. Commands invoke the quoted absolute Tessera binary with `codex-hook`, use a two-second timeout, and are synchronous to reduce out-of-order observations. Stop expects JSON output: the helper returns `{}` with exit zero, including outside Tessera and on transport failure. It never emits decisions, context, continuation requests, or permission overrides.

`install-codex-hooks [hooks.json]` uses the same backup/atomic merge/idempotence safeguards as Claude installation and respects `CODEX_HOME` (default `~/.codex`). It preserves existing file metadata and other hook handlers. It does not change `config.toml`, including existing inline hooks or notify commands. `uninstall-codex-hooks` removes only its exact Codex command; Claude's command and unrelated hooks remain. Both installers are explicit CLI operations, never application startup behavior.

Codex requires review and trust of the exact hook definition through `/hooks`; updated definitions may need trust again. Trust and policy are managed by Codex, and Tessera does not bypass them. Use a release supporting lifecycle hooks. No actual Codex inference is required to install or test Tessera's adapter.

The event's provider is assigned by the helper command, not by untrusted stdin. Session identity includes provider, pane, and source session ID. Old histories without a provider deserialize as Claude and retain their old selection keys. Codex histories persist with Codex identity. The Overview shows both providers, and its existing needs-attention/running/task filters operate on both. Interrupted Codex turns are marked input-needed; response completion does not imply acceptance. Sessions appearing only after a later hook are supported.

Only session ID, lifecycle name, notification type, and tool name are forwarded. Prompt, tool arguments/output, transcript paths, model output, and credentials are discarded. The existing input bound can reject tool events whose full stdin payload exceeds 64 KiB. Tool hooks are not exhaustive; missed hooks or unsupported versions can leave stale states. Subagent hooks use the parent session ID in Codex; this increment aggregates any shared-ID observations into the parent and does not register SubagentStop or infer parent completion from it. Local PTY inheritance is required; remote/Cloud sessions are untracked.

Validation covers documented stdin fixtures, privacy/bounds, provider identity isolation, old-history restoration, transitions, install preservation/idempotence/exact uninstall, mixed-provider headless Overview, and CLI output. Interactive Codex and native Mac validation remain outstanding; no paid session was launched.
