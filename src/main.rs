mod choices;
mod directories;
mod discovery;
mod integration;
mod model;
mod search;
mod selection;
mod specs;
mod terminal;
mod ui;
mod updater;
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("hook" | "codex-hook") => {
            let agent = if args[1] == "codex-hook" {
                model::Agent::Codex
            } else {
                model::Agent::Claude
            };
            if let Err(e) = integration::hook(agent) {
                eprintln!("Tessera hook: {e}");
            }
            if agent == model::Agent::Codex {
                println!("{{}}");
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
        Some("codex-hooks") => {
            println!(
                "{}",
                serde_json::to_string_pretty(&integration::settings_for(
                    &std::env::current_exe()?,
                    model::Agent::Codex
                ))?
            );
            Ok(())
        }
        Some("install-codex-hooks" | "uninstall-codex-hooks") => {
            let path = args
                .get(2)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    let home = std::env::var("CODEX_HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|_| {
                            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                                .join(".codex")
                        });
                    home.join("hooks.json")
                });
            integration::install_for(
                &path,
                &std::env::current_exe()?,
                args[1] == "uninstall-codex-hooks",
                model::Agent::Codex,
            )?;
            println!(
                "Updated {}. Review and trust Tessera's definitions with /hooks in Codex.",
                path.display()
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
                "Tessera {}\n\nUsage: tessera [hooks | install-hooks [settings.json] | uninstall-hooks [settings.json]]\n\nNo argument opens the terminal application. Run hooks or codex-hooks to preview integration before installing.\nCodex: tessera codex-hooks | install-codex-hooks [hooks.json] | uninstall-codex-hooks [hooks.json]",
                env!("CARGO_PKG_VERSION")
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
                viewport: main_viewport()
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

fn main_viewport() -> eframe::egui::ViewportBuilder {
    let viewport = eframe::egui::ViewportBuilder::default();
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false);
    viewport
}
