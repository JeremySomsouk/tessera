struct DebugLog;

impl log::Log for DebugLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Debug && metadata.target().starts_with("tessera::")
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("DEBUG {}: {}", record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let enabled = std::env::var("RUST_LOG").is_ok_and(|value| {
        value.split(',').any(|part| {
            matches!(
                part.trim(),
                "debug" | "trace" | "tessera=debug" | "tessera=trace"
            )
        })
    });
    if enabled && log::set_logger(&DebugLog).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}
