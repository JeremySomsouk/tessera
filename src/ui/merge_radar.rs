use super::*;
use crate::merge_radar::{self as provider, Config, Lane, PullRequest, ReviewSource};
use crate::remote_runs::{Gh, GhCli};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::AtomicBool,
    mpsc::{Receiver, TryRecvError, channel},
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub config: Config,
    snoozes: BTreeMap<String, u64>,
}

impl Preferences {
    fn expire(&mut self, at: u64) -> bool {
        let before = self.snoozes.len();
        self.snoozes.retain(|_, until| *until > at);
        before != self.snoozes.len()
    }

    fn snoozed(&self, url: &str, at: u64) -> bool {
        self.snoozes.get(url).is_some_and(|until| *until > at)
    }
}

struct RefreshResult {
    result: Result<Vec<PullRequest>, String>,
    finished: Instant,
    timestamp: u64,
}

impl RefreshResult {
    fn new(result: Result<Vec<PullRequest>, String>) -> Self {
        Self {
            result,
            finished: Instant::now(),
            timestamp: now(),
        }
    }
}

struct Lookup {
    config: Config,
    receiver: Receiver<RefreshResult>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Lookup {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

struct CancellableGh {
    cli: GhCli,
    cancelled: Arc<AtomicBool>,
}

impl Gh for CancellableGh {
    fn run(&self, args: &[String]) -> Result<String, String> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err("Refresh cancelled".into());
        }
        self.cli.run(args)
    }
}

struct Editor {
    original: Config,
    draft: Config,
    error: String,
}

#[derive(Default)]
pub(super) struct Panel {
    config: Option<Config>,
    prs: Vec<PullRequest>,
    lookup: Option<Lookup>,
    last_attempt: Option<Instant>,
    updated: Option<u64>,
    error: String,
    query: String,
    lane: Option<Lane>,
    editor: Option<Editor>,
}

impl Panel {
    fn sync(&mut self, config: &Config) {
        if self.config.as_ref() != Some(config) {
            if let Some(lookup) = &self.lookup {
                lookup.cancelled.store(true, Ordering::Release);
            }
            self.config = Some(config.clone());
            self.prs.clear();
            self.updated = None;
            self.last_attempt = None;
            self.error.clear();
        }
    }

