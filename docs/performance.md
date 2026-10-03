# Performance workload

```sh
cargo test --locked overview_hundred_session_workload -- --nocapture
```

This replays 100 headless egui Overview frames with 100 synthetic recorded sessions. It never launches Claude and is not a native GPU/input benchmark. Lists are virtualized and session order/selection remain stable.

Implementation environment: Linux x86_64 container, Rust 1.99.0, debug test build. Initial full-chrome workload measured 181–280 ms total for 100 frames (about 1.8–2.8 ms/frame), across the initial and filtered Overview implementations. This is indicative geometry/layout CPU cost only. CI reruns the reproducible workload; exact numbers will vary.

Real PTY test sends input, issues a terminal cursor-position query, resizes, and verifies `stty size`. It is a correctness check, not throughput/latency proof.

Still required on named Macs: p95 visible input latency under sustained output, Overview cold/cached opening, idle CPU/energy, throughput, resize cost, memory growth, 32 active PTYs, 100 summaries, and reduced motion. Provisional targets remain 60 Hz active interaction, about 32 ms p95 input response, and cached Overview within 100 ms. None is claimed achieved by the headless workload.
