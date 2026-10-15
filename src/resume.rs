use crate::model::Agent;
use anyhow::{Context, Result, bail};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Prepare an exact-session launch. Call from a worker: CLI checks can take three seconds each.
pub fn launch(agent: Agent, session_id: &str) -> Result<String> {
    validate_id(session_id)?;
    let home = std::env::var_os("HOME").context("Home directory is unavailable")?;
    let root = match agent {
        Agent::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".claude"))
            .join("projects"),
        Agent::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".codex"))
            .join("sessions"),
    };
    if !session_exists(&root, agent, session_id, 0) {
        bail!(
            "{} session {} has no readable local conversation record. Its identity is preserved; restore the CLI's session files and retry.",
            agent.label(),
            session_id
        );
    }
    let name = match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let path = checked_output(Command::new(shell).args(["-lc", &format!("command -v {name}")]))?;
    let path = path.trim();
    if !Path::new(path).is_absolute() || !Path::new(path).is_file() {
        bail!(
            "{} was not found as an executable in your login shell PATH",
            agent.label()
        );
    }
    let mut help = Command::new(path);
    match agent {
        Agent::Claude => {
            help.arg("--help");
        }
        Agent::Codex => {
            help.args(["resume", "--help"]);
        }
    }
    let help = checked_output(&mut help)
        .with_context(|| format!("{} resume support could not be verified", agent.label()))?;
    command_for_help(agent, path, session_id, &help)
}

fn validate_id(id: &str) -> Result<()> {
    let parsed =
        uuid::Uuid::parse_str(id).context("Saved conversation identity is not a session UUID")?;
    if parsed.hyphenated().to_string() != id.to_ascii_lowercase() {
        bail!("Saved conversation identity must be a canonical session UUID");
    }
    Ok(())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn command_for_help(agent: Agent, executable: &str, id: &str, help: &str) -> Result<String> {
    validate_id(id)?;
    let args = match agent {
        Agent::Claude if help.contains("--resume") && help.contains("session ID") => "--resume",
        Agent::Codex if help.contains("resume") && help.contains("[SESSION_ID]") => "resume",
        _ => bail!(
            "Installed {} does not advertise supported exact-session resume",
            agent.label()
        ),
    };
    Ok(format!("{} {} {}", quote(executable), args, quote(id)))
}

fn checked_output(command: &mut Command) -> Result<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not run installed CLI")?;
    let stdout = child.stdout.take().context("CLI output unavailable")?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut data = Vec::new();
        let result = stdout.take(256 * 1024).read_to_end(&mut data).map(|_| data);
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("Installed CLI exited with {status}; repair its installation and retry");
            }
            let data = receiver
                .recv_timeout(Duration::from_millis(100))
                .context("CLI output timed out")??;
            return String::from_utf8(data).context("CLI returned invalid help text");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Installed CLI did not respond within three seconds; retry when it is available");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn session_exists(root: &Path, agent: Agent, id: &str, depth: usize) -> bool {
    if depth > 5 {
        return false;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && session_exists(&path, agent, id, depth + 1) {
            return true;
        }
        if !kind.is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.contains(id) {
            continue;
        }
        let Ok(file) = File::open(path) else {
            continue;
        };
        // Read a bounded prefix without executing or displaying transcript content.
        for line in BufReader::new(file.take(1024 * 1024))
            .lines()
            .take(128)
            .map_while(Result::ok)
        {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let matches = match agent {
                Agent::Claude => record.get("sessionId").and_then(|v| v.as_str()) == Some(id),
                Agent::Codex => {
                    record.get("type").and_then(|v| v.as_str()) == Some("session_meta")
                        && record.pointer("/payload/id").and_then(|v| v.as_str()) == Some(id)
                }
            };
            if matches {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "c7b4b158-2656-4cb2-8036-7d19faf1cc71";
    #[test]
    fn resumes_only_exact_identity_with_verified_flags() {
        assert_eq!(
            command_for_help(
                Agent::Claude,
                "/bin/claude",
                ID,
                "--resume [value] session ID"
            )
            .unwrap(),
            format!("'/bin/claude' --resume '{ID}'")
        );
        assert_eq!(
            command_for_help(Agent::Codex, "/bin/codex", ID, "resume [SESSION_ID]").unwrap(),
            format!("'/bin/codex' resume '{ID}'")
        );
        assert!(command_for_help(Agent::Codex, "/bin/codex", ID, "--last").is_err());
        assert!(
            command_for_help(
                Agent::Claude,
                "/bin/claude",
                "process:123",
                "--resume session ID"
            )
            .is_err()
        );
        assert!(validate_id("'; touch /tmp/injected").is_err());
        assert_eq!(quote("/a'b/c"), "'/a'\\''b/c'");
    }
    #[test]
    fn requires_matching_transcript_identity() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(format!("{ID}.jsonl"));
        fs::write(&file, "{\"sessionId\":\"different\"}\n").unwrap();
        assert!(!session_exists(dir.path(), Agent::Claude, ID, 0));
        fs::write(&file, format!("{{\"sessionId\":\"{ID}\"}}\n")).unwrap();
        assert!(session_exists(dir.path(), Agent::Claude, ID, 0));
        assert!(!session_exists(dir.path(), Agent::Codex, ID, 0));
        fs::write(
            &file,
            format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{ID}\"}}}}\n"),
        )
        .unwrap();
        assert!(session_exists(dir.path(), Agent::Codex, ID, 0));
    }
    #[test]
    fn failed_cli_does_not_create_a_launch_command() {
        assert!(checked_output(Command::new("/bin/sh").args(["-c", "exit 1"])).is_err());
    }
}
