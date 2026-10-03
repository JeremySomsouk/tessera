use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tessera"))
}

#[test]
fn codex_cli_preview_install_uninstall_and_advisory_output() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let original = "notify = [\"existing-notifier\"]\n[features]\nhooks = true\n";
    std::fs::write(&config, original).unwrap();
    let output = cli().arg("codex-hooks").output().unwrap();
    assert!(output.status.success());
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        preview["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with(" codex-hook")
    );
    assert!(preview["hooks"].get("Notification").is_none());
    for verb in [
        "install-codex-hooks",
        "install-codex-hooks",
        "uninstall-codex-hooks",
    ] {
        assert!(
            cli()
                .arg(verb)
                .env("CODEX_HOME", home.path())
                .status()
                .unwrap()
                .success()
        );
    }
    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
    let installed: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("hooks.json")).unwrap()).unwrap();
    assert!(installed["hooks"]["Stop"].as_array().unwrap().is_empty());
    // Outside Tessera, and when its endpoint is unavailable: always advisory JSON.
    for connected in [false, true] {
        let mut command = cli();
        command
            .arg("codex-hook")
            .env_remove("TESSERA_SOCKET")
            .env_remove("TESSERA_PANE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if connected {
            command
                .env("TESSERA_SOCKET", home.path().join("missing.sock"))
                .env("TESSERA_PANE", uuid::Uuid::new_v4().to_string());
        }
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(br#"{"session_id":"thr_123","hook_event_name":"Stop","last_assistant_message":"secret"}"#).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({})
        );
    }
    let path = home.path().join("hooks.json");
    std::fs::write(&path, "invalid").unwrap();
    assert!(
        !cli()
            .arg("install-codex-hooks")
            .env("CODEX_HOME", home.path())
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid");
}

#[test]
#[ignore = "requires local Unix socket permission; unrestricted CI uses --include-ignored"]
fn codex_cli_round_trip_strips_content_and_tags_provider() {
    use std::{io::Read, os::unix::net::UnixListener};
    let d = tempfile::tempdir().unwrap();
    let socket = d.path().join("events.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let pane = uuid::Uuid::new_v4();
    let mut child = cli()
        .arg("codex-hook")
        .env("TESSERA_SOCKET", &socket)
        .env("TESSERA_PANE", pane.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(br#"{"session_id":"thr_123","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"secret"},"transcript_path":"/secret"}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(e) => panic!("Codex hook failed to connect: {e}"),
        }
    };
    // Darwin can reject SO_RCVTIMEO after the short-lived sender has closed.
    // Read already-buffered data with a bounded nonblocking loop instead.
    stream.set_nonblocking(true).unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(e) => panic!("Codex hook payload could not be read: {e}"),
        }
    }
    let event: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(event["agent"], "Codex");
    assert_eq!(event["pane"], pane.to_string());
    assert_eq!(event["hook_event_name"], "PermissionRequest");
    assert_eq!(event["detail"], "Bash");
    assert!(!String::from_utf8(bytes).unwrap().contains("secret"));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({})
    );
}
