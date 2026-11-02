//! Opt-in, bounded end-to-end PTY measurements; see docs/terminal-performance.md.
use crate::terminal::Terminal;
use eframe::egui::Context;
use std::time::{Duration, Instant};
use uuid::Uuid;

fn marker(terminal: &Terminal, text: &str) -> bool {
    terminal.term.try_lock().is_ok_and(|term| {
        term.grid()
            .display_iter()
            .map(|cell| cell.cell.c)
            .collect::<String>()
            .contains(text)
    })
}

#[test]
#[ignore = "set TESSERA_BENCH_MIB=1,4,16 to run bounded PTY workloads"]
fn terminal_workloads() {
    let Ok(mib) = std::env::var("TESSERA_BENCH_MIB") else {
        return;
    };
    let mib: usize = mib.parse().unwrap();
    assert!(
        (1..=16).contains(&mib),
        "benchmark budget: 1..=16 MiB/session"
    );
    let bytes = mib * 1024 * 1024;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests/fixtures/terminal/workload.py");
    let overall = Instant::now();
    let (initial_cpu, _) = resources();
    for kind in ["build", "agent", "line"] {
        for sessions in [1, 4] {
            let startup = Instant::now();
            let temporary = tempfile::tempdir().unwrap();
            let terminals: Vec<_> = (0..sessions)
                .map(|index| {
                    Terminal::spawn_agent(
                        Uuid::new_v4(),
                        root,
                        std::path::Path::new("/tmp/tessera-bench-unused.sock"),
                        Context::default(),
                        &format!(
                            "python3 '{}' {kind} {bytes} '{}'",
                            fixture.display(),
                            temporary.path().join(index.to_string()).display()
                        ),
                    )
                    .unwrap()
                })
                .collect();
            let deadline = Instant::now() + Duration::from_secs(15);
            while !terminals.iter().all(|term| marker(term, "BENCH_READY")) {
                assert!(Instant::now() < deadline, "startup timeout");
                std::thread::sleep(Duration::from_millis(1));
            }
            let startup_ms = startup.elapsed().as_secs_f64() * 1000.0;
            let initial_output: Vec<_> = terminals.iter().map(Terminal::output_bytes).collect();
            let started = Instant::now();
            for term in &terminals {
                term.input(b"s".to_vec()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(10));
            let input_started = Instant::now();
            for term in &terminals {
                term.input(b"p".to_vec()).unwrap();
            }
            let mut input_ms = None;
            let mut probes = 0usize;
            let mut busy = 0usize;
            let mut max_probe_us = 0u128;
            loop {
                let probe = Instant::now();
                let mut complete = true;
                for term in &terminals {
                    probes += 1;
                    if term.term.try_lock().is_err() {
                        busy += 1;
                    }
                    complete &= marker(term, "BENCH_DONE");
                }
                max_probe_us = max_probe_us.max(probe.elapsed().as_micros());
                if input_ms.is_none()
                    && (0..sessions).all(|index| temporary.path().join(index.to_string()).exists())
                {
                    input_ms = Some(input_started.elapsed().as_secs_f64() * 1000.0);
                }
                if complete && input_ms.is_some() {
                    break;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "output timeout"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            let elapsed = started.elapsed();
            for (term, initial) in terminals.iter().zip(initial_output) {
                assert_eq!(
                    term.output_bytes() - initial,
                    (bytes + b"\r\nBENCH_DONE\r\n".len()) as u64
                );
                assert_eq!(term.input_bytes(), 2);
            }
            let snapshot = Instant::now();
            let snapshot_bytes: usize = terminals
                .iter()
                .map(|term| term.snapshot_output().len())
                .sum();
            let snapshot_ms = snapshot.elapsed().as_secs_f64() * 1000.0;
            let teardown = Instant::now();
            drop(terminals);
            println!(
                "BENCH workload={kind} sessions={sessions} bytes_per_session={bytes} startup_ms={startup_ms:.2} input_ack_ms={:.2} output_ms={:.2} mib_s={:.2} busy_pct={:.1} max_probe_us={max_probe_us} snapshot_ms={snapshot_ms:.2} snapshot_bytes={snapshot_bytes} teardown_ms={:.2}",
                input_ms.unwrap(),
                elapsed.as_secs_f64() * 1000.0,
                mib as f64 * sessions as f64 / elapsed.as_secs_f64(),
                busy as f64 * 100.0 / probes as f64,
                teardown.elapsed().as_secs_f64() * 1000.0
            );
            let (cpu, rss) = resources();
            println!(
                "RESOURCES cumulative_cpu_s={:.3} peak_rss_bytes={rss}",
                cpu - initial_cpu
            );
            assert!(
                overall.elapsed() < Duration::from_secs(180),
                "overall benchmark time budget"
            );
        }
    }
}

#[cfg(unix)]
fn resources() -> (f64, i64) {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the supplied rusage on success.
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let usage = unsafe { usage.assume_init() };
    let cpu = usage.ru_utime.tv_sec as f64
        + usage.ru_utime.tv_usec as f64 / 1e6
        + usage.ru_stime.tv_sec as f64
        + usage.ru_stime.tv_usec as f64 / 1e6;
    let rss = if cfg!(target_os = "macos") {
        usage.ru_maxrss
    } else {
        usage.ru_maxrss * 1024
    };
    (cpu, rss)
}

#[test]
#[ignore = "set TESSERA_BENCH_SOAK=1 for a bounded 256 MiB continuous session"]
fn terminal_soak() {
    if std::env::var_os("TESSERA_BENCH_SOAK").is_none() {
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let temporary = tempfile::tempdir().unwrap();
    let ack = temporary.path().join("ack");
    let bytes = 256 * 1024 * 1024;
    let terminal = Terminal::spawn_agent(
        Uuid::new_v4(),
        root,
        std::path::Path::new("/tmp/tessera-bench-unused.sock"),
        Context::default(),
        &format!(
            "python3 '{}' soak {bytes} '{}'",
            root.join("tests/fixtures/terminal/workload.py").display(),
            ack.display()
        ),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker(&terminal, "BENCH_READY") {
        assert!(Instant::now() < deadline, "startup timeout");
        std::thread::sleep(Duration::from_millis(1));
    }
    let initial_output = terminal.output_bytes();
    let started = Instant::now();
    let (initial_cpu, _) = resources();
    terminal.input(b"s".to_vec()).unwrap();
    std::thread::sleep(Duration::from_millis(10));
    let input_started = Instant::now();
    terminal.input(b"p".to_vec()).unwrap();
    while !ack.exists() {
        assert!(
            input_started.elapsed() < Duration::from_secs(5),
            "input acknowledgement timeout"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let input_ms = input_started.elapsed().as_secs_f64() * 1000.0;
    let mut next_sample = 1;
    loop {
        if started.elapsed().as_secs() >= next_sample {
            let (cpu, rss) = resources();
            println!(
                "SOAK seconds={} cpu_s={:.3} peak_rss_bytes={rss}",
                started.elapsed().as_secs(),
                cpu - initial_cpu
            );
            assert!(
                rss < 512 * 1024 * 1024,
                "512 MiB process memory budget exceeded"
            );
            next_sample += 1;
        }
        if marker(&terminal, "BENCH_DONE") {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "soak time budget exceeded"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        terminal.output_bytes() - initial_output,
        (bytes + b"\r\nBENCH_DONE\r\n".len()) as u64
    );
    assert_eq!(terminal.input_bytes(), 2);
    println!(
        "SOAK_DONE bytes={bytes} elapsed_ms={:.2} input_ack_ms={input_ms:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
