use crate::model::{Agent, HookEvent};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, sync_channel},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
const LIMIT: u64 = 64 * 1024;
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub struct Endpoint {
    pub path: PathBuf,
    pub receiver: Receiver<HookEvent>,
}
impl Endpoint {
    pub fn start(ctx: eframe::egui::Context) -> Result<Self> {
        let dir = PathBuf::from("/tmp").join(format!("tessera-{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join("events.sock");
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let (tx, receiver) = sync_channel(256);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
                let mut bytes = Vec::new();
                if Read::by_ref(&mut stream)
                    .take(LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .is_err()
                    || bytes.len() as u64 > LIMIT
                {
                    continue;
                }
                if let Ok(e) = serde_json::from_slice::<HookEvent>(&bytes) {
                    if e.session_id.len() > 256
                        || e.detail.len() > 4096
                        || e.hook_event_name.len() > 64
                    {
                        continue;
                    }
                    if tx.send(e).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
        });
        Ok(Self { path, receiver })
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(d) = self.path.parent() {
            let _ = std::fs::remove_dir(d);
        }
    }
}
pub fn hook(agent: Agent) -> Result<()> {
    let Ok(socket) = std::env::var("TESSERA_SOCKET") else {
        return Ok(());
    };
    let pane = std::env::var("TESSERA_PANE")?.parse()?;
    let mut bytes = Vec::new();
    std::io::stdin().take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        bail!("hook input exceeds 64 KiB");
    }
    let e = parse_hook(&bytes, pane, agent)?;
    let mut stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(std::time::Duration::from_millis(500)))?;
    stream.write_all(&serde_json::to_vec(&e)?)?;
    Ok(())
}
// Providers share the documented stdin schema. Only allowlisted metadata is forwarded.
fn parse_hook(bytes: &[u8], pane: Uuid, agent: Agent) -> Result<HookEvent> {
    if bytes.len() as u64 > LIMIT {
        bail!("hook input exceeds 64 KiB");
    }
    let v: Value = serde_json::from_slice(bytes)?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let session_id = field("session_id");
    if session_id.is_empty() || session_id.len() > 256 {
        bail!("invalid session_id");
    }
    let hook_event_name = field("hook_event_name");
    if !events(agent).contains(&hook_event_name.as_str()) {
        bail!("unsupported hook event");
    }
    let detail = [field("notification_type"), field("tool_name")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if detail.len() > 4096 {
        bail!("hook metadata exceeds limit");
    }
    Ok(HookEvent {
        id: Uuid::new_v4(),
        pane,
        agent,
        sequence: SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        session_id,
        hook_event_name,
        detail,
    })
}
fn events(agent: Agent) -> &'static [&'static str] {
    match agent {
        Agent::Claude => &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "Notification",
            "Stop",
            "SessionEnd",
        ],
        Agent::Codex => &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "Stop",
            "Interrupt",
            "SessionEnd",
        ],
    }
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
pub fn settings(binary: &Path) -> Value {
    settings_for(binary, Agent::Claude)
}
pub fn settings_for(binary: &Path, agent: Agent) -> Value {
    let verb = match agent {
        Agent::Claude => "hook",
        Agent::Codex => "codex-hook",
    };
    let command = format!("{} {verb}", quote(&binary.to_string_lossy()));
    let mut hooks = serde_json::Map::new();
    for &event in events(agent) {
        hooks.insert(
            event.into(),
            json!([{ "matcher":"", "hooks":[{"type":"command","command":command,"timeout":2}] }]),
        );
    }
    json!({"hooks":hooks})
}
pub fn install(path: &Path, binary: &Path, remove: bool) -> Result<()> {
    install_for(path, binary, remove, Agent::Claude)
}
pub fn install_for(path: &Path, binary: &Path, remove: bool, agent: Agent) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("settings path is a symlink; pass its target explicitly");
    }
    let existed = path.exists();
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e.into()),
    };
    let mut v: Value =
        serde_json::from_str(&original).context("invalid settings JSON; untouched")?;
    let hooks = v
        .as_object_mut()
        .context("settings must be an object")?
        .entry("hooks")
        .or_insert(json!({}))
        .as_object_mut()
        .context("hooks must be an object")?;
    let proposed = settings_for(binary, agent);
    for (event, entries) in proposed["hooks"].as_object().unwrap() {
        let target = hooks
            .entry(event)
            .or_insert(json!([]))
            .as_array_mut()
            .context("hook event must be an array")?;
        let cmd = entries[0]["hooks"][0]["command"].as_str().unwrap();
        for group in target.iter_mut() {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                list.retain(|h| h["command"].as_str() != Some(cmd));
            }
        }
        target.retain(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|a| !a.is_empty())
        });
        if !remove {
            target.push(entries[0].clone());
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let backup = path.with_extension(format!("json.{}.bak", Uuid::new_v4()));
    if path.exists() {
        std::fs::copy(path, backup)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&v)?)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    let current = std::fs::read_to_string(path);
    if (existed && current.as_ref().ok() != Some(&original)) || (!existed && path.exists()) {
        let _ = std::fs::remove_file(tmp);
        bail!("settings changed during installation; retry after reviewing them");
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_payload_is_metadata_only_and_validated() {
        let p = Uuid::new_v4();
        let input = json!({"session_id":"thr_123", "hook_event_name":"PostToolUse", "tool_name":"apply_patch", "turn_id":"turn_1", "prompt":"secret", "tool_input":{"command":"secret"}, "tool_response":"secret", "transcript_path":"/secret"});
        let e = parse_hook(&serde_json::to_vec(&input).unwrap(), p, Agent::Codex).unwrap();
        assert_eq!(e.pane, p);
        assert_eq!(e.agent, Agent::Codex);
        assert_eq!(e.detail, "apply_patch");
        assert!(!serde_json::to_string(&e).unwrap().contains("secret"));
        for input in [
            json!({"hook_event_name":"Stop"}),
            json!({"session_id":"x","hook_event_name":"Notification"}),
            json!({"session_id":"x","hook_event_name":"PostToolUseFailure"}),
            json!({"session_id":"x","hook_event_name":"Stop","tool_name":"x".repeat(4097)}),
        ] {
            assert!(parse_hook(&serde_json::to_vec(&input).unwrap(), p, Agent::Codex).is_err());
        }
        assert!(parse_hook(&vec![b' '; LIMIT as usize + 1], p, Agent::Codex).is_err());
        assert!(parse_hook(b"broken", p, Agent::Codex).is_err());
    }
    #[test]
    fn codex_install_preserves_metadata_and_other_provider() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hooks.json");
        std::fs::write(&p, r#"{"description":"my hooks","hooks":{"Stop":[{"hooks":[{"type":"command","command":"other"}]}]}}"#).unwrap();
        let binary = Path::new("/tmp/Tessera's app/tessera");
        install(&p, binary, false).unwrap();
        install_for(&p, binary, false, Agent::Codex).unwrap();
        install_for(&p, binary, false, Agent::Codex).unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["description"], "my hooks");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 3);
        assert_eq!(v["hooks"]["Interrupt"][0]["hooks"][0]["timeout"], 2);
        assert!(
            v["hooks"]["Interrupt"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with(" codex-hook")
        );
        install_for(&p, binary, true, Agent::Codex).unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "other");
        assert!(v["hooks"]["Interrupt"].as_array().unwrap().is_empty());
        let before = std::fs::read(&p).unwrap();
        let link = d.path().join("link.json");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(install_for(&link, binary, false, Agent::Codex).is_err());
        assert_eq!(std::fs::read(p).unwrap(), before);
    }
    #[test]
    fn install_preserves_other_hooks_and_uninstall_is_exact() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        std::fs::write(&p,r#"{"env":{"X":"Y"},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other"}]}]}}"#).unwrap();
        install(&p, Path::new("/tmp/test tessera"), false).unwrap();
        install(&p, Path::new("/tmp/test tessera"), false).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["env"]["X"], "Y");
        install(&p, Path::new("/tmp/test tessera"), true).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    #[test]
    #[ignore = "requires local Unix socket permission; unrestricted CI uses --include-ignored"]
    fn socket_ingests_valid_events_and_rejects_oversized_payloads() {
        let endpoint = Endpoint::start(eframe::egui::Context::default()).unwrap();
        let event = HookEvent {
            id: Uuid::new_v4(),
            pane: Uuid::new_v4(),
            sequence: 1,
            session_id: "transport-test".into(),
            agent: Agent::Claude,
            hook_event_name: "PermissionRequest".into(),
            detail: "Bash".into(),
        };
        let mut stream = UnixStream::connect(&endpoint.path).unwrap();
        stream
            .write_all(&serde_json::to_vec(&event).unwrap())
            .unwrap();
        drop(stream);
        let received = endpoint
            .receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(received.id, event.id);
        let mut stream = UnixStream::connect(&endpoint.path).unwrap();
        let _ = stream.write_all(&vec![b' '; LIMIT as usize + 1]);
        drop(stream);
        assert!(
            endpoint
                .receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
    }
    #[test]
    fn malformed_settings_are_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        std::fs::write(&p, "not json").unwrap();
        assert!(install(&p, Path::new("/tmp/tessera"), false).is_err());
        assert_eq!(std::fs::read_to_string(p).unwrap(), "not json");
    }
}