    fn poll(&mut self) {
        let Some(lookup) = &self.lookup else { return };
        let result = match lookup.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                RefreshResult::new(Err("Refresh stopped unexpectedly. Try again.".into()))
            }
        };
        let matches = self.config.as_ref() == Some(&lookup.config)
            && !lookup.cancelled.load(Ordering::Acquire);
        self.lookup = None;
        if !matches {
            return;
        }
        self.last_attempt = Some(result.finished);
        match result.result {
            Ok(prs) => {
                self.prs = prs;
                self.updated = Some(result.timestamp);
                self.error.clear();
            }
            Err(error) => self.error = error,
        }
    }

    fn refresh(&mut self, config: &Config, ctx: &egui::Context) {
        if self.lookup.is_some() {
            return;
        }
        self.last_attempt = Some(Instant::now());
        if let Err(error) = config.validate() {
            self.error = error;
            return;
        }
        let (sender, receiver) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let gh = CancellableGh {
            cli: GhCli::new(config.gh_path.clone()),
            cancelled: cancelled.clone(),
        };
        let config = config.clone();
        self.lookup = Some(Lookup {
            config: config.clone(),
            receiver,
            cancelled,
        });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = provider::fetch(&gh, &config);
            let _ = sender.send(RefreshResult::new(result));
            ctx.request_repaint();
        });
    }

    fn show(
        &mut self,
        preferences: &mut Preferences,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        paused: bool,
    ) -> bool {
        let mut changed = preferences.expire(now());
        self.sync(&preferences.config);
        self.poll();
        let interval = Duration::from_secs(preferences.config.minutes.clamp(1, 60) * 60);
        if !paused
            && self.editor.is_none()
            && self
                .last_attempt
                .is_none_or(|last| last.elapsed() >= interval)
        {
            self.refresh(&preferences.config, ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.heading("Merge Radar");
            ui.add_space(12.0);
            if ui
                .add_enabled(
                    self.lookup.is_none() && !paused,
                    egui::Button::new("Refresh"),
                )
                .clicked()
            {
                self.refresh(&preferences.config, ctx);
            }
            if ui.button("Radar settings").clicked() {
                self.editor = Some(Editor {
                    original: preferences.config.clone(),
                    draft: preferences.config.clone(),
                    error: String::new(),
                });
            }
            if self.lookup.is_some() {
                ui.spinner();
                ui.weak("Scanning pull requests…");
            } else if let Some(updated) = self.updated {
                ui.weak(format!(
                    "Updated {}",
                    relative_time(now().saturating_sub(updated))
                ));
            }
        });
        ui.label("Your reviews and pull requests, grouped by what needs to happen next.");
        ui.add_space(8.0);
        if !self.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.error);
            if self.updated.is_some() {
                ui.weak("Showing the last successful refresh. Status may have changed.");
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.lane.is_none(), "All").clicked() {
                self.lane = None;
            }
            for lane in Lane::ALL.into_iter().filter(|lane| *lane != Lane::Draft) {
                let count = self
                    .prs
                    .iter()
                    .filter(|pr| pr.lane == lane && !preferences.snoozed(&pr.url, now()))
                    .count();
                if ui
                    .selectable_label(
                        self.lane == Some(lane),
                        RichText::new(format!("{} · {count}", lane.label()))
                            .color(lane_color(ui, lane)),
                    )
                    .clicked()
                {
                    self.lane = if self.lane == Some(lane) {
                        None
                    } else {
                        Some(lane)
                    };
                }
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .desired_width(340.0)
                .hint_text("Filter by title, repository, or PR number"),
        );
        ui.add_space(8.0);
        egui::ScrollArea::vertical().id_salt("merge-radar-page").auto_shrink([false, false]).show(ui, |ui| {
            if let Some(editor) = &mut self.editor {
                match editor.show(&mut preferences.config, ui) {
                    EditorAction::Saved => {
                        changed = true;
                        self.editor = None;
                        self.sync(&preferences.config);
                    }
                    EditorAction::Cancel => self.editor = None,
                    EditorAction::None => {}
                }
                ui.add_space(12.0);
            }
            if self.updated.is_none() && self.prs.is_empty() {
                ui.add_space(40.0);
                if self.lookup.is_some() {
                    ui.heading("Scanning your pull requests");
                    ui.label("Gathering reviews, checks, and feedback from GitHub.");
                } else {
                    ui.heading("Connect through GitHub CLI");
                    ui.label("Install gh and run gh auth login in a terminal, then refresh.");
                }
                return;
            }
            let query = self.query.trim().to_lowercase();
            let filtered: Vec<_> = self.prs.iter().filter(|pr| {
                (self.lane.is_none_or(|lane| pr.lane == lane)) &&
                (query.is_empty() || format!("{} {} #{}", pr.title, pr.repo, pr.number).to_lowercase().contains(&query))
            }).collect();
            let active: Vec<_> = filtered.iter().copied().filter(|pr| !preferences.snoozed(&pr.url, now())).collect();
            if active.is_empty() {
                ui.label(if self.prs.is_empty() { "No open pull requests in this scope." } else { "No active pull requests match this view." });
            }
            let reviews: Vec<_> = active.iter().copied().filter(|pr| pr.lane == Lane::Review).collect();
            if !reviews.is_empty() {
                changed |= lane(ui, Lane::Review, &reviews, preferences);
                ui.add_space(16.0);
            }
            let order = if preferences.config.bot_first { [true, false] } else { [false, true] };
            for bot in order {
                let group: Vec<_> = active.iter().copied().filter(|pr| pr.is_bot == bot && !matches!(pr.lane, Lane::Review | Lane::Draft)).collect();
                if group.is_empty() { continue; }
                ui.label(RichText::new(if bot { "OPENED BY BOTS" } else { "AUTHORED OR ASSIGNED TO ME" }).strong().small());
                ui.add_space(6.0);
                let lanes = [Lane::Me, Lane::Others, Lane::Ready];
                if ui.available_width() >= 960.0 {
                    ui.columns(3, |columns| {
                        for (column, kind) in columns.iter_mut().zip(lanes) {
                            let prs: Vec<_> = group.iter().copied().filter(|pr| pr.lane == kind).collect();
                            changed |= lane(column, kind, &prs, preferences);
                        }
                    });
                } else {
                    for kind in lanes {
                        let prs: Vec<_> = group.iter().copied().filter(|pr| pr.lane == kind).collect();
                        if !prs.is_empty() { changed |= lane(ui, kind, &prs, preferences); }
                    }
                }
                ui.add_space(16.0);
            }
            let drafts: Vec<_> = active.iter().copied().filter(|pr| pr.lane == Lane::Draft).collect();
            if !drafts.is_empty() {
                egui::CollapsingHeader::new(format!("Drafts · {}", drafts.len())).show(ui, |ui| {
                    for pr in drafts { changed |= card(ui, pr, preferences, false); }
                });
            }
            let snoozed: Vec<_> = filtered.iter().copied().filter(|pr| preferences.snoozed(&pr.url, now())).collect();
            if !snoozed.is_empty() {
                egui::CollapsingHeader::new(format!("Snoozed · {}", snoozed.len())).show(ui, |ui| {
                    for pr in snoozed { changed |= card(ui, pr, preferences, true); }
                });
            }
            ui.add_space(12.0);
            ui.weak("Ready to merge is advisory. Confirm current checks and repository rules on GitHub.");
        });
        changed
    }
}

