use super::*;
mod dashboard;
use crate::remote_runs::{self as provider, Config, GhCli, Job, PullRequest, RepoResult, Run};
pub(super) use dashboard::Panel;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{Receiver, TryRecvError, channel},
};

static ACTIVE_REQUESTS: AtomicUsize = AtomicUsize::new(0);
struct RequestPermit;
impl Drop for RequestPermit {
    fn drop(&mut self) {
        ACTIVE_REQUESTS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RequestGh<G> {
    inner: G,
    cancelled: Arc<AtomicBool>,
}

impl<G: provider::Gh> provider::Gh for RequestGh<G> {
    fn run(&self, args: &[String]) -> Result<String, String> {
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return Err("Lookup cancelled".into());
            }
            let active = ACTIVE_REQUESTS.load(Ordering::Acquire);
            if active < 8
                && ACTIVE_REQUESTS
                    .compare_exchange(active, active + 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                let _permit = RequestPermit;
                if self.cancelled.load(Ordering::Acquire) {
                    return Err("Lookup cancelled".into());
                }
                return self.inner.run(args);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

type LaunchResult = anyhow::Result<(Uuid, PathBuf, Terminal)>;

struct TicketPanel {
    cancelled: Arc<AtomicBool>,
    query: String,
    key: String,
    config: Result<Config, String>,
    pending: usize,
    receiver: Option<Receiver<RepoResult>>,
    results: Vec<RepoResult>,
    cached: Vec<crate::autobot::CachedRepo>,
    selected_group: Option<(String, String)>,
    selected_run: Option<u64>,
    refresh_requested: bool,
    error: String,
    notice: String,
    output: Option<Output>,
    comment: Option<Comment>,
    launch: Option<Receiver<LaunchResult>>,
}

struct Output {
    cancelled: Arc<AtomicBool>,
    repo: String,
    run: Run,
    jobs: Vec<Job>,
    receiver: Option<Receiver<Result<Vec<Job>, String>>>,
    job: Option<u64>,
    step: Option<String>,
    source: provider::Source,
    error: String,
}

struct Comment {
    repo: String,
    pr: PullRequest,
    body: String,
    preview: Option<provider::CommentPreview>,
    posting: Option<Receiver<Result<(), String>>>,
    error: String,
}

fn available(value: &serde_json::Value) -> bool {
    if value.is_null() || value.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        return false;
    }
    match value.get("sources") {
        Some(serde_json::Value::Array(sources)) => !sources.is_empty(),
        Some(_) => true,
        None => !value.is_object(),
    }
}

impl Drop for TicketPanel {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl TicketPanel {
    fn new(query: String, value: &serde_json::Value) -> Self {
        Self {
            cancelled: Arc::default(),
            query,
            key: String::new(),
            config: Config::parse(value),
            pending: 0,
            receiver: None,
            results: Vec::new(),
            cached: Vec::new(),
            selected_group: None,
            selected_run: None,
            refresh_requested: false,
            error: String::new(),
            notice: String::new(),
            output: None,
            comment: None,
            launch: None,
        }
    }

    fn lookup(&mut self, ctx: &egui::Context) {
        let tasks = match &self.config {
            Ok(config) => config.tasks(self.query.trim()),
            Err(error) => Err(error.clone()),
        };
        let tasks = match tasks {
            Ok(tasks) => tasks,
            Err(error) => {
                self.error = error;
                return;
            }
        };
        let config = self.config.as_ref().unwrap();
        std::mem::take(&mut self.cancelled).store(true, Ordering::Release);
        self.key = self.query.trim().to_owned();

        self.error.clear();
        let (tx, rx) = channel();
        self.pending = tasks.len();
        self.receiver = Some(rx);
        let key = self.key.clone();
        let path = config.gh_path.clone();
        let limit = config.run_limit;
        let ctx = ctx.clone();
        let cancelled = self.cancelled.clone();
        std::thread::spawn(move || {
            let gh = RequestGh {
                inner: GhCli::new(path),
                cancelled: cancelled.clone(),
            };
            for task in tasks {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                let result = provider::lookup(&gh, &task, &key, limit);
                if tx.send(result).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    }

    fn poll(&mut self, _ctx: &egui::Context) {
        if let Some(receiver) = &self.receiver {
            loop {
                match receiver.try_recv() {
                    Ok(result) => {
                        if let Some(cached) = self.cached.iter_mut().find(|cached| {
                            cached.result.repo == result.repo
                                && cached.result.source_name == result.source_name
                        }) {
                            cached.update(result, now());
                        } else {
                            self.cached
                                .push(crate::autobot::CachedRepo::new(result, now()));
                        }
                        self.results = self
                            .cached
                            .iter()
                            .map(|cached| cached.result.clone())
                            .collect();
                        self.pending = self.pending.saturating_sub(1);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if self.pending != 0 {
                            self.error = "A lookup worker stopped. Refresh to retry.".into();
                            self.pending = 0;
                        }
                        break;
                    }
                }
            }
        }
        if let Some(output) = &mut self.output
            && let Some(receiver) = &output.receiver
        {
            match receiver.try_recv() {
                Ok(Ok(jobs)) => {
                    let selected =
                        (!output.jobs.is_empty()).then(|| (output.job, output.step.clone()));
                    output.jobs = jobs;
                    output.job = output
                        .source
                        .output_job
                        .as_ref()
                        .and_then(|name| {
                            output
                                .jobs
                                .iter()
                                .find(|job| job.name.contains(name))
                                .map(|job| job.id)
                        })
                        .or_else(|| {
                            if output.source.output_job.is_some() {
                                return None;
                            }
                            output.source.output_step.as_ref().and_then(|name| {
                                output
                                    .jobs
                                    .iter()
                                    .find(|job| job.steps.iter().any(|step| step.name == *name))
                                    .map(|job| job.id)
                            })
                        });
                    output.step = output
                        .job
                        .and_then(|id| output.jobs.iter().find(|job| job.id == id))
                        .and_then(|job| {
                            output.source.output_step.as_ref().and_then(|name| {
                                job.steps
                                    .iter()
                                    .find(|step| step.name == *name)
                                    .map(|step| step.name.clone())
                            })
                        });
                    if let Some((job, step)) = selected {
                        output.job = job.filter(|id| output.jobs.iter().any(|job| job.id == *id));
                        output.step = step.filter(|name| {
                            output
                                .jobs
                                .iter()
                                .find(|job| Some(job.id) == output.job)
                                .is_some_and(|job| job.steps.iter().any(|step| step.name == *name))
                        });
                    }
                    if output.source.output_job.is_some() && output.job.is_none() {
                        output.error = "Configured output job was not found. Choose a job or open the full log.".into();
                    } else if output.source.output_step.is_some() && output.step.is_none() {
                        output.error = "Configured output step was not found. Choose a step or open the whole job/full log.".into();
                    }
                    output.receiver = None;
                }
                Ok(Err(error)) => {
                    output.error = error;
                    output.receiver = None;
                }
                Err(TryRecvError::Disconnected) => {
                    output.error = "Job lookup stopped. Open the full log or retry.".into();
                    output.receiver = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let posted = self.comment.as_mut().and_then(|comment| {
            comment
                .posting
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Disconnected) => Some(Err(
                        "Comment worker stopped. Check the PR before retrying.".into(),
                    )),
                    Err(TryRecvError::Empty) => None,
                })
        });
        if let Some(result) = posted {
            match result {
                Ok(()) => {
                    self.comment = None;
                    self.notice =
                        "Comment posted. Refresh again when the follow-up run starts.".into();
                    self.refresh_requested = true;
                }
                Err(error) => {
                    let comment = self.comment.as_mut().unwrap();
                    comment.posting = None;
                    comment.error = error;
                }
            }
        }
    }

    fn all_runs(&self) -> Vec<(String, String, Run)> {
        let mut rows: Vec<_> = self
            .results
            .iter()
            .flat_map(|result| {
                result
                    .runs
                    .iter()
                    .cloned()
                    .map(|run| (result.repo.clone(), result.source_name.clone(), run))
            })
            .collect();
        rows.sort_by(|a, b| {
            b.2.created_at
                .cmp(&a.2.created_at)
                .then_with(|| a.0.cmp(&b.0))
                .then_with(|| b.2.id.cmp(&a.2.id))
                .then_with(|| {
                    let rank = |source: &str| {
                        self.config
                            .as_ref()
                            .ok()
                            .and_then(|config| {
                                config.sources.iter().position(|entry| entry.name == source)
                            })
                            .unwrap_or(usize::MAX)
                    };
                    rank(&a.1).cmp(&rank(&b.1)).then_with(|| a.1.cmp(&b.1))
                })
        });
        let mut seen = std::collections::HashSet::new();
        rows.retain(|(repo, _, run)| seen.insert((repo.clone(), run.id)));
        rows
    }

    fn runs(&self) -> Vec<(String, String, Run)> {
        self.all_runs()
            .into_iter()
            .filter(|(repo, source, _)| {
                self.selected_group
                    .as_ref()
                    .is_none_or(|(selected_source, selected_repo)| {
                        source == selected_source && repo == selected_repo
                    })
            })
            .collect()
    }

    fn prs(&self) -> Vec<(String, PullRequest)> {
        let mut seen = std::collections::HashSet::new();
        self.results
            .iter()
            .filter(|result| {
                self.selected_group.as_ref().is_none_or(|(source, repo)| {
                    result.source_name == *source && result.repo == *repo
                })
            })
            .flat_map(|result| {
                result
                    .prs
                    .iter()
                    .cloned()
                    .map(|pr| (result.repo.clone(), pr))
            })
            .filter(|(repo, pr)| seen.insert((repo.clone(), pr.number)))
            .collect()
    }

    fn open_output(&mut self, repo: String, source_name: String, run: Run, ctx: &egui::Context) {
        let config = self.config.as_ref().unwrap();
        let Some(source) = config
            .sources
            .iter()
            .find(|source| source.name == source_name)
            .cloned()
        else {
            return;
        };
        let (tx, receiver) = channel();
        let path = config.gh_path.clone();
        let worker_repo = repo.clone();
        let id = run.id;
        let cancelled = Arc::default();
        let worker_cancelled = Arc::clone(&cancelled);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider::jobs(
                &RequestGh {
                    inner: GhCli::new(path),
                    cancelled: worker_cancelled,
                },
                &worker_repo,
                id,
            ));
            ctx.request_repaint();
        });
        let previous_selection = self
            .output
            .as_ref()
            .filter(|output| output.repo == repo && output.run.id == run.id)
            .map(|output| (output.job, output.step.clone()))
            .unwrap_or_default();
        let previous_jobs = self
            .output
            .as_ref()
            .filter(|output| output.repo == repo && output.run.id == run.id)
            .map(|output| output.jobs.clone())
            .unwrap_or_default();
        self.output = Some(Output {
            cancelled,
            repo,
            run,
            jobs: previous_jobs,
            receiver: Some(receiver),
            job: previous_selection.0,
            step: previous_selection.1,
            source,
            error: String::new(),
        });
    }

    fn launch(&mut self, app: &mut App, ctx: &egui::Context, command: Result<String, String>) {
        let command = match command {
            Ok(command) => command,
            Err(error) => {
                self.error = error;
                return;
            }
        };
        if self.launch.is_some() {
            return;
        }
        if app.panes.len() + usize::from(app.folder_launch.is_some()) >= 32 {
            self.error = "Pane limit (32) reached. Close a terminal and try again.".into();
            return;
        }
        let path = app
            .focused_directory()
            .ok()
            .and_then(|directory| crate::directories::resolve(&directory).ok())
            .unwrap_or_else(crate::directories::default_directory);
        let socket = app
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.path.clone())
            .unwrap_or_default();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        app.remote_workers.spawn(move || {
            let id = Uuid::new_v4();
            let result = Terminal::spawn_remote(id, &path, &socket, ctx.clone(), &command)
                .map(|terminal| (id, path, terminal));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.launch = Some(rx);
    }

    fn poll_launch(&mut self, app: &mut App, ctx: &egui::Context) {
        self.poll(ctx);
        if let Some(receiver) = &self.launch {
            match receiver.try_recv() {
                Ok(Ok((id, path, terminal))) => {
                    if app.panes.len() + usize::from(app.folder_launch.is_some()) >= 32 {
                        self.error = "Pane limit (32) reached while opening log.".into();
                        self.launch = None;
                    } else {
                        app.remote_panes.insert(id);
                        app.panes.insert(
                            id,
                            Pane {
                                terminal,
                                search: TerminalSearch::default(),
                            },
                        );
                        if let Some(workspace) = app.saved.workspaces.get_mut(app.ui.active) {
                            workspace.layout.split(workspace.focus, id, true);
                            workspace.focus = id;
                        } else {
                            app.saved.workspaces.push(Workspace {
                                task: Task {
                                    id: Uuid::new_v4(),
                                    title: "Remote run".into(),
                                    directory: path.to_string_lossy().into_owned(),
                                    state: TaskState::Implementing,
                                },
                                custom_title: Some(true),
                                layout: Layout::Pane(id),
                                focus: id,
                            });
                            app.register_workspace_window();
                            app.ui.active = app.saved.workspaces.len() - 1;
                        }
                        app.ui.overview = false;
                        app.ui.specifications_view = false;
                        app.ui.maximized = false;
                        app.dirty = true;
                        ctx.memory_mut(|memory| {
                            if let Some(id) = memory.focused() {
                                memory.surrender_focus(id);
                            }
                        });
                        self.launch = None;
                        app.ui.autobot_visible = false;
                    }
                }
                Ok(Err(error)) => {
                    self.error = format!("Cannot open remote terminal: {error}");
                    self.launch = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = "Remote terminal worker stopped. Try again.".into();
                    self.launch = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    fn detail(&mut self, app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.comment.is_some() {
            self.comment_ui(app, ui, ctx);
            return;
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        if !self.error.is_empty() {
            ui.colored_label(attention_color(ui), &self.error);
        }
        ui.heading("Run timeline");
        let runs = self.runs();
        if runs.is_empty() {
            ui.label("No matching runs in the configured lookup window.");
        }
        if !self
            .selected_run
            .is_some_and(|id| runs.iter().any(|(_, _, run)| run.id == id))
        {
            self.selected_run = runs.first().map(|(_, _, run)| run.id);
            self.output = None;
        }
        for (repo, source, run) in runs {
            let selected = self.selected_run == Some(run.id);
            let label = format!(
                "{} · {} · {}",
                run.title,
                run.kind.as_deref().unwrap_or(&run.workflow_name),
                crate::autobot::state(&run)
            );
            if ui.selectable_label(selected, label).clicked() {
                self.selected_run = Some(run.id);
                self.output = None;
            }
            ui.small(format!(
                "{} · {}",
                timestamp_age(&run.created_at),
                crate::autobot::elapsed(&run, now())
            ));
            if self.selected_run == Some(run.id) {
                if run.status != "completed"
                    && self
                        .output
                        .as_ref()
                        .is_none_or(|output| output.run.id != run.id || output.repo != repo)
                {
                    self.open_output(repo.clone(), source.clone(), run.clone(), ctx);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to("Open run in browser", &run.url);
                    let path = self.config.as_ref().unwrap().gh_path.clone();
                    if run.status != "completed" && ui.button("Watch run").clicked() {
                        self.launch(app, ctx, provider::watch_command(&path, &repo, run.id));
                    }
                    if ui.button("Output…").clicked() {
                        self.open_output(repo.clone(), source, run.clone(), ctx);
                    }
                    if ui.button("Full log").clicked() {
                        self.launch(
                            app,
                            ctx,
                            provider::log_command(&path, &repo, run.id, None, None),
                        );
                    }
                });
                if let Some(output) = &self.output
                    && output.run.id == run.id
                {
                    for job in &output.jobs {
                        let completed = job
                            .steps
                            .iter()
                            .filter(|step| step.status == "completed")
                            .count();
                        ui.label(format!(
                            "{} · {} / {} · {}/{} known steps completed",
                            job.name,
                            job.status,
                            job.conclusion.as_deref().unwrap_or("pending"),
                            completed,
                            job.steps.len()
                        ));
                        for step in job.steps.iter().filter(|step| step.status == "in_progress") {
                            ui.label(format!("Current step: {}. {}", step.number, step.name));
                        }
                    }
                }
                self.output_ui(app, ui, ctx);
            }
            ui.separator();
        }
        if !self.prs().is_empty() {
            ui.heading("Linked pull requests");
        }
        for (repo, pr) in self.prs() {
            ui.label(format!(
                "{}#{} · {} · {}",
                repo, pr.number, pr.title, pr.state
            ));
            ui.small(format!("Branch: {}", pr.head_ref_name));
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to("Open PR in browser", &pr.url);
                if ui.button("Write comment…").clicked() {
                    self.comment = Some(Comment {
                        repo,
                        pr,
                        body: String::new(),
                        preview: None,
                        posting: None,
                        error: String::new(),
                    });
                }
            });
        }
    }

    fn output_ui(&mut self, app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut command = None;
        let mut clear = false;
        if let Some(output) = &mut self.output {
            ui.separator();
            ui.label(RichText::new(format!("Output: {}#{}", output.repo, output.run.id)).strong());
            if output.receiver.is_some() {
                ui.spinner();
                ui.label("Loading jobs…");
            }
            if !output.error.is_empty() {
                ui.label(&output.error);
            }
            let old_job = output.job;
            egui::ComboBox::from_id_salt("remote-output-job")
                .selected_text(
                    output
                        .jobs
                        .iter()
                        .find(|job| Some(job.id) == output.job)
                        .map(|job| job.name.as_str())
                        .unwrap_or("Full run"),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut output.job, None, "Full run");
                    for job in &output.jobs {
                        ui.selectable_value(
                            &mut output.job,
                            Some(job.id),
                            format!(
                                "{} · {} / {}",
                                job.name,
                                job.status,
                                job.conclusion.as_deref().unwrap_or("pending")
                            ),
                        );
                    }
                });
            if old_job != output.job {
                output.step = None;
            }
            if let Some(job) = output.jobs.iter().find(|job| Some(job.id) == output.job) {
                egui::ComboBox::from_id_salt("remote-output-step")
                    .selected_text(output.step.as_deref().unwrap_or("Whole job"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut output.step, None, "Whole job");
                        for step in &job.steps {
                            ui.selectable_value(
                                &mut output.step,
                                Some(step.name.clone()),
                                format!(
                                    "{}. {} · {} / {}",
                                    step.number,
                                    step.name,
                                    step.status,
                                    step.conclusion.as_deref().unwrap_or("pending")
                                ),
                            );
                        }
                    });
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        output.receiver.is_none(),
                        egui::Button::new("Open selected output"),
                    )
                    .clicked()
                {
                    command = Some(provider::log_command(
                        &self.config.as_ref().unwrap().gh_path,
                        &output.repo,
                        output.run.id,
                        output.job,
                        output.step.as_deref(),
                    ));
                }
                clear = ui.button("Close output selection").clicked();
            });
            ui.separator();
        }
        if clear {
            self.output = None;
        }
        if let Some(command) = command {
            self.launch(app, ctx, command);
        }
    }

    fn comment_ui(&mut self, app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut cancel = false;
        let comment = self.comment.as_mut().unwrap();
        ui.label(
            RichText::new(format!(
                "{}#{}: {}",
                comment.repo, comment.pr.number, comment.pr.title
            ))
            .strong(),
        );
        ui.label("Posting may start a paid agent run, create commits, and merge changes if auto-merge is armed.");
        if comment.pr.needs_warning() {
            if comment.pr.auto_merge_enabled {
                ui.colored_label(
                    attention_color(ui),
                    "Warning: auto-merge is armed on this pull request.",
                );
            }
            if comment.pr.state != "OPEN" {
                ui.colored_label(
                    attention_color(ui),
                    format!("Warning: this pull request is {}.", comment.pr.state),
                );
            }
        }
        if !comment.error.is_empty() {
            ui.colored_label(attention_color(ui), &comment.error);
        }
        if comment.posting.is_some() {
            ui.spinner();
            ui.label("Posting confirmed comment…");
            return;
        }
        if let Some(preview) = &comment.preview {
            ui.heading("Preview comment");
            ui.small(format!("Target: {}#{}", preview.repo(), preview.pr()));
            ui.label(preview.body());
            ui.horizontal_wrapped(|ui| {
                if ui.button("Back to editor").clicked() {
                    comment.preview = None;
                }
                if ui.button("Confirm and post comment").clicked()
                    && let Some(preview) = comment.preview.take()
                {
                    let confirmed = preview.confirm();
                    let path = self.config.as_ref().unwrap().gh_path.clone();
                    let (tx, rx) = channel();
                    let ctx = ctx.clone();
                    app.remote_workers.spawn(move || {
                        let _ = tx.send(provider::post_comment(&GhCli::new(path), &confirmed));
                        ctx.request_repaint();
                    });
                    comment.posting = Some(rx);
                }
            });
        } else {
            ui.label("Comment (Markdown)");
            ui.add(
                egui::TextEdit::multiline(&mut comment.body)
                    .id_salt("remote-comment-editor")
                    .desired_width(f32::INFINITY)
                    .desired_rows(10)
                    .code_editor(),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !comment.body.trim().is_empty(),
                        egui::Button::new("Preview comment"),
                    )
                    .clicked()
                {
                    match provider::CommentPreview::new(
                        &comment.repo,
                        comment.pr.number,
                        comment.body.clone(),
                    ) {
                        Ok(preview) => {
                            comment.preview = Some(preview);
                            comment.error.clear();
                        }
                        Err(error) => comment.error = error,
                    }
                }
                cancel = ui.button("Cancel comment").clicked();
            });
        }
        if cancel {
            self.comment = None;
        }
    }
}

