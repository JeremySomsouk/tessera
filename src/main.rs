mod integration;
mod model;
mod terminal;
mod ui;
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("hook") => {
            if let Err(e) = integration::hook() {
                eprintln!("Tessera hook: {e}");
            }
            Ok(())
        }
        Some("hooks") => {
            println!(
                "{}",
                serde_json::to_string_pretty(&integration::settings(&std::env::current_exe()?))?
            );
            Ok(())
        }
        Some("install-hooks" | "uninstall-hooks") => {
            let path = args
                .get(2)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                        .join(".claude/settings.json")
                });
            integration::install(
                &path,
                &std::env::current_exe()?,
                args[1] == "uninstall-hooks",
            )?;
            println!("Updated {} (existing settings backed up)", path.display());
            Ok(())
        }
        Some("--help" | "-h") => {
            println!(
                "Tessera 0.1.0\n\nUsage: tessera [hooks | install-hooks [settings.json] | uninstall-hooks [settings.json]]\n\nNo argument opens the terminal application. Run hooks to preview integration before installing."
            );
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("tessera {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(arg) => anyhow::bail!("unknown command: {arg}; use --help"),
        None => eframe::run_native(
            "Tessera",
            eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default()
                    .with_icon(eframe::icon_data::from_png_bytes(include_bytes!(
                        "../assets/app-icon-window.png"
                    ))?)
                    .with_inner_size([1180.0, 760.0])
                    .with_min_inner_size([640.0, 400.0]),
                ..Default::default()
            },
            Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
        )
        .map_err(|e| anyhow::anyhow!("{e}")),
    }
}