fn lane_color(ui: &egui::Ui, lane: Lane) -> Color32 {
    match (ui.visuals().dark_mode, lane) {
        (true, Lane::Review) => Color32::from_rgb(195, 165, 245),
        (false, Lane::Review) => Color32::from_rgb(112, 64, 176),
        (_, Lane::Me) => ui.visuals().error_fg_color,
        (true, Lane::Others) => Color32::from_rgb(225, 190, 112),
        (false, Lane::Others) => Color32::from_rgb(135, 94, 16),
        (_, Lane::Ready) => accent(ui),
        (_, Lane::Draft) => ui.visuals().weak_text_color(),
    }
}

fn lane(
    ui: &mut egui::Ui,
    kind: Lane,
    prs: &[&PullRequest],
    preferences: &mut Preferences,
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(8)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} · {}", kind.label(), prs.len()))
                        .strong()
                        .color(lane_color(ui, kind)),
                );
                if !prs.is_empty()
                    && matches!(kind, Lane::Others | Lane::Ready)
                    && ui.small_button("Copy links").clicked()
                {
                    ui.ctx().copy_text(
                        prs.iter()
                            .map(|pr| {
                                format!("- {}#{}: {}\n  {}", pr.repo, pr.number, pr.title, pr.url)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            });
            ui.add_space(6.0);
            if prs.is_empty() {
                ui.weak("All clear");
            }
            for pr in prs {
                changed |= card(ui, pr, preferences, false);
            }
        });
    changed
}

fn card(ui: &mut egui::Ui, pr: &PullRequest, preferences: &mut Preferences, snoozed: bool) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .stroke(Stroke::new(
            1.0_f32,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(6)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} #{}", pr.repo, pr.number))
                        .small()
                        .strong(),
                );
                if let Some(updated) = crate::remote_runs::created_at_seconds(&pr.updated_at) {
                    let age = now().saturating_sub(updated);
                    ui.weak(relative_time(age));
                    if age >= preferences.config.idle_days.saturating_mul(86400) {
                        ui.weak("Idle");
                    }
                }
            });
            web_link(ui, RichText::new(&pr.title).strong(), &pr.url);
            ui.horizontal_wrapped(|ui| {
                for blocker in &pr.blockers {
                    web_link(
                        ui,
                        RichText::new(&blocker.label)
                            .small()
                            .color(lane_color(ui, pr.lane)),
                        &blocker.url,
                    );
                }
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if pr.lane == Lane::Review {
                    web_link(ui, RichText::new("Review changes").small(), &pr.review_url);
                }
                if let Some(url) = &pr.failed_check_url {
                    web_link(ui, RichText::new("Open failing check").small(), url);
                }
                if let Some(url) = &pr.feedback_url {
                    web_link(ui, RichText::new("View feedback").small(), url);
                }
                if ui.small_button("Copy review request").clicked() {
                    ui.ctx().copy_text(format!(
                        "Could you review {}#{}: {}?\n{}",
                        pr.repo, pr.number, pr.title, pr.url
                    ));
                }
                if snoozed {
                    if ui.small_button("Restore").clicked() {
                        preferences.snoozes.remove(&pr.url);
                        changed = true;
                    }
                } else if ui.small_button("Snooze 24h").clicked() {
                    preferences
                        .snoozes
                        .insert(pr.url.clone(), now().saturating_add(86400));
                    changed = true;
                }
            });
            if snoozed && let Some(until) = preferences.snoozes.get(&pr.url) {
                ui.weak(format!(
                    "Returns in {}h {}m",
                    until.saturating_sub(now()) / 3600,
                    (until.saturating_sub(now()) % 3600) / 60
                ));
            }
        });
    ui.add_space(6.0);
    changed
}

