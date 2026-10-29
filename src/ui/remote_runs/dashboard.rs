use super::*;

pub(in crate::ui) struct Panel {
    pub(super) config: Result<Config, String>,
    pub(super) config_value: serde_json::Value,
    pub(super) tickets: std::collections::BTreeMap<String, TicketPanel>,
    pub(super) query: String,
    pub(super) error: String,
    pub(super) selected: Option<crate::autobot::GroupId>,
    pub(super) detail_visible: bool,
    pub(super) refresh: crate::autobot::Refresh,
    pub(super) generation: Option<u64>,
    pub(super) clock: Instant,
    pub(super) initialized: bool,
}

impl Panel {
    pub(super) fn new(value: &serde_json::Value) -> Self {
        Self {
            config: Config::parse(value),
            config_value: value.clone(),
            tickets: Default::default(),
            query: String::new(),
            error: String::new(),
            selected: None,
            detail_visible: false,
            refresh: Default::default(),
            generation: None,
            clock: Instant::now(),
            initialized: false,
        }
    }

    pub(super) fn cancel(&mut self) {
        self.refresh.cancel();
        self.generation = None;
        for panel in self.tickets.values_mut() {
            panel.cancelled.store(true, Ordering::Release);
            panel.receiver = None;
            panel.pending = 0;
            panel.output = None;
            panel.comment = None;
            panel.launch = None;
        }
    }

    pub(super) fn sync(
        &mut self,
        preferences: &mut crate::autobot::Preferences,
        value: &serde_json::Value,
    ) {
        preferences.normalize();
        if self.config_value != *value {
            self.cancel();
            self.config_value = value.clone();
            self.config = Config::parse(value);
            self.tickets.clear();
            self.initialized = false;
        }
        self.tickets.retain(|key, _| preferences.keys.contains(key));
        for key in &preferences.keys {
            self.tickets.entry(key.clone()).or_insert_with(|| {
                let mut panel = TicketPanel::new(key.clone(), value);
                panel.key = key.clone();
                panel
            });
        }
        let groups = self.groups();
        if !self
            .selected
            .as_ref()
            .is_some_and(|selected| groups.iter().any(|(id, _)| id == selected))
        {
            self.selected = groups.first().map(|(id, _)| id.clone());
        }
    }