fn timestamp_age(timestamp: &str) -> String {
    provider::created_at_seconds(timestamp)
        .map(|at| relative_time(now().saturating_sub(at)))
        .unwrap_or_else(|| timestamp.to_owned())
}

impl App {
    pub(super) fn remote_available(&self) -> bool {
        available(&self.saved.remote_runs)
    }

    pub(super) fn open_remote_runs(&mut self, key: String, ctx: &egui::Context) {
        let mut panel = self
            .ui
            .remote
            .take()
            .unwrap_or_else(|| Panel::new(&self.saved.remote_runs));
        panel.sync(&mut self.saved.autobot, &self.saved.remote_runs);
        if !key.trim().is_empty() {
            match &panel.config {
                Ok(config) => match self.saved.autobot.add(&key, config) {
                    Ok(changed) => {
                        self.dirty |= changed;
                        panel.sync(&mut self.saved.autobot, &self.saved.remote_runs);
                        panel.detail_visible = true;
                        panel.selected = panel
                            .groups()
                            .into_iter()
                            .find(|(id, _)| id.ticket == key.trim())
                            .map(|(id, _)| id);
                        if changed {
                            panel.cancel();
                            panel.refresh(ctx);
                        }
                    }
                    Err(error) => panel.error = error,
                },
                Err(error) => panel.error = error.clone(),
            }
        }
        self.ui.remote = Some(panel);
        self.ui.palette = false;
        self.ui.autobot_visible = true;
        self.ui.radar_visible = false;
        self.ui.overview = false;
        self.ui.specifications_view = false;
    }

