use super::*;

const VIEWPORT: Vec2 = Vec2::new(1280.0, 800.0);
const BYTES_PER_PANE: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct PaneMeasurements {
    fresh: usize,
    cached: usize,
    last_fresh: Instant,
    longest_stale: Duration,
}

impl Default for PaneMeasurements {
    fn default() -> Self {
        Self {
            fresh: 0,
            cached: 0,
            last_fresh: Instant::now(),
            longest_stale: Duration::ZERO,
        }
    }
}

struct Frames {
    ctx: egui::Context,
    searches: Vec<TerminalSearch>,
    panes: Vec<PaneMeasurements>,
    active: Vec<bool>,
    probe: Rect,
    clicks: usize,
    focused: usize,
}

impl Frames {
    fn new(ctx: egui::Context, count: usize) -> Self {
        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new("terminal-render-counts"),
                TerminalRenderCounts::default(),
            );
        });
        Self {
            ctx,
            searches: (0..count).map(|_| TerminalSearch::default()).collect(),
            panes: vec![PaneMeasurements::default(); count],
            active: vec![true; count],
            probe: Rect::NOTHING,
            clicks: 0,
            focused: count - 1,
        }
    }

    fn draw(&mut self, terminals: &mut [Terminal], events: Vec<egui::Event>) -> Duration {
        let started = Instant::now();
        let ctx = self.ctx.clone();
        let columns = match terminals.len() {
            1 => 1,
            4 => 2,
            _ => 4,
        };
        let rows = terminals.len().div_ceil(columns);
        let focused = self.focused;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, VIEWPORT)),
                events,
                ..Default::default()
            },
            |ctx| {
                prepare_terminal_rendering(ctx, terminals.len());
                egui::CentralPanel::default().show(ctx, |ui| {
                    let probe = ui.button("UI response probe");
                    self.probe = probe.rect;
                    self.clicks += usize::from(probe.clicked());
                    let available = ui.available_rect_before_wrap();
                    let size = available.size() / Vec2::new(columns as f32, rows as f32);
                    for (index, terminal) in terminals.iter_mut().enumerate() {
                        let min = available.min
                            + Vec2::new(
                                (index % columns) as f32 * size.x,
                                (index / columns) as f32 * size.y,
                            );
                        let rect = Rect::from_min_size(min, size).shrink(2.0);
                        let before = TerminalRenderCounts::read(ctx);
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .id_salt(("benchmark-pane", index))
                                .max_rect(rect),
                            |ui| {
                                terminal_view(
                                    ui,
                                    terminal,
                                    &mut self.searches[index],
                                    index == focused,
                                    15.0,
                                    false,
                                )
                                .1
                                .unwrap();
                            },
                        );
                        let after = TerminalRenderCounts::read(ctx);
                        if self.active[index] {
                            let pane = &mut self.panes[index];
                            pane.longest_stale = pane.longest_stale.max(pane.last_fresh.elapsed());
                            pane.fresh += after.fresh - before.fresh;
                            pane.cached += after.busy - before.busy;
                            if after.fresh > before.fresh {
                                pane.last_fresh = Instant::now();
                            }
                        }
                    }
                });
            },
        );
        std::hint::black_box(output);
        started.elapsed()
    }

    fn click_events(&self) -> Vec<egui::Event> {
        let position = self.probe.center();
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]
    }

    fn reset(&mut self) {
        self.panes.fill_with(PaneMeasurements::default);
    }
}

fn quantile(values: &[f64], percent: usize) -> f64 {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values[(values.len() * percent / 100).min(values.len() - 1)]
}

fn marker(terminal: &Terminal, text: &str) -> bool {
    terminal.term.try_lock().is_ok_and(|term| {
        term.grid()
            .display_iter()
            .map(|cell| cell.cell.c)
            .collect::<String>()
            .contains(text)
    })
}

fn peak_rss() -> i64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage fills the supplied structure on success.
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let usage = unsafe { usage.assume_init() };
    if cfg!(target_os = "macos") {
        usage.ru_maxrss
    } else {
        usage.ru_maxrss * 1024
    }
}