fn web_link(ui: &mut egui::Ui, label: RichText, url: &str) {
    if (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(char::is_control)
    {
        ui.add(egui::Hyperlink::from_label_and_url(label, url));
    } else {
        ui.label(label);
    }
}

enum EditorAction {
    None,
    Saved,
    Cancel,
}

impl Editor {
    fn show(&mut self, config: &mut Config, ui: &mut egui::Ui) -> EditorAction {
        let mut action = EditorAction::None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Radar settings");
            ui.weak("Saved in Tessera. Uses your existing GitHub CLI login.");
            field(ui, "GitHub CLI path", &mut self.draft.gh_path, "gh");
            field(ui, "Title keyword", &mut self.draft.keyword, "Optional title text");
            field(ui, "Extra GitHub filters", &mut self.draft.extra, "repo:owner/repository -label:wip");
            field(ui, "Ignored checks", &mut self.draft.ignore, "Comma-separated parts of check names");
            ui.horizontal_wrapped(|ui| {
                ui.label("Refresh every");
                ui.add(egui::DragValue::new(&mut self.draft.minutes).range(1..=60).suffix(" min"));
                ui.label("Mark idle after");
                ui.add(egui::DragValue::new(&mut self.draft.idle_days).range(1..=90).suffix(" days"));
            });
            ui.checkbox(&mut self.draft.bot_first, "Show bot pull requests first");
            ui.label("Review source");
            ui.radio_value(&mut self.draft.review_source, ReviewSource::Github, "Direct requests across GitHub");
            ui.radio_value(&mut self.draft.review_source, ReviewSource::Repositories, "My repositories and teams");
            if self.draft.review_source == ReviewSource::Repositories {
                field(ui, "Review repositories", &mut self.draft.review_repos, "owner/web, owner/api");
                field(ui, "Review teams", &mut self.draft.review_teams, "owner/platform");
                field(ui, "Excluded title prefixes", &mut self.draft.review_exclude_prefixes, "chore(deps), WIP:");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Maximum review age");
                    ui.add(egui::DragValue::new(&mut self.draft.review_max_age_days).range(0..=3650).suffix(" days"));
                });
                ui.weak("Title keyword and extra filters apply only to authored and assigned PRs in this mode.");
            }
            if !self.error.is_empty() { ui.colored_label(ui.visuals().error_fg_color, &self.error); }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Save radar settings").clicked() {
                    if *config != self.original {
                        self.error = "Settings changed in another window. Cancel and reopen settings before saving.".into();
                    } else if let Err(error) = self.draft.validate() {
                        self.error = error;
                    } else {
                        *config = self.draft.clone();
                        action = EditorAction::Saved;
                    }
                }
                if ui.button("Cancel").clicked() { action = EditorAction::Cancel; }
                if ui.button("Reset form to defaults").clicked() { self.draft = Config::default(); self.error.clear(); }
            });
        });
        action
    }
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    let label = ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(ui.available_width().min(600.0))
            .hint_text(hint),
    )
    .labelled_by(label.id);
}

impl App {
    pub(super) fn open_merge_radar(&mut self) {
        self.ui.radar_visible = true;
        self.ui.autobot_visible = false;
        self.ui.overview = false;
        self.ui.specifications_view = false;
        self.ui.palette = false;
    }

    pub(super) fn merge_radar_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut panel = self.ui.radar.take().unwrap_or_default();
        self.dirty |= panel.show(&mut self.saved.merge_radar, ui, ctx, self.ui.palette);
        self.ui.radar = Some(panel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> Modifiers {
        if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL | Modifiers::ALT
        }
    }

    fn pr(kind: Lane, number: u64) -> PullRequest {
        let url = format!("https://github.com/example/project/pull/{number}");
        PullRequest {
            number,
            title: "Improve keyboard navigation and preserve selection while reviewing changes"
                .into(),
            repo: "example/project".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            is_bot: number == 6,
            lane: kind,
            blockers: vec![provider::Blocker {
                label: "Review required".into(),
                url: url.clone(),
            }],
            review_url: format!("{url}/files"),
            failed_check_url: None,
            feedback_url: None,
            url,
        }
    }

    fn panel() -> Panel {
        Panel {
            config: Some(Config::default()),
            prs: Lane::ALL
                .into_iter()
                .enumerate()
                .map(|(i, kind)| pr(kind, i as u64 + 1))
                .chain([pr(Lane::Ready, 6)])
                .collect(),
            updated: Some(now()),
            last_attempt: Some(Instant::now()),
            ..Default::default()
        }
    }

