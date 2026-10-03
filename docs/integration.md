# Claude Code integration

Reference: https://code.claude.com/docs/en/hooks (consulted 2026-10-03).

The adapter registers command hooks for SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, Notification, Stop, and SessionEnd. Schemas use session_id, hook_event_name, notification_type, and tool_name. Extra fields are ignored; missing identity is rejected. Unknown events preserve session state. Unsupported/newer events are not required. Check `/hooks` with your installed CLI; a real Claude session was not launched in the implementation environment.

The socket lives inside a random mode-0700 directory below `/tmp`, with mode 0600 on the socket. The short path also avoids macOS Unix socket pathname limits. It is a local user boundary, not isolation from processes running as that user. The app only accepts events for its live panes. No TCP endpoint is exposed.

The helper reads at most 64 KiB, strips content down to lifecycle metadata, and connects using TESSERA_SOCKET and TESSERA_PANE inherited from the PTY. Reads/writes are timed and the hook timeout is two seconds. Failure is observational: the command does not block or approve Claude actions, and exits successfully even if tracking is unavailable.

SessionStart/Prompt/tool events indicate running activity. PermissionRequest and permission_prompt notifications indicate attention. idle_prompt indicates input. Stop indicates idle response completion. SessionEnd indicates ended. Tool failures are recorded without declaring task failure. Human review/acceptance are separate local actions.

Each helper event receives a UUID and a local nanosecond sequence timestamp. Duplicate/older events cannot rewind a pane/session. This is local observation ordering, not a guarantee of the source CLI's causal order. Missing events can leave stale states; there is no synthesized completion, remote process inspection, or invented progress.

`hooks` previews additions only. `install-hooks [settings.json]` merges groups, keeps all other settings and hook commands, makes a unique backup, verifies the source did not change before replacement, and atomically replaces the settings file. It is idempotent for the current executable path. `uninstall-hooks` removes only matching helper commands. Use the same binary location to uninstall. If the binary was moved, remove its old exact command via `/hooks` or the settings editor, then reinstall from the final location. There is no implicit editing of dotfiles on app launch.

SSH does not forward the local endpoint. Remote agents are untracked in v0.1. A local pane continuing to run SSH does not imply remote agent tracking.