    pub(super) fn suspend_autobot(&mut self) {
        if let Some(panel) = &mut self.ui.remote {
            panel.cancel();
        }
    }

    pub(super) fn autobot_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut panel = self
            .ui
            .remote
            .take()
            .unwrap_or_else(|| Panel::new(&self.saved.remote_runs));
        panel.sync(&mut self.saved.autobot, &self.saved.remote_runs);
        panel.show(self, ui, ctx);
        self.ui.remote = Some(panel);
    }

    pub(super) fn remote_work(&mut self, index: usize, ui: &mut egui::Ui) {
        if !self.remote_available() {
            return;
        }
        let work = &mut self.saved.work_items[index];
        ui.label("Ticket key (optional)");
        let mut key = work.ticket_key.clone().unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::singleline(&mut key)
                    .id_salt(("work-ticket", work.id))
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            work.ticket_key = (!key.trim().is_empty()).then(|| key.trim().to_owned());
            self.dirty = true;
        }
        if ui
            .add_enabled(
                !key.trim().is_empty(),
                egui::Button::new("Open ticket in Autobot"),
            )
            .clicked()
        {
            self.open_remote_runs(key.trim().to_owned(), ui.ctx());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> serde_json::Value {
        json!({"enabled":true,"sources":[{"name":"acme","ticket_pattern":"ACME-[0-9]+","repos":["acme/api"],"output_step":"Agent"}]})
    }

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        )
    }

    fn texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn closing_panel_and_output_cancels_their_metadata_requests() {
        let panel = TicketPanel::new(String::new(), &config());
        let cancelled = panel.cancelled.clone();
        drop(panel);
        assert!(cancelled.load(Ordering::Acquire));
        let output = Output {
            cancelled: Arc::default(),
            repo: "acme/api".into(),
            run: Run::default(),
            jobs: vec![],
            receiver: None,
            job: None,
            step: None,
            source: Config::parse(&config()).unwrap().sources.remove(0),
            error: String::new(),
        };
        let cancelled = output.cancelled.clone();
        drop(output);
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn metadata_requests_share_a_global_limit_and_release_slots_on_error() {
        struct Observed {
            active: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
        }
        impl provider::Gh for Observed {
            fn run(&self, _args: &[String]) -> Result<String, String> {
                let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
                self.peak.fetch_max(active, Ordering::AcqRel);
                std::thread::sleep(Duration::from_millis(40));
                self.active.fetch_sub(1, Ordering::AcqRel);
                Err("Remote failure".into())
            }
        }
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let gh = RequestGh {
                    inner: Observed {
                        active: active.clone(),
                        peak: peak.clone(),
                    },
                    cancelled: Arc::default(),
                };
                std::thread::spawn(move || {
                    assert!(provider::Gh::run(&gh, &[]).is_err());
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert!(
            peak.load(Ordering::Acquire) <= 8,
            "global request limit exceeded"
        );
        assert!(
            peak.load(Ordering::Acquire) > 1,
            "independent requests ran serially"
        );
    }

    #[test]
    fn cancelling_a_lookup_stops_requests_between_events_and_repositories() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        struct CancelAfterFirst {
            cancelled: Arc<AtomicBool>,
            calls: Arc<AtomicUsize>,
        }
        impl provider::Gh for CancelAfterFirst {
            fn run(&self, _args: &[String]) -> Result<String, String> {
                self.calls.fetch_add(1, Ordering::AcqRel);
                self.cancelled.store(true, Ordering::Release);
                Ok("[]".into())
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let gh = RequestGh {
            inner: CancelAfterFirst {
                cancelled: cancelled.clone(),
                calls: calls.clone(),
            },
            cancelled,
        };
        let mut value = config();
        value["sources"][0]["events"] = json!(["repository_dispatch", "workflow_dispatch"]);
        let task = Config::parse(&value)
            .unwrap()
            .tasks("ACME-42")
            .unwrap()
            .remove(0);
        provider::lookup(&gh, &task, "ACME-42", 200);
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "cancelled lookup continued making remote requests"
        );
    }

    #[test]
    fn commands_hide_unconfigured_feature_and_surface_invalid_config() {
        let ctx = egui::Context::default();
        let mut app = super::super::render_tests::fixture(0);
        app.open_commands(CommandPage::Commands);
        app.ui.filter = "bot".into();
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec2::new(900.0, 700.0), vec![]);
        }
        let output = frame(&mut app, &ctx, Vec2::new(900.0, 700.0), vec![]);
        assert!(!texts(&output).iter().any(|text| text == "Autobot"));
        app.saved.remote_runs = config();
        let output = frame(&mut app, &ctx, Vec2::new(900.0, 700.0), vec![]);
        assert!(texts(&output).iter().any(|text| text == "Autobot"));
        app.saved.remote_runs["sources"][0]["ticket_pattern"] = json!("[");
        app.open_remote_runs(String::new(), &ctx);
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec2::new(900.0, 700.0), vec![]);
        }
        let output = frame(&mut app, &ctx, Vec2::new(900.0, 700.0), vec![]);
        assert!(
            texts(&output)
                .iter()
                .any(|text| text.contains("invalid ticket_pattern"))
        );
        assert!(app.saved.workspaces.is_empty());
    }

    #[test]
    fn example_preferences_compile_with_the_real_config_parser() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../../docs/examples/remote-runs.json")).unwrap();
        let config = Config::parse(&value["remote_runs"]).unwrap();
        assert_eq!(config.tasks("ACME-42").unwrap().len(), 2);
    }

    #[test]
    fn result_arrival_order_never_changes_newest_first_or_duplicates_runs() {
        let mut panel = TicketPanel::new(String::new(), &config());
        let older: Run = serde_json::from_value(
            json!({"databaseId":1,"displayTitle":"ACME-42","createdAt":"2026-10-28T00:00:00Z"}),
        )
        .unwrap();
        let newer: Run = serde_json::from_value(
            json!({"databaseId":2,"displayTitle":"ACME-42","createdAt":"2026-10-29T00:00:00Z"}),
        )
        .unwrap();
        let (tx, rx) = channel();
        panel.receiver = Some(rx);
        panel.pending = 2;
        tx.send(RepoResult {
            repo: "acme/api".into(),
            source_name: "acme".into(),
            runs: vec![older.clone()],
            prs: vec![],
            errors: vec!["One event could not be queried".into()],
            warnings: vec![],
        })
        .unwrap();
        tx.send(RepoResult {
            repo: "acme/api".into(),
            source_name: "second".into(),
            runs: vec![newer, older],
            prs: vec![],
            errors: vec![],
            warnings: vec![],
        })
        .unwrap();
        panel.poll(&egui::Context::default());
        assert_eq!(panel.pending, 0);
        assert_eq!(
            panel
                .runs()
                .iter()
                .map(|(_, _, run)| run.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(panel.results[0].errors.len(), 1);
    }

    #[test]
    fn step_only_configuration_selects_its_job() {
        let mut panel = TicketPanel::new(String::new(), &config());
        let run = serde_json::from_value(json!({"databaseId":1})).unwrap();
        let jobs = serde_json::from_value(
            json!([{"databaseId":9,"name":"Build","steps":[{"number":1,"name":"Agent"}]}]),
        )
        .unwrap();
        let (tx, rx) = channel();
        tx.send(Ok(jobs)).unwrap();
        panel.output = Some(Output {
            cancelled: Arc::default(),
            repo: "acme/api".into(),
            run,
            jobs: vec![],
            receiver: Some(rx),
            job: None,
            step: None,
            source: Config::parse(&config()).unwrap().sources.remove(0),
            error: String::new(),
        });
        panel.poll(&egui::Context::default());
        let output = panel.output.as_ref().unwrap();
        assert_eq!(output.job, Some(9));
        assert_eq!(output.step.as_deref(), Some("Agent"));
    }

    #[test]
    fn editor_shortcuts_cannot_post_and_preview_shows_target_body_and_warnings() {
        for (size, light) in [
            (Vec2::new(1180.0, 760.0), false),
            (Vec2::new(640.0, 400.0), true),
        ] {
            let ctx = egui::Context::default();
            configure_appearance(&ctx, light);
            let mut app = super::super::render_tests::fixture(0);
            app.saved.remote_runs = config();
            let mut panel = TicketPanel::new(String::new(), &app.saved.remote_runs);
            let pr = PullRequest {
                number: 7,
                title: "ACME-42 Agent changes".into(),
                url: "https://github.com/acme/api/pull/7".into(),
                head_ref_name: "ACME-42".into(),
                state: "CLOSED".into(),
                auto_merge_enabled: true,
            };
            panel.comment = Some(Comment {
                repo: "acme/api".into(),
                pr,
                body: "Please retry the failing checks.".into(),
                preview: None,
                posting: None,
                error: String::new(),
            });
            let mut dashboard = Panel::new(&app.saved.remote_runs);
            panel.key = "ACME-42".into();
            panel.query = "ACME-42".into();
            dashboard.tickets.insert("ACME-42".into(), panel);
            dashboard.selected = Some(crate::autobot::GroupId {
                source: "acme".into(),
                ticket: "ACME-42".into(),
                repo: "acme/api".into(),
            });
            dashboard.detail_visible = true;
            dashboard.initialized = true;
            app.saved.autobot.keys = vec!["ACME-42".into()];
            app.ui.autobot_visible = true;
            app.ui.remote = Some(dashboard);
            for _ in 0..3 {
                frame(&mut app, &ctx, size, vec![]);
            }
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("remote-comment-editor")));
            for modifiers in [Modifiers::NONE, Modifiers::CTRL, Modifiers::MAC_CMD] {
                let output = frame(
                    &mut app,
                    &ctx,
                    size,
                    vec![egui::Event::Key {
                        key: Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                );
                let comment = app.ui.remote.as_ref().unwrap().tickets["ACME-42"]
                    .comment
                    .as_ref()
                    .unwrap();
                assert!(comment.preview.is_none());
                assert!(comment.posting.is_none());
                assert!(
                    !texts(&output)
                        .iter()
                        .any(|text| text == "Confirm and post comment")
                );
            }
            let comment = app
                .ui
                .remote
                .as_mut()
                .unwrap()
                .tickets
                .get_mut("ACME-42")
                .unwrap()
                .comment
                .as_mut()
                .unwrap();
            comment.preview = Some(
                provider::CommentPreview::new(
                    &comment.repo,
                    comment.pr.number,
                    comment.body.clone(),
                )
                .unwrap(),
            );
            let output = frame(&mut app, &ctx, size, vec![]);
            let text = texts(&output).join("\n");
            assert!(text.contains("acme/api#7"));
            assert!(text.contains("Please retry the failing checks."));
            assert!(text.contains("auto-merge is armed"));
            assert!(text.contains("CLOSED"));
            assert!(
                !serde_json::to_string(&app.saved)
                    .unwrap()
                    .contains("Please retry")
            );
        }
    }
    fn dashboard_fixture(light: bool, status: &str, stale: bool) -> (App, egui::Context) {
        let ctx = egui::Context::default();
        configure_appearance(&ctx, light);
        let mut app = super::super::render_tests::fixture(0);
        app.saved.remote_runs = config();
        app.saved.autobot.keys = vec!["ACME-42".into()];
        app.ui.autobot_visible = true;
        let mut dashboard = Panel::new(&app.saved.remote_runs);
        dashboard.sync(&mut app.saved.autobot, &app.saved.remote_runs);
        dashboard.initialized = true;
        let panel = dashboard.tickets.get_mut("ACME-42").unwrap();
        let run = Run {
            id: 42,
            title: "ACME-42 Implementation".into(),
            status: status.into(),
            conclusion: (status == "completed").then(|| "failure".into()),
            workflow_name: "Implementation".into(),
            created_at: "2026-10-29T12:00:00Z".into(),
            started_at: "2026-10-29T12:00:00Z".into(),
            updated_at: "2026-10-29T12:01:30Z".into(),
            ..Default::default()
        };
        let result = RepoResult {
            source_name: "acme".into(),
            repo: "acme/api".into(),
            runs: vec![run.clone()],
            ..Default::default()
        };
        let mut cached = crate::autobot::CachedRepo::new(result.clone(), now() - 100);
        if stale {
            cached.update(
                RepoResult {
                    source_name: "acme".into(),
                    repo: "acme/api".into(),
                    errors: vec!["Network unavailable; refresh to retry".into()],
                    ..Default::default()
                },
                now(),
            );
        }
        panel.results = vec![cached.result.clone()];
        panel.cached = vec![cached];
        panel.output = Some(Output { cancelled: Arc::default(), repo: "acme/api".into(), run, jobs: serde_json::from_value(json!([{"databaseId":9,"name":"Build","status":"in_progress","steps":[{"number":1,"name":"Setup","status":"completed"},{"number":2,"name":"Agent","status":"in_progress"}]}])).unwrap(), receiver: None, job:Some(9), step:Some("Agent".into()), source: Config::parse(&config()).unwrap().sources.remove(0), error:String::new() });
        panel.selected_run = Some(42);
        app.ui.remote = Some(dashboard);
        (app, ctx)
    }

    #[test]
    fn dashboard_overview_detail_stale_and_progress_render_in_both_themes_and_widths() {
        for light in [false, true] {
            for width in [320.0, 640.0, 1180.0] {
                for detail in [false, true] {
                    let (mut app, ctx) = dashboard_fixture(light, "in_progress", true);
                    app.ui.remote.as_mut().unwrap().detail_visible = detail;
                    let mut output = None;
                    for _ in 0..3 {
                        output = Some(ctx.run(
                            egui::RawInput {
                                screen_rect: Some(Rect::from_min_size(
                                    Pos2::ZERO,
                                    Vec2::new(width, 760.0),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default()
                                    .show(ctx, |ui| app.autobot_page(ui, ctx));
                            },
                        ));
                    }
                    let text = texts(output.as_ref().unwrap()).join("\n");
                    assert!(text.contains("Autobot"));
                    assert!(text.contains("ACME-42"));
                    if width >= 760.0 || !detail {
                        assert!(text.contains("Stale data"));
                        assert!(text.contains("Network unavailable"));
                    }
                    if width >= 760.0 || detail {
                        assert!(text.contains("1/2 known steps completed"));
                        assert!(text.contains("Current step: 2. Agent"));
                    }
                    assert!(
                        ctx.used_rect().right() <= width + 1.0,
                        "overflow at {width}: {:?}",
                        ctx.used_rect()
                    );
                }
            }
        }
    }

    #[test]
    fn tracking_groups_match_all_sources_once_and_preserve_selection_on_arrival() {
        let (mut app, ctx) = dashboard_fixture(false, "completed", false);
        let mut value = config();
        let mut other = value["sources"][0].clone();
        other["name"] = json!("second");
        value["sources"].as_array_mut().unwrap().push(other);
        value["sources"][0]["repos"] = json!(["acme/api", "acme/api"]);
        app.saved.remote_runs = value;
        let mut dashboard = app.ui.remote.take().unwrap();
        dashboard.sync(&mut app.saved.autobot, &app.saved.remote_runs);
        assert_eq!(dashboard.groups().len(), 2);
        let selected = dashboard.selected.clone();
        let panel = dashboard.tickets.get_mut("ACME-42").unwrap();
        let (tx, rx) = channel();
        panel.receiver = Some(rx);
        panel.pending = 2;
        let run = Run {
            id: 42,
            status: "completed".into(),
            created_at: "2026-10-29T12:00:00Z".into(),
            ..Default::default()
        };
        for source in ["second", "acme"] {
            tx.send(RepoResult {
                source_name: source.into(),
                repo: "acme/api".into(),
                runs: vec![run.clone()],
                ..Default::default()
            })
            .unwrap();
        }
        panel.poll(&ctx);
        dashboard.sync(&mut app.saved.autobot, &app.saved.remote_runs);
        assert_eq!(dashboard.selected, selected);
        let groups = dashboard.groups();
        assert_eq!(
            groups
                .iter()
                .map(|(_, cached)| cached.as_ref().unwrap().result.runs.len())
                .sum::<usize>(),
            1
        );
        assert_eq!(
            groups
                .iter()
                .find(|(id, _)| id.source == "second")
                .unwrap()
                .1
                .as_ref()
                .unwrap()
                .shared_sources,
            vec!["acme"]
        );
    }

    #[test]
    fn cancellation_rejects_obsolete_receiver_and_keeps_metadata_without_live_polling() {
        let (mut app, _) = dashboard_fixture(false, "completed", false);
        let mut dashboard = app.ui.remote.take().unwrap();
        let generation = dashboard.refresh.start().unwrap();
        dashboard.generation = Some(generation);
        let panel = dashboard.tickets.get_mut("ACME-42").unwrap();
        let (tx, rx) = channel();
        panel.receiver = Some(rx);
        panel.pending = 1;
        let token = panel.cancelled.clone();
        dashboard.cancel();
        assert!(token.load(Ordering::Acquire));
        assert!(tx.send(RepoResult::default()).is_err());
        assert_eq!(dashboard.tickets["ACME-42"].results[0].runs[0].id, 42);
        assert!(dashboard.initialized);
        assert!(!dashboard.refresh.due(1000, true, false));
        assert!(!dashboard.refresh.finish(generation, 1000, false));
    }

    #[test]
    fn run_and_output_selection_survive_newer_metadata_and_job_refresh() {
        let (mut app, ctx) = dashboard_fixture(false, "completed", false);
        let dashboard = app.ui.remote.as_mut().unwrap();
        let panel = dashboard.tickets.get_mut("ACME-42").unwrap();
        panel.selected_group = Some(("acme".into(), "acme/api".into()));
        let (tx, rx) = channel();
        panel.receiver = Some(rx);
        panel.pending = 1;
        let mut result = panel.results[0].clone();
        result.runs.push(Run {
            id: 43,
            status: "completed".into(),
            created_at: "2026-10-29T13:00:00Z".into(),
            ..Default::default()
        });
        tx.send(result).unwrap();
        let output = panel.output.as_mut().unwrap();
        output.step = None;
        let (tx, rx) = channel();
        tx.send(Ok(output.jobs.clone())).unwrap();
        output.receiver = Some(rx);
        panel.poll(&ctx);
        assert_eq!(panel.selected_run, Some(42));
        assert_eq!(
            panel
                .runs()
                .iter()
                .map(|(_, _, run)| run.id)
                .collect::<Vec<_>>(),
            vec![43, 42]
        );
        assert_eq!(panel.output.as_ref().unwrap().job, Some(9));
        assert_eq!(panel.output.as_ref().unwrap().step, None);
    }
}
