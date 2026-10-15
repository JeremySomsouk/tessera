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
    verify_resume(
        &mut Command::new(crate::terminal::agent_shell()),
        agent,
        session_id,
    )
}

fn verify_resume(shell: &mut Command, agent: Agent, session_id: &str) -> Result<String> {
    validate_id(session_id)?;
    let (name, help_args) = match agent {
        Agent::Claude => ("claude", "--help"),
        Agent::Codex => ("codex", "resume --help"),
    };
    // Frame the response so shell startup output cannot be mistaken for a CLI path or help.
    let marker = format!("TESSERA_RESUME_{}", uuid::Uuid::new_v4().simple());
    let script = format!(
        "tessera_executable=$(command -v {name}) || tessera_executable=''; \
         if [ ! -f \"$tessera_executable\" ] || [ ! -x \"$tessera_executable\" ]; then tessera_executable=''; fi; \
         printf '\\n{marker}\\n%s\\n' \"$tessera_executable\"; \
         if [ -n \"$tessera_executable\" ]; then exec \"$tessera_executable\" {help_args}; fi"
    );
    // Match the interactive login shell used by Terminal, including PATH and CLI dependencies.
    let output = checked_output(shell.args(["-lic", &script])).with_context(|| {
        format!(
            "{} resume support could not be verified in your interactive login shell",
            agent.label()
        )
    })?;
    let framed = output
        .split_once(&format!("\n{marker}\n"))
        .context("Login shell did not return the CLI verification result")?
        .1;
    let (path, help) = framed
        .split_once('\n')
        .context("CLI verification result is incomplete")?;
    if !Path::new(path).is_absolute() || !Path::new(path).is_file() {
        bail!(
            "{} was not found as an executable in your interactive login shell PATH; check its installation and retry",
            agent.label()
        );
    }
    command_for_help(agent, path, session_id, help)
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

fn capture_output(reader: impl Read + Send + 'static) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut data = Vec::new();
        let result = (|| {
            let mut buffer = [0; 8192];
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                let keep = count.min((256 * 1024usize).saturating_sub(data.len()));
                data.extend_from_slice(&buffer[..keep]);
            }
            Ok(data)
        })();
        let _ = sender.send(result);
    });
    receiver
}

fn checked_output(command: &mut Command) -> Result<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start CLI verification")?;
    let stdout = capture_output(child.stdout.take().context("CLI output unavailable")?);
    let stderr = capture_output(child.stderr.take().context("CLI diagnostics unavailable")?);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let data = stderr
                    .recv_timeout(Duration::from_millis(100))
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                let diagnostic: String = String::from_utf8_lossy(&data)
                    .chars()
                    .rev()
                    .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                    .take(4096)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if diagnostic.trim().is_empty() {
                    bail!("CLI verification exited with {status}");
                }
                bail!(
                    "CLI verification exited with {status}: {}",
                    diagnostic.trim()
                );
            }
            let data = stdout
                .recv_timeout(Duration::from_millis(100))
                .context("CLI output timed out")??;
            return String::from_utf8(data).context("CLI returned invalid help text");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "CLI verification did not respond within three seconds; retry when it is available"
            );
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
    #[cfg(target_os = "macos")]
    fn shell_fixture(cli: Option<&str>) -> (tempfile::TempDir, Command) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(dir.path().join(".zshrc"), format!(
            "export PATH={}\nexport TESSERA_FIXTURE=ready\nprintf 'startup chatter --resume session ID\\n'\nprintf 'startup diagnostic\\n' >&2\n",
            quote(bin.to_str().unwrap())
        )).unwrap();
        if let Some(cli) = cli {
            let path = bin.join("claude");
            fs::write(&path, format!("#!/bin/sh\n{cli}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut shell = Command::new("/bin/zsh");
        shell
            .env_clear()
            .env("HOME", dir.path())
            .env("ZDOTDIR", dir.path())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        (dir, shell)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn interactive_shell_resolves_cli_and_its_environment_from_desktop_path() {
        let (dir, mut shell) = shell_fixture(Some(
            "[ \"$TESSERA_FIXTURE\" = ready ] || exit 1\n[ \"$1\" = --help ] || exit 2\nfixture-helper",
        ));
        use std::os::unix::fs::PermissionsExt;
        let helper = dir.path().join("bin/fixture-helper");
        fs::write(&helper, "#!/bin/sh\nprintf '%s\\n' '--resume session ID'\n").unwrap();
        fs::set_permissions(helper, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            verify_resume(&mut shell, Agent::Claude, ID).unwrap(),
            format!(
                "{} --resume '{ID}'",
                quote(dir.path().join("bin/claude").to_str().unwrap())
            )
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_chatter_cannot_advertise_resume_for_an_unsupported_cli() {
        let (_dir, mut shell) = shell_fixture(Some("printf 'unsupported CLI\\n'"));
        assert!(
            verify_resume(&mut shell, Agent::Claude, ID)
                .unwrap_err()
                .to_string()
                .contains("does not advertise supported exact-session resume")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_cli_and_failed_cli_have_distinct_diagnostics() {
        let (_dir, mut shell) = shell_fixture(None);
        let missing = verify_resume(&mut shell, Agent::Claude, ID)
            .unwrap_err()
            .to_string();
        assert!(
            missing.contains("was not found as an executable"),
            "{missing}"
        );
        assert!(!missing.contains("startup diagnostic"));
        let (_dir, mut shell) =
            shell_fixture(Some("printf 'missing native binary\\n' >&2; exit 1"));
        let failed = format!(
            "{:#}",
            verify_resume(&mut shell, Agent::Claude, ID).unwrap_err()
        );
        assert!(
            failed.contains("resume support could not be verified"),
            "{failed}"
        );
        assert!(failed.contains("missing native binary"), "{failed}");
        assert!(!failed.contains("was not found"));
    }

    #[test]
    fn captured_output_is_bounded_and_drained() {
        let reader = std::io::Cursor::new(vec![b'x'; 1024 * 1024]);
        assert_eq!(
            capture_output(reader).recv().unwrap().unwrap().len(),
            256 * 1024
        );
        let error = checked_output(
            Command::new("/bin/sh").args(["-c", "printf 'native binary missing' >&2; exit 1"]),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("native binary missing"));
    }

    #[test]
    fn failed_cli_does_not_create_a_launch_command() {
        assert!(checked_output(Command::new("/bin/sh").args(["-c", "exit 1"])).is_err());
    }
}