    #[test]
    fn settings_and_snoozes_survive_workspace_roundtrip_and_expire() {
        let mut saved = Saved::default();
        saved.merge_radar.config.keyword = "example".into();
        saved
            .merge_radar
            .snoozes
            .insert("https://github.com/example/project/pull/1".into(), 200);
        let mut restored: Saved =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(restored.merge_radar.config.keyword, "example");
        assert!(
            restored
                .merge_radar
                .snoozed("https://github.com/example/project/pull/1", 199)
        );
        assert!(restored.merge_radar.expire(200));
        assert!(restored.merge_radar.snoozes.is_empty());
        let mut legacy = serde_json::to_value(saved).unwrap();
        legacy.as_object_mut().unwrap().remove("merge_radar");
        assert_eq!(
            serde_json::from_value::<Saved>(legacy)
                .unwrap()
                .merge_radar
                .config,
            Config::default()
        );
    }

    #[test]
    fn refresh_keeps_previous_results_on_error_and_discards_changed_scope() {
        let mut panel = panel();
        let (sender, receiver) = channel();
        panel.lookup = Some(Lookup {
            config: Config::default(),
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        sender
            .send(RefreshResult::new(Err("Network unavailable".into())))
            .unwrap();
        panel.poll();
        assert_eq!(panel.prs.len(), 6);
        assert_eq!(panel.error, "Network unavailable");
        assert!(panel.updated.is_some());
        let (sender, receiver) = channel();
        panel.lookup = Some(Lookup {
            config: Config::default(),
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        let config = Config {
            keyword: "different".into(),
            ..Default::default()
        };
        panel.sync(&config);
        sender
            .send(RefreshResult::new(Ok(vec![pr(Lane::Ready, 99)])))
            .unwrap();
        panel.poll();
        assert!(panel.prs.is_empty() && panel.updated.is_none());
        assert!(panel.last_attempt.is_none());
    }

    #[test]
    fn refresh_completed_while_hidden_keeps_its_real_age() {
        let mut panel = panel();
        let (sender, receiver) = channel();
        panel.lookup = Some(Lookup {
            config: Config::default(),
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        let finished = Instant::now() - Duration::from_secs(600);
        let timestamp = now().saturating_sub(600);
        sender
            .send(RefreshResult {
                result: Ok(vec![pr(Lane::Ready, 1)]),
                finished,
                timestamp,
            })
            .unwrap();
        panel.poll();
        assert_eq!(panel.updated, Some(timestamp));
        assert!(panel.last_attempt.unwrap().elapsed() >= Duration::from_secs(600));
    }

    #[test]
    fn radar_renders_within_narrow_and_wide_windows_in_both_themes() {
        for light in [false, true] {
            for width in [640.0, 1180.0] {
                let ctx = egui::Context::default();
                configure_appearance(&ctx, light);
                let mut panel = panel();
                let mut preferences = Preferences::default();
                for _ in 0..3 {
                    let mut content = Rect::NOTHING;
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(width, 900.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                panel.show(&mut preferences, ui, ctx, false);
                                content = ui.min_rect();
                            });
                        },
                    );
                    assert!(
                        content.is_finite() && content.right() <= width + 1.0,
                        "page overflow at {width}: {content:?}"
                    );
                    let labels: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::epaint::Shape::Text(text) => Some(text.galley.text()),
                            _ => None,
                        })
                        .collect();
                    assert!(labels.contains(&"Merge Radar"));
                    assert!(
                        labels
                            .iter()
                            .any(|text| text.starts_with("Needs my review"))
                    );
                    assert!(labels.iter().any(|text| text.starts_with("Ready to merge")));
                }
            }
        }
    }

    #[test]
    fn radar_navigation_isolates_terminal_shortcuts() {
        let ctx = egui::Context::default();
        let mut app = super::super::render_tests::fixture(2);
        app.open_merge_radar();
        app.ui.radar = Some(panel());
        for (key, modifiers) in [
            (Key::D, command()),
            (Key::W, command()),
            (Key::R, command() | Modifiers::SHIFT),
            (Key::Enter, command() | Modifiers::SHIFT),
        ] {
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| app.shortcuts(ctx),
            );
            assert!(app.ui.radar_visible);
            assert!(app.ui.rename.is_none() && app.ui.closing.is_none());
            assert!(!app.ui.maximized);
            assert_eq!(app.saved.workspaces.len(), 2);
        }
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: Key::Num2,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: command(),
                }],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert!(!app.ui.radar_visible);
        assert_eq!(app.ui.active, 1);
        app.open_merge_radar();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        );
        assert!(
            app.ui.radar_visible,
            "Radar must remain visible without Autobot configured"
        );
        app.saved.remote_runs = serde_json::json!({"sources": [{"name":"example", "ticket_pattern":"EX-[0-9]+", "repos":["example/project"]}]});
        app.open_remote_runs(String::new(), &ctx);
        assert!(app.ui.autobot_visible && !app.ui.radar_visible);
    }
}
