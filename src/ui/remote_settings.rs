use super::*;
use crate::remote_runs::Config;
use serde_json::{Value, json};

pub(super) struct Editor {
    original: Value,
    enabled: bool,
    gh_path: String,
    run_limit: usize,
    sources: Vec<SourceEditor>,
    error: String,
    recovery: Option<String>,
}

struct SourceEditor {
    original: Value,
    name: String,
    ticket_pattern: String,
    repos: String,
    events: String,
    title_match: String,
    pr_label: String,
    output_job: String,
    output_step: String,
    kinds: Vec<(String, String)>,
}

fn text(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .into()
}

fn lines(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn list(value: &str) -> Vec<&str> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn object(value: &Value) -> Value {
    if value.is_object() {
        value.clone()
    } else {
        json!({})
    }
}

impl SourceEditor {
    fn new(value: &Value) -> Self {
        Self {
            original: value.clone(),
            name: text(value, "name", ""),
            ticket_pattern: text(value, "ticket_pattern", ""),
            repos: lines(value, "repos"),
            events: lines(value, "events"),
            title_match: text(value, "title_match", "{key}\\b"),
            pr_label: text(value, "pr_label", ""),
            output_job: text(value, "output_job", ""),
            output_step: text(value, "output_step", ""),
            kinds: value
                .get("kinds")
                .and_then(Value::as_array)
                .map(|kinds| {
                    kinds
                        .iter()
                        .map(|kind| (text(kind, "label", ""), text(kind, "title_regex", "")))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn value(&self) -> Value {
        let mut value = object(&self.original);
        value["name"] = json!(self.name.trim());
        value["ticket_pattern"] = json!(self.ticket_pattern);
        value["repos"] = json!(list(&self.repos));
        value["events"] = json!(list(&self.events));
        value["title_match"] = json!(self.title_match);
        for (key, content) in [
            ("pr_label", &self.pr_label),
            ("output_job", &self.output_job),
            ("output_step", &self.output_step),
        ] {
            value[key] = if content.is_empty() {
                Value::Null
            } else {
                json!(content)
            };
        }
        value["kinds"] = json!(
            self.kinds
                .iter()
                .map(|(label, pattern)| json!({"label": label, "title_regex": pattern}))
                .collect::<Vec<_>>()
        );
        value
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        field(ui, "Source name", &mut self.name, "acme-ci");
        field(
            ui,
            "Ticket pattern (regular expression)",
            &mut self.ticket_pattern,
            "ACME-[0-9]+",
        );
        ui.small("Matches the complete ticket key.");
        ui.label("Repositories (one owner/name per line)");
        ui.add(
            egui::TextEdit::multiline(&mut self.repos)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("acme/api\nacme/web"),
        );
        egui::CollapsingHeader::new("Matching and output options").show(ui, |ui| {
            field(ui, "Run title pattern", &mut self.title_match, "{key}\\b");
            ui.small("Use {key} for the escaped ticket key.");
            ui.label("GitHub events (one per line, empty means all)");
            ui.add(
                egui::TextEdit::multiline(&mut self.events)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            field(
                ui,
                "Pull request label filter (optional)",
                &mut self.pr_label,
                "",
            );
            field(
                ui,
                "Output job contains (optional)",
                &mut self.output_job,
                "",
            );
            field(
                ui,
                "Output step exact name (optional)",
                &mut self.output_step,
                "",
            );
            ui.label("Run kinds (first matching title pattern wins)");
            let mut remove = None;
            for (index, (label, pattern)) in self.kinds.iter_mut().enumerate() {
                ui.push_id(index, |ui| {
                    field(ui, "Kind label", label, "Build");
                    field(ui, "Title pattern", pattern, "\\[build\\]");
                    if ui.small_button("Remove kind").clicked() {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = remove {
                self.kinds.remove(index);
            }
            if ui.button("Add run kind").clicked() {
                self.kinds.push((String::new(), String::new()));
            }
        });
    }
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .hint_text(hint),
    );
}

impl Editor {
    fn new(value: &Value) -> Self {
        Self {
            original: value.clone(),
            enabled: value
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            gh_path: text(value, "gh_path", "gh"),
            run_limit: value
                .get("run_limit")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(30),
            sources: value
                .get("sources")
                .and_then(Value::as_array)
                .map(|sources| sources.iter().map(SourceEditor::new).collect())
                .unwrap_or_default(),
            error: String::new(),
            recovery: Config::parse(value).err(),
        }
    }

    fn validated(&self, current: &Value) -> Result<Value, String> {
        if current != &self.original {
            return Err(
                "Sources changed in another window. Cancel and reopen the editor before saving."
                    .into(),
            );
        }
        let mut value = object(&self.original);
        value["enabled"] = json!(self.enabled);
        value["gh_path"] = json!(self.gh_path.trim());
        value["run_limit"] = json!(self.run_limit);
        value["sources"] = json!(
            self.sources
                .iter()
                .map(SourceEditor::value)
                .collect::<Vec<_>>()
        );
        Config::parse(&value)?;
        Ok(value)
    }

    fn save(&self, saved: &mut Saved) -> Result<bool, String> {
        let value = self.validated(&saved.remote_runs)?;
        let changed = saved.remote_runs != value;
        saved.remote_runs = value;
        Ok(changed)
    }

    fn show(&mut self, ui: &mut egui::Ui) -> (bool, bool) {
        if let Some(error) = &self.recovery {
            ui.colored_label(attention_color(ui), error);
            ui.label("The form keeps readable fields. Saving replaces malformed fields with the values shown here; Cancel keeps your original configuration.");
        }
        ui.checkbox(
            &mut self.enabled,
            "Show Autobot when sources are configured",
        );
        field(ui, "GitHub CLI executable", &mut self.gh_path, "gh");
        ui.small("Authentication stays with gh. Sign in from your terminal with gh auth login.");
        ui.horizontal_wrapped(|ui| {
            ui.label("Runs per repository and event");
            ui.add(egui::DragValue::new(&mut self.run_limit).range(1..=1000));
        });
        let mut remove = None;
        for (index, source) in self.sources.iter_mut().enumerate() {
            ui.push_id(("autobot-source", index), |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.strong(format!("Source {}", index + 1));
                    source.show(ui);
                    if ui.button("Remove source").clicked() {
                        remove = Some(index);
                    }
                });
            });
        }
        if let Some(index) = remove {
            self.sources.remove(index);
        }
        if ui
            .add_enabled(self.sources.len() < 32, egui::Button::new("Add source"))
            .clicked()
        {
            self.sources.push(SourceEditor::new(&Value::Null));
        }
        if self.sources.is_empty() {
            ui.label("Add a source to make Autobot available.");
        }
        if !self.error.is_empty() {
            ui.colored_label(attention_color(ui), &self.error);
        }
        ui.horizontal_wrapped(|ui| {
            (
                primary_button(ui, "Save Autobot sources").clicked(),
                ui.button("Cancel source edits").clicked(),
            )
        })
        .inner
    }
}

impl App {
    pub(super) fn remote_settings(&mut self, ui: &mut egui::Ui) -> bool {
        section_label(ui, "AUTOBOT SOURCES");
        if self.ui.remote_settings.is_none() {
            match Config::parse(&self.saved.remote_runs) {
                Ok(config) => {
                    ui.label(format!("{} configured sources", config.sources.len()));
                }
                Err(error) => {
                    ui.colored_label(attention_color(ui), error);
                }
            }
            if ui.button("Configure Autobot sources").clicked() {
                self.ui.remote_settings = Some(Editor::new(&self.saved.remote_runs));
            }
            return false;
        }
        let mut editor = self.ui.remote_settings.take().unwrap();
        let (save, cancel) = editor.show(ui);
        if cancel {
            return false;
        }
        if save {
            match editor.save(&mut self.saved) {
                Ok(changed) => {
                    self.dirty |= changed;
                    return changed;
                }
                Err(error) => editor.error = error,
            }
        }
        self.ui.remote_settings = Some(editor);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Value {
        json!({"enabled":true,"extension":"keep","sources":[{"name":"acme-ci","ticket_pattern":"ACME-[0-9]+","repos":["acme/api"],"events":["push"],"title_match":"{key}\\b","pr_label":"automation","output_job":"Build","output_step":"Test","kinds":[{"label":"test","title_regex":"test"}],"extension":"source"}]})
    }

    #[test]
    fn form_round_trips_existing_configuration_and_preserves_extensions() {
        let value = config();
        let editor = Editor::new(&value);
        let saved = editor.validated(&value).unwrap();
        let parsed = Config::parse(&saved).unwrap();
        assert_eq!(parsed.tasks("ACME-42").unwrap().len(), 1);
        assert_eq!(parsed.sources[0].events, vec!["push"]);
        assert_eq!(parsed.sources[0].pr_label.as_deref(), Some("automation"));
        assert_eq!(parsed.sources[0].output_job.as_deref(), Some("Build"));
        assert_eq!(parsed.sources[0].output_step.as_deref(), Some("Test"));
        assert_eq!(parsed.sources[0].kinds[0].title_regex, "test");
        assert_eq!(saved["extension"], "keep");
        assert_eq!(saved["sources"][0]["extension"], "source");
    }

    #[test]
    fn validation_rejects_bad_patterns_repositories_and_concurrent_edits() {
        let value = config();
        let mut editor = Editor::new(&value);
        editor.sources[0].ticket_pattern = "[".into();
        assert!(
            editor
                .validated(&value)
                .unwrap_err()
                .contains("ticket_pattern")
        );
        editor.sources[0].ticket_pattern = "ACME-[0-9]+".into();
        editor.sources[0].repos = "invalid".into();
        assert!(editor.validated(&value).unwrap_err().contains("owner/name"));
        editor.sources[0].repos = "acme/api".into();
        assert!(
            editor
                .validated(&json!({}))
                .unwrap_err()
                .contains("another window")
        );
        editor.sources[0].title_match = "ACME".into();
        assert!(editor.validated(&value).unwrap_err().contains("{key}"));
    }

    #[test]
    fn malformed_configuration_can_be_repaired_without_changing_saved_state() {
        let value = json!({"sources":"bad", "extension":42});
        let mut editor = Editor::new(&value);
        assert!(editor.recovery.is_some());
        editor
            .sources
            .push(SourceEditor::new(&config()["sources"][0]));
        let repaired = editor.validated(&value).unwrap();
        assert_eq!(repaired["extension"], 42);
        assert_eq!(value["sources"], "bad");
        assert!(Config::parse(&repaired).unwrap().visible());
    }

    #[test]
    fn saving_changes_only_source_preferences_and_invalid_edits_change_nothing() {
        let mut saved = Saved {
            remote_runs: config(),
            light: true,
            font_size: 18.0,
            folder_search_roots: vec!["/tmp/acme".into()],
            ..Saved::default()
        };
        let mut before = serde_json::to_value(&saved).unwrap();
        let mut editor = Editor::new(&saved.remote_runs);
        editor.sources[0].repos = "acme/web".into();
        assert!(editor.save(&mut saved).unwrap());
        assert_eq!(
            saved.remote_runs["sources"][0]["repos"],
            json!(["acme/web"])
        );
        let mut after = serde_json::to_value(&saved).unwrap();
        before.as_object_mut().unwrap().remove("remote_runs");
        after.as_object_mut().unwrap().remove("remote_runs");
        assert_eq!(before, after);
        let before = serde_json::to_value(&saved).unwrap();
        let mut editor = Editor::new(&saved.remote_runs);
        editor.sources[0].ticket_pattern = "[".into();
        assert!(editor.save(&mut saved).is_err());
        assert_eq!(before, serde_json::to_value(&saved).unwrap());
    }

    #[test]
    fn source_form_renders_narrow_and_wide_in_both_themes() {
        for light in [false, true] {
            for width in [260.0, 640.0, 1000.0] {
                let ctx = egui::Context::default();
                configure_appearance(&ctx, light);
                let mut editor = Editor::new(&config());
                for _ in 0..3 {
                    let mut bounds = Rect::NOTHING;
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(width, 1400.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                editor.show(ui);
                                bounds = ui.min_rect();
                            });
                        },
                    );
                    assert!(!output.shapes.is_empty());
                    assert!(
                        bounds.right() <= width + 1.0,
                        "form overflow at {width}: {bounds:?}"
                    );
                }
            }
        }
    }
}
