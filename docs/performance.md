# Performance workload

```sh
cargo test --locked overview_hundred_session_workload -- --nocapture
```

This replays 100 headless egui Overview frames with 100 synthetic recorded sessions. It never launches Claude and is not a native GPU/input benchmark. Lists are virtualized and session order/selection remain stable.

Implementation environment: Linux x86_64 container, Rust 1.99.0, debug test build. Initial full-chrome workload measured 181–280 ms total for 100 frames (about 1.8–2.8 ms/frame), across the initial and filtered Overview implementations. This is indicative geometry/layout CPU cost only. CI reruns the reproducible workload; exact numbers will vary.

Real PTY test sends input, issues a terminal cursor-position query, resizes, and verifies `stty size`. It is a correctness check, not throughput/latency proof.

Still required on named Macs: p95 visible input latency under sustained output, Overview cold/cached opening, idle CPU/energy, throughput, resize cost, memory growth, 32 active PTYs, 100 summaries, and reduced motion. Provisional targets remain 60 Hz active interaction, about 32 ms p95 input response, and cached Overview within 100 ms. None is claimed achieved by the headless workload.

## Streaming, scrolling and switching panes

Run the release-mode comparison with the same benchmark source in both checkouts.

```sh
TESSERA_BENCH_DAILY=1 cargo test --release --locked benchmark_daily_terminal_interactions -- --ignored --nocapture
```

On an Apple M4 Pro, macOS 26.7.1 and Rust 1.100.0, three real shell-backed terminal grids each received 16 wrapped lines per frame. One pane was visible at 900 × 480 points. The scroll case delivered pixel deltas during output. The switch case cycled through the three pane identities. Each run used 10 warm-up frames and 120 timed frames per scenario. The table gives the median of three runs' 95th percentiles. Execution order alternated between the two builds.

| Time to construct an egui terminal frame | Main at `8d6d189` | Updated terminal |
| --- | ---: | ---: |
| Follow streamed output | 0.221 ms | 0.105 ms |
| Scroll during output | 0.181 ms | 0.086 ms |
| Switch among three updating panes | 0.177 ms | 0.053 ms |

The timer includes pane interaction, text layout and paint-command construction. Parser injection, workspace chrome, OS event delivery, GPU presentation and display refresh are outside the timer. End-to-end input latency and energy use remain unmeasured.