#[test]
#[ignore = "set TESSERA_BENCH_MULTIPANE=1 for bounded multi-pane rendering measurements"]
fn benchmark_multiple_busy_panes() {
    if std::env::var_os("TESSERA_BENCH_MULTIPANE").is_none() {
        return;
    }
    let overall = Instant::now();
    for (count, hold_first, only_busy) in [
        (1, false, None),
        (4, false, None),
        (8, false, None),
        (16, false, None),
        (16, true, None),
        (16, false, Some(0)),
        (16, false, Some(15)),
    ] {
        let pane_bytes: Vec<_> = (0..count)
            .map(|index| {
                if only_busy.is_none_or(|busy| busy == index) {
                    BYTES_PER_PANE
                } else {
                    0
                }
            })
            .collect();
        let focused = only_busy.unwrap_or(count - 1);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let temporary = tempfile::tempdir().unwrap();
        let fixture = root.join("tests/fixtures/terminal/multipane.py");
        let ctx = egui::Context::default();
        let mut terminals: Vec<_> = (0..count)
            .map(|index| {
                Terminal::spawn_agent(
                    Uuid::new_v4(),
                    root,
                    std::path::Path::new(""),
                    ctx.clone(),
                    &format!(
                        "python3 '{}' {} '{}'",
                        fixture.display(),
                        pane_bytes[index],
                        temporary.path().join(index.to_string()).display(),
                    ),
                )
                .unwrap()
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !terminals
            .iter()
            .all(|terminal| marker(terminal, "MULTI_READY"))
        {
            assert!(
                Instant::now() < deadline,
                "multi-pane startup budget exceeded"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut frames = Frames::new(ctx, count);
        frames.focused = focused;
        for _ in 0..3 {
            frames.draw(&mut terminals, Vec::new());
            std::thread::sleep(Duration::from_millis(20));
        }
        frames.reset();
        let initial_bytes: Vec<_> = terminals.iter().map(Terminal::output_bytes).collect();
        let grid = terminals[0].term.clone();
        let mut held = hold_first.then(|| grid.lock().unwrap());
        let started = Instant::now();
        for terminal in &terminals {
            terminal.input(b"s".to_vec()).unwrap();
        }
        let mut times = Vec::new();
        let mut acknowledgement = Vec::new();
        let mut awaiting = None;
        let mut sent = 0;
        let mut next_input = Duration::from_millis(100);
        let mut next_click = Duration::from_millis(200);
        let mut clicks = Vec::new();
        let ack = temporary.path().join(focused.to_string());
        loop {
            let elapsed = started.elapsed();
            if elapsed >= Duration::from_millis(500) {
                held.take();
            }
            let mut events = Vec::new();
            let mut clicked = false;
            if elapsed >= next_click {
                events = frames.click_events();
                next_click += Duration::from_millis(500);
                clicked = true;
            } else if elapsed >= next_input && awaiting.is_none() {
                events.push(egui::Event::Text("p".into()));
                awaiting = Some(Instant::now());
                sent += 1;
                next_input += Duration::from_millis(500);
            }
            for (index, terminal) in terminals.iter().enumerate() {
                frames.active[index] = terminal.output_bytes() - initial_bytes[index]
                    < (pane_bytes[index] + b"\r\nMULTI_DONE\r\n".len()) as u64;
            }
            let previous_clicks = frames.clicks;
            let frame = frames.draw(&mut terminals, events);
            let frame_ms = frame.as_secs_f64() * 1000.0;
            times.push(frame_ms);
            if clicked {
                assert_eq!(
                    frames.clicks,
                    previous_clicks + 1,
                    "nonterminal UI event was missed"
                );
                clicks.push(frame_ms);
            }
            if let Some(input_started) = awaiting {
                let received = std::fs::read_to_string(&ack)
                    .ok()
                    .and_then(|text| text.parse::<usize>().ok());
                if received == Some(sent) {
                    acknowledgement.push(input_started.elapsed().as_secs_f64() * 1000.0);
                    awaiting = None;
                }
            }
            let complete = terminals.iter().zip(&initial_bytes).zip(&pane_bytes).all(
                |((terminal, initial), bytes)| {
                    terminal.output_bytes() - initial
                        == (bytes + b"\r\nMULTI_DONE\r\n".len()) as u64
                },
            );
            if complete && awaiting.is_none() && sent > 0 && !clicks.is_empty() {
                break;
            }
            assert!(
                elapsed < Duration::from_secs(60),
                "multi-pane output budget exceeded"
            );
            assert!(
                overall.elapsed() < Duration::from_secs(240),
                "overall benchmark budget exceeded"
            );
            assert!(
                peak_rss() < 1024 * 1024 * 1024,
                "1 GiB process memory budget exceeded"
            );
            std::thread::sleep(Duration::from_millis(16).saturating_sub(frame));
        }
        let elapsed = started.elapsed().as_secs_f64();
        let recovery_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let before = TerminalRenderCounts::read(&frames.ctx).fresh;
            frames.draw(&mut terminals, Vec::new());
            if TerminalRenderCounts::read(&frames.ctx).fresh - before == count {
                break;
            }
            assert!(
                Instant::now() < recovery_deadline,
                "final display did not recover"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            terminals
                .iter()
                .all(|terminal| marker(terminal, "MULTI_DONE"))
        );
        let scenario = match (hold_first, only_busy) {
            (true, _) => "held-first",
            (_, Some(0)) => "mixed-first",
            (_, Some(_)) => "mixed-last",
            _ => "busy",
        };
        println!(
            "MULTIPANE scenario={scenario} panes={count} bytes_per_pane={BYTES_PER_PANE} frames={} frame_median_ms={:.3} frame_p95_ms={:.3} frame_max_ms={:.3} over16ms_pct={:.1} ui_clicks={} ui_click_frame_p95_ms={:.3} input_probes={} input_ack_p95_ms={:.3} input_ack_max_ms={:.3} output_ms={:.3} mib_s={:.2} peak_rss_bytes={}",
            times.len(),
            quantile(&times, 50),
            quantile(&times, 95),
            quantile(&times, 100),
            100.0 * times.iter().filter(|ms| **ms > 16.0).count() as f64 / times.len() as f64,
            clicks.len(),
            quantile(&clicks, 95),
            acknowledgement.len(),
            quantile(&acknowledgement, 95),
            quantile(&acknowledgement, 100),
            elapsed * 1000.0,
            pane_bytes.iter().sum::<usize>() as f64 / (1024.0 * 1024.0) / elapsed,
            peak_rss(),
        );
        for (index, pane) in frames.panes.iter().enumerate() {
            if pane_bytes[index] == 0 {
                continue;
            }
            println!(
                "PANE scenario={scenario} panes={count} index={index} fresh={} cached={} fresh_pct={:.1} max_stale_ms={:.3} fresh_hz={:.2}",
                pane.fresh,
                pane.cached,
                100.0 * pane.fresh as f64 / (pane.fresh + pane.cached) as f64,
                pane.longest_stale.as_secs_f64() * 1000.0,
                pane.fresh as f64 / elapsed
            );
        }
        assert_eq!(terminals[focused].input_bytes(), 1 + sent as u64);
    }
}

#[test]
#[ignore = "set TESSERA_BENCH_MULTIPANE=1 for bounded all-busy-lock frame measurements"]
fn benchmark_all_grids_held() {
    if std::env::var_os("TESSERA_BENCH_MULTIPANE").is_none() {
        return;
    }
    let ctx = egui::Context::default();
    let temporary = tempfile::tempdir().unwrap();
    let mut terminals: Vec<_> = (0..16)
        .map(|_| {
            Terminal::spawn_test(
                Uuid::new_v4(),
                temporary.path(),
                std::path::Path::new(""),
                ctx.clone(),
            )
            .unwrap()
        })
        .collect();
    let mut frames = Frames::new(ctx, 16);
    for _ in 0..3 {
        frames.draw(&mut terminals, Vec::new());
        std::thread::sleep(Duration::from_millis(20));
    }
    let grids: Vec<_> = terminals
        .iter()
        .map(|terminal| terminal.term.clone())
        .collect();
    let held: Vec<_> = grids.iter().map(|grid| grid.lock().unwrap()).collect();
    frames.reset();
    let started = Instant::now();
    let mut times = Vec::new();
    for _ in 0..100 {
        let events = frames.click_events();
        let before = frames.clicks;
        times.push(frames.draw(&mut terminals, events).as_secs_f64() * 1000.0);
        assert_eq!(frames.clicks, before + 1);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "held-grid budget exceeded"
        );
    }
    drop(held);
    let before_recovery = TerminalRenderCounts::read(&frames.ctx).fresh;
    frames.draw(&mut terminals, Vec::new());
    assert_eq!(
        TerminalRenderCounts::read(&frames.ctx).fresh - before_recovery,
        16,
        "every pane must recover immediately after contention ends"
    );
    println!(
        "HELD panes=16 frames=100 frame_median_ms={:.3} frame_p95_ms={:.3} frame_max_ms={:.3} ui_clicks=100",
        quantile(&times, 50),
        quantile(&times, 95),
        quantile(&times, 100)
    );
}