    pub(super) fn groups(
        &self,
    ) -> Vec<(crate::autobot::GroupId, Option<crate::autobot::CachedRepo>)> {
        let Ok(config) = &self.config else {
            return Vec::new();
        };
        let mut groups = Vec::new();
        for (key, panel) in &self.tickets {
            let owners: std::collections::HashMap<_, _> = panel
                .all_runs()
                .into_iter()
                .map(|(repo, source, run)| ((repo, run.id), source))
                .collect();
            if let Ok(tasks) = config.tasks(key) {
                for task in tasks {
                    let mut cached = panel
                        .cached
                        .iter()
                        .find(|cached| {
                            cached.result.repo == task.repo
                                && cached.result.source_name == task.source.name
                        })
                        .cloned();
                    if let Some(cached) = &mut cached {
                        let mut shared = std::collections::BTreeSet::new();
                        cached.result.runs.retain(|run| {
                            let owner = owners.get(&(task.repo.clone(), run.id));
                            if let Some(owner) = owner
                                && *owner != task.source.name
                            {
                                shared.insert(owner.clone());
                                return false;
                            }
                            true
                        });
                        cached.shared_sources = shared.into_iter().collect();
                    }
                    groups.push((
                        crate::autobot::GroupId {
                            source: task.source.name,
                            ticket: key.clone(),
                            repo: task.repo,
                        },
                        cached,
                    ));
                }
            }
        }
        // Keep each source together, with its most actionable ticket first.
        let mut priorities = std::collections::BTreeMap::new();
        for (id, cached) in &groups {
            let priority = cached
                .as_ref()
                .map_or(4, |cached| crate::autobot::priority(&cached.result));
            priorities
                .entry(id.source.clone())
                .and_modify(|current: &mut u8| *current = (*current).min(priority))
                .or_insert(priority);
        }
        groups.sort_by(|(a, ac), (b, bc)| {
            priorities[&a.source]
                .cmp(&priorities[&b.source])
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| {
                    ac.as_ref()
                        .map_or(4, |cached| crate::autobot::priority(&cached.result))
                        .cmp(
                            &bc.as_ref()
                                .map_or(4, |cached| crate::autobot::priority(&cached.result)),
                        )
                })
                .then_with(|| a.ticket.cmp(&b.ticket))
                .then_with(|| a.repo.cmp(&b.repo))
        });
        groups
    }

    pub(super) fn refresh(&mut self, ctx: &egui::Context) {
        if self.tickets.values().any(|panel| panel.pending != 0) {
            return;
        }
        let Some(generation) = self.refresh.start() else {
            return;
        };
        self.generation = Some(generation);
        self.initialized = true;
        self.error.clear();
        for panel in self.tickets.values_mut() {
            panel.lookup(ctx);
        }
    }

    pub(super) fn show(&mut self, app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
        for panel in self.tickets.values_mut() {
            panel.poll_launch(app, ctx);
        }
        if !app.ui.autobot_visible
            || app.ui.palette
            || ctx.input(|input| input.viewport().minimized.unwrap_or(false))
        {
            return;
        }
        if let Some(generation) = self.generation
            && self.tickets.values().all(|panel| panel.pending == 0)
        {
            let failed = self.tickets.values().any(|panel| {
                !panel.error.is_empty() || panel.cached.iter().any(|cached| cached.stale)
            });
            self.refresh
                .finish(generation, self.clock.elapsed().as_secs(), failed);
            self.generation = None;
            // Re-fetch selected progress once per metadata cycle, retaining known jobs on failure.
            for panel in self.tickets.values_mut() {
                if let Some(output) = &panel.output
                    && output.run.status != "completed"
                {
                    let repo = output.repo.clone();
                    let source = output.source.name.clone();
                    let run = panel
                        .results
                        .iter()
                        .filter(|result| result.repo == repo && result.source_name == source)
                        .flat_map(|result| &result.runs)
                        .find(|run| run.id == output.run.id)
                        .cloned();
                    if let Some(run) = run {
                        panel.open_output(repo, source, run, ctx);
                    }
                }
            }
        }
        let posted = self
            .tickets
            .values_mut()
            .any(|panel| std::mem::take(&mut panel.refresh_requested));
        if !self.tickets.is_empty()
            && (!self.initialized
                || posted
                || self
                    .refresh
                    .due(self.clock.elapsed().as_secs(), true, app.saved.autobot.live))
        {
            self.refresh(ctx);
        }
        if app.saved.autobot.live {
            ctx.request_repaint_after(Duration::from_secs(
                self.refresh
                    .next_at
                    .saturating_sub(self.clock.elapsed().as_secs())
                    .clamp(1, 60),
            ));
        }
        ui.heading("Autobot");
        if self
            .selected
            .as_ref()
            .and_then(|selected| self.tickets.get(&selected.ticket))
            .is_some_and(|panel| panel.comment.is_some())
        {
            egui::ScrollArea::vertical()
                .id_salt("autobot-comment")
                .show(ui, |ui| self.detail(app, ui, ctx));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.refresh.active && !self.tickets.is_empty() && self.config.is_ok(),
                    egui::Button::new("Refresh"),
                )
                .clicked()
            {
                self.refresh(ctx);
            }
            if ui
                .checkbox(&mut app.saved.autobot.live, "Live refresh")
                .changed()
            {
                app.dirty = true;
            }
            if self.refresh.active {
                ui.spinner();
                ui.label("Refreshing…");
            } else if app.saved.autobot.live {
                ui.small(format!(
                    "Next refresh in {}s",
                    self.refresh
                        .next_at
                        .saturating_sub(self.clock.elapsed().as_secs())
                ));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Ticket key");
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id_salt("autobot-ticket")
                    .desired_width(180.0)
                    .hint_text("ACME-42"),
            );
            if ui.button("Follow ticket").clicked()
                || (field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
            {
                match &self.config {
                    Ok(config) => match app.saved.autobot.add(&self.query, config) {
                        Ok(changed) => {
                            app.dirty |= changed;
                            let key = self.query.trim().to_owned();
                            self.query.clear();
                            self.error.clear();
                            self.cancel();
                            self.sync(&mut app.saved.autobot, &app.saved.remote_runs);
                            self.selected = self
                                .groups()
                                .into_iter()
                                .find(|(id, _)| id.ticket == key)
                                .map(|(id, _)| id);
                            self.refresh(ctx);
                        }
                        Err(error) => self.error = error,
                    },
                    Err(error) => self.error = error.clone(),
                }
            }
        });
        if let Err(error) = &self.config {
            ui.colored_label(
                attention_color(ui),
                format!("Autobot configuration: {error}. Open Settings to correct it."),
            );
        }
        if !self.error.is_empty() {
            ui.colored_label(attention_color(ui), &self.error);
        }
        if self.tickets.is_empty() {
            ui.label("Follow a ticket to see runs from every matching configured source.");
        }
        ui.horizontal_wrapped(|ui| {
            for key in app.saved.autobot.keys.clone() {
                if ui.button(format!("Remove {key}")).clicked() {
                    self.cancel();
                    app.saved.autobot.keys.retain(|value| value != &key);
                    app.dirty = true;
                    self.sync(&mut app.saved.autobot, &app.saved.remote_runs);
                }
            }
        });
        for (key, panel) in &self.tickets {
            if let Ok(config) = &self.config
                && let Err(error) = config.tasks(key)
            {
                ui.colored_label(attention_color(ui), format!("{key}: {error}"));
            }
            if !panel.error.is_empty() {
                ui.colored_label(attention_color(ui), format!("{key}: {}", panel.error));
            }
        }
        ui.separator();
        let narrow = ui.available_width() < 760.0;
        if narrow && self.detail_visible {
            if ui.button("Back to all bots").clicked() {
                self.detail_visible = false;
            }
            egui::ScrollArea::vertical()
                .id_salt("autobot-detail")
                .show(ui, |ui| self.detail(app, ui, ctx));
        } else if narrow {
            egui::ScrollArea::vertical()
                .id_salt("autobot-overview")
                .show(ui, |ui| self.overview(ui));
        } else {
            ui.columns(2, |columns| {
                egui::ScrollArea::vertical()
                    .id_salt("autobot-overview")
                    .show(&mut columns[0], |ui| self.overview(ui));
                egui::ScrollArea::vertical()
                    .id_salt("autobot-detail")
                    .show(&mut columns[1], |ui| self.detail(app, ui, ctx));
            });
        }
    }

    pub(super) fn overview(&mut self, ui: &mut egui::Ui) {
        let mut previous_source = String::new();
        for (id, cached) in self.groups() {
            if previous_source != id.source {
                ui.heading(&id.source);
                previous_source = id.source.clone();
            }
            ui.push_id((&id.source, &id.ticket, &id.repo), |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    if ui
                        .selectable_label(
                            self.selected.as_ref() == Some(&id),
                            format!("{} · {}", id.ticket, id.repo),
                        )
                        .clicked()
                    {
                        if self.selected.as_ref() != Some(&id) {
                            for panel in self.tickets.values_mut() {
                                panel.output = None;
                                panel.comment = None;
                                panel.selected_run = None;
                            }
                        }
                        self.selected = Some(id.clone());
                        self.detail_visible = true;
                    }
                    if let Some(cached) = cached {
                        if let Some(run) = crate::autobot::latest(&cached.result.runs) {
                            let color = match crate::autobot::priority(&cached.result) {
                                0 | 1 => attention_color(ui),
                                2 => {
                                    if ui.visuals().dark_mode {
                                        ACCENT
                                    } else {
                                        Color32::from_rgb(25, 104, 77)
                                    }
                                }
                                _ => ui.visuals().text_color(),
                            };
                            ui.colored_label(
                                color,
                                format!(
                                    "{} · {}",
                                    run.kind.as_deref().unwrap_or(&run.workflow_name),
                                    crate::autobot::state(run)
                                ),
                            );
                            ui.small(crate::autobot::elapsed(run, now()));
                        } else if !cached.shared_sources.is_empty() {
                            ui.label("Matching runs are listed once under another source");
                        } else {
                            ui.label("No matching runs in lookup window");
                        }
                        for source in &cached.shared_sources {
                            if ui.button(format!("See shared runs in {source}")).clicked() {
                                self.selected = Some(crate::autobot::GroupId {
                                    source: source.clone(),
                                    ticket: id.ticket.clone(),
                                    repo: id.repo.clone(),
                                });
                                self.detail_visible = true;
                            }
                        }
                        ui.small(&cached.change);
                        let refreshed = cached
                            .last_success
                            .map(|at| {
                                format!(
                                    "Last successful refresh: {} (Unix {at})",
                                    relative_time(now().saturating_sub(at))
                                )
                            })
                            .unwrap_or_else(|| "No successful refresh yet".into());
                        if cached.stale {
                            ui.colored_label(
                                attention_color(ui),
                                format!("Stale data. {refreshed}"),
                            );
                        } else {
                            ui.small(refreshed);
                        }
                        for warning in &cached.result.warnings {
                            ui.small(warning);
                        }
                        for error in &cached.result.errors {
                            ui.colored_label(attention_color(ui), error);
                        }
                        ui.horizontal_wrapped(|ui| {
                            for pr in cached.result.prs {
                                ui.hyperlink_to(
                                    format!("PR #{} · {}", pr.number, pr.state),
                                    pr.url,
                                );
                            }
                        });
                    } else {
                        ui.label(if self.refresh.active {
                            "Loading repository…"
                        } else {
                            "Not refreshed yet"
                        });
                    }
                });
            });
        }
    }

    pub(super) fn detail(&mut self, app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(selected) = &self.selected else {
            ui.label("Select a ticket and repository to inspect its runs.");
            return;
        };
        ui.label(
            RichText::new(format!(
                "{} · {} · {}",
                selected.source, selected.ticket, selected.repo
            ))
            .strong(),
        );
        if let Some(panel) = self.tickets.get_mut(&selected.ticket) {
            panel.selected_group = Some((selected.source.clone(), selected.repo.clone()));
            panel.detail(app, ui, ctx);
        }
    }
}
