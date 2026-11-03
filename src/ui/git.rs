use super::*;
use crate::git::{Action, Branch, Repository};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

struct Outcome {
    repository: Result<Repository, String>,
    error: Option<String>,
}

pub(super) struct Panel {
    directory: PathBuf,
    repository: Option<Repository>,
    pending: Option<Receiver<Outcome>>,
    query: String,
    selected: usize,
    focus: bool,
    message: String,
    error: String,
    return_to: Option<CommandPage>,
}

impl Panel {
    fn new(directory: PathBuf, ctx: &egui::Context) -> Self {
        let mut panel = Self {
            directory,
            repository: None,
            pending: None,
            query: String::new(),
            selected: 0,
            focus: true,
            message: String::new(),
            error: String::new(),
            return_to: None,
        };
        panel.start(Action::Refresh, ctx);
        panel
    }

    fn start(&mut self, action: Action, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        self.message = match &action {
            Action::Refresh => "Refreshing branches…".into(),
            Action::Fetch => "Fetching origin…".into(),
            Action::Main => "Switching to the default branch…".into(),
            Action::Pull => "Pulling changes…".into(),
            Action::Switch(branch) => format!("Switching to {}…", branch.name),
        };
        self.error.clear();
        let directory = self.directory.clone();
        let ctx = ctx.clone();
        let (sender, receiver) = channel();
        match std::thread::Builder::new()
            .name("git-controls".into())
            .spawn(move || {
                let outcome = match crate::git::execute(&directory, &action) {
                    Ok(repository) => Outcome {
                        repository: Ok(repository),
                        error: None,
                    },
                    Err(error) => Outcome {
                        repository: crate::git::load(&directory),
                        error: Some(error),
                    },
                };
                let _ = sender.send(outcome);
                ctx.request_repaint();
            }) {
            Ok(_) => self.pending = Some(receiver),
            Err(error) => {
                self.error = format!("Cannot start Git operation: {error}");
                self.message.clear();
            }
        }
    }

    fn poll(&mut self) {
        let Some(receiver) = &self.pending else {
            return;
        };
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Outcome {
                repository: Err(
                    "Git operation stopped unexpectedly. Refresh to inspect the repository.".into(),
                ),
                error: None,
            },
        };
        self.pending = None;
        self.message.clear();
        self.selected = 0;
        self.focus = true;
        match outcome.repository {
            Ok(repository) => {
                self.directory = repository.root.clone();
                self.repository = Some(repository);
                self.error = outcome.error.unwrap_or_default();
                if self.error.is_empty() {
                    self.message = "Git operation completed.".into();
                }
            }
            Err(error) => {
                self.repository = None;
                self.error = outcome.error.unwrap_or(error);
            }
        }
    }

    fn show(&mut self, ctx: &egui::Context) -> bool {
        self.poll();
        let busy = self.pending.is_some();
        let mut close = false;
        let mut action = None;
        let response = egui::Modal::new(egui::Id::new("git-branches")).show(ctx, |ui| {
            ui.set_width(520.0_f32.min((ctx.content_rect().width() - 48.0).max(120.0)));
            let height = (ctx.content_rect().height() - 64.0).clamp(100.0, 580.0);
            ui.set_height(height);
            egui::ScrollArea::vertical().id_salt("git-dialog-body")
                .max_height(height).auto_shrink([false, false])
                .show(ui, |ui| {
            ui.heading("Git branches");
            ui.add(egui::Label::new(self.directory.display().to_string()).wrap());
            if let Some(repo) = &self.repository {
                ui.label(format!("Current branch: {}", repo.current.as_deref().unwrap_or("Detached HEAD")));
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Refresh")).clicked() { action = Some(Action::Refresh); }
                ui.add_enabled_ui(!busy && self.repository.is_some(), |ui| {
                    if ui.button("Fetch origin").on_hover_text("Download branches and commits from origin without changing working files").clicked() { action = Some(Action::Fetch); }
                    let default = self.repository.as_ref().and_then(|r| r.default_branch.as_deref());
                    if ui.add_enabled(default.is_some(), egui::Button::new("gcm"))
                        .on_hover_text(format!("Switch to the default branch ({})", default.unwrap_or("unknown"))).clicked() { action = Some(Action::Main); }
                    if ui.add_enabled(self.repository.as_ref().is_some_and(|r| r.current.is_some()), egui::Button::new("gl"))
                        .on_hover_text("Pull the current branch from its upstream, fast-forward only").clicked() { action = Some(Action::Pull); }
                });
            });
            ui.small("Fetch origin, gcm, gl, then search for the branch you want.");
            ui.separator();
            let input_id = egui::Id::new("git-branch-search");
            let searching = ctx.memory(|m| m.has_focus(input_id));
            let down = !busy && searching && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown));
            let up = !busy && searching && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp));
            let enter = !busy && searching && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            let field = ui.add_enabled(!busy && self.repository.is_some(),
                egui::TextEdit::singleline(&mut self.query).id(input_id)
                    .hint_text("Search local and origin branches").desired_width(f32::INFINITY));
            if self.focus && field.enabled() && !ui.is_sizing_pass() {
                field.request_focus();
                self.focus = false;
            }
            if field.changed() { self.selected = 0; }
            if let Some(repo) = &self.repository {
                let branches = filtered_branches(repo, &self.query);
                let count = branches.len();
                if down { self.selected = (self.selected + 1).min(count.saturating_sub(1)); }
                if up { self.selected = self.selected.saturating_sub(1); }
                self.selected = self.selected.min(count.saturating_sub(1));
                egui::ScrollArea::vertical().id_salt("git-branch-results")
                    .max_height((ctx.content_rect().height() - 340.0).clamp(60.0, 280.0))
                    .auto_shrink([false, false]).show(ui, |ui| {
                        for (index, branch) in branches.iter().enumerate() {
                            let current = !branch.remote && repo.current.as_deref() == Some(&branch.name);
                            let label = format!("{}{}", branch_label(branch), if current { " (current)" } else { "" });
                            let row = ui.add_enabled(!busy && !current,
                                egui::Button::selectable(index == self.selected, label).wrap());
                            if index == self.selected && (up || down) { row.scroll_to_me(Some(egui::Align::Center)); }
                            if row.clicked() || (!busy && !current && enter && index == self.selected) {
                                action = Some(Action::Switch((*branch).clone()));
                            }
                        }
                        if count == 0 { ui.label("No matching branches. Fetch origin to discover remote branches."); }
                    });
            }
            if !self.error.is_empty() {
                egui::ScrollArea::vertical().id_salt("git-error").max_height(100.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&self.error).color(ui.visuals().error_fg_color)).wrap());
                });
            }
            if !self.message.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    if busy { ui.spinner(); }
                    ui.label(&self.message);
                });
            }
            if self.return_to.is_none() {
                ui.small("Working files are shared with the running agent. Switch branches when the agent is idle.");
            }
            close = ui.add_enabled(!busy, egui::Button::new(if self.return_to.is_some() { "Back to launch" } else { "Close" })).clicked();
                });
        });
        if let Some(action) = action {
            self.start(action, ctx);
        }
        self.pending.is_none() && (close || response.should_close())
    }
}

impl App {
    pub(super) fn open_launch_git(&mut self, ctx: &egui::Context, directory: PathBuf) {
        if self.folder_launch.is_some() {
            return;
        }
        if self.ui.command_page == CommandPage::Folders {
            self.ui.folder_focus = Some(directory.clone());
        }
        let mut panel = Panel::new(directory, ctx);
        panel.return_to = Some(self.ui.command_page);
        self.ui.palette = false;
        self.ui.git = Some(panel);
    }

    pub(super) fn open_git(&mut self, ctx: &egui::Context, pane: Uuid) {
        if self.remote_panes.contains(&pane) {
            self.ui.error = "Git controls are available for local terminals. Use a shell on the remote host for this pane.".into();
            return;
        }
        let directory = self
            .panes
            .get(&pane)
            .ok_or_else(|| anyhow::anyhow!("Terminal is unavailable"))
            .and_then(|pane| pane.terminal.current_directory());
        match directory {
            Ok(directory) => {
                self.ui.palette = false;
                self.ui.git = Some(Panel::new(directory, ctx));
            }
            Err(error) => self.ui.error = format!("Cannot open Git controls: {error}"),
        }
    }

    pub(super) fn git_dialog(&mut self, ctx: &egui::Context) {
        if self.ui.git.as_mut().is_some_and(|panel| panel.show(ctx)) {
            let panel = self.ui.git.take().unwrap();
            if let Some(page) = panel.return_to {
                self.ui.palette = true;
                self.ui.command_page = page;
                self.ui.command_focus = false;
                self.ui.create_focus = page == CommandPage::Create;
            }
        }
    }
}

fn branch_label(branch: &Branch) -> String {
    if branch.remote {
        format!("origin/{}", branch.name)
    } else {
        branch.name.clone()
    }
}

fn filtered_branches<'a>(repo: &'a Repository, query: &str) -> Vec<&'a Branch> {
    let query = query.to_lowercase();
    repo.branches
        .iter()
        .filter(|branch| branch_label(branch).to_lowercase().contains(&query))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 760.0))),
                events,
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        )
    }

    fn click_text(app: &mut App, ctx: &egui::Context, label: &str) {
        app_frame(app, ctx, vec![]);
        let output = app_frame(app, ctx, vec![]);
        let pos = output
            .shapes
            .iter()
            .rev()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing button: {label}"));
        for pressed in [true, false] {
            app_frame(
                app,
                ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn folder_git_prepares_branch_then_returns_without_launching() {
        let (_temp, source, work) = crate::git::tests::fixture();
        crate::git::tests::git(&work, &["branch", "feature/prelaunch"]);
        let ctx = egui::Context::default();
        let mut app = super::super::render_tests::fixture(0);
        app.open_commands(CommandPage::Folders);
        app.ui.launch_kind = 2;
        app.ui.folder_query = work.display().to_string();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app_frame(&mut app, &ctx, vec![]);
            if app
                .ui
                .folder_search
                .as_ref()
                .is_some_and(|s| !s.loading && s.results.len() == 1)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        app.ui.folder_search.as_mut().unwrap().results.insert(
            0,
            crate::folders::FolderResult {
                name: "Another folder".into(),
                path: source,
            },
        );
        click_text(&mut app, &ctx, "Git");
        assert!(app.ui.git.is_some());
        assert!(!app.ui.palette);
        ready(app.ui.git.as_mut().unwrap());
        app.ui.git.as_mut().unwrap().start(
            Action::Switch(Branch {
                name: "feature/prelaunch".into(),
                remote: false,
            }),
            &ctx,
        );
        ready(app.ui.git.as_mut().unwrap());
        click_text(&mut app, &ctx, "Back to launch");
        assert!(app.ui.git.is_none() && app.ui.palette);
        assert!(app.ui.command_page == CommandPage::Folders);
        assert_eq!(app.ui.launch_kind, 2);
        assert_eq!(app.ui.folder_query, work.display().to_string());
        assert!(app.panes.is_empty() && app.folder_launch.is_none());
        assert_eq!(
            crate::git::tests::git(&work, &["branch", "--show-current"]),
            "feature/prelaunch"
        );
        for _ in 0..3 {
            app_frame(&mut app, &ctx, vec![]);
        }
        let search = egui::Id::new("folder-filter");
        assert!(
            !ctx.memory(|memory| memory.has_focus(search)),
            "Returning from Git should focus the folder, not search"
        );
        assert!(ctx.memory(|memory| memory.focused().is_some()));
        assert_eq!(
            app.ui.command_index, 1,
            "Focus should follow the prepared folder, not the old selection"
        );
        std::fs::remove_dir_all(&work).unwrap();
        app_frame(&mut app, &ctx, vec![key(Key::Enter)]);
        assert!(
            app.folder_launch.is_some(),
            "Enter should launch the prepared folder"
        );
        assert_eq!(app.folder_launch.as_ref().unwrap().kind, 2);
    }

    #[test]
    fn new_workspace_git_returns_to_unchanged_launch_form() {
        assert_workspace_launch_after_git(false);
    }

    #[test]
    fn failed_git_preparation_still_focuses_create_and_allows_enter_to_launch() {
        assert_workspace_launch_after_git(true);
    }

    fn assert_workspace_launch_after_git(git_fails: bool) {
        let (_temp, _, work) = crate::git::tests::fixture();
        let ctx = egui::Context::default();
        let mut app = super::super::render_tests::fixture(0);
        app.open_commands(CommandPage::Create);
        app.ui.new_directory = work.display().to_string();
        app.ui.new_workspace_name = "My Claude task".into();
        app.ui.launch_kind = 0;
        click_text(&mut app, &ctx, "Git before launch");
        assert!(app.ui.git.is_some());
        ready(app.ui.git.as_mut().unwrap());
        if git_fails {
            let panel = app.ui.git.as_mut().unwrap();
            panel.start(
                Action::Switch(Branch {
                    name: "missing-branch".into(),
                    remote: false,
                }),
                &ctx,
            );
            wait(panel);
            assert!(!panel.error.is_empty(), "The checkout should fail");
            assert_eq!(
                panel.repository.as_ref().unwrap().current.as_deref(),
                Some("main")
            );
        }
        app_frame(&mut app, &ctx, vec![]);
        app_frame(&mut app, &ctx, vec![key(Key::Escape)]);
        assert!(app.ui.git.is_none() && app.ui.palette);
        assert!(app.ui.command_page == CommandPage::Create);
        assert_eq!(app.ui.new_directory, work.display().to_string());
        assert_eq!(app.ui.new_workspace_name, "My Claude task");
        assert_eq!(app.ui.launch_kind, 0);
        assert!(app.panes.is_empty() && app.folder_launch.is_none());
        for _ in 0..3 {
            app_frame(&mut app, &ctx, vec![]);
        }
        let focused = ctx.memory(|memory| memory.focused());
        assert!(focused.is_some());
        assert_ne!(focused, Some(egui::Id::new("workspace-directory")));
        assert_ne!(focused, Some(egui::Id::new("new-workspace-name")));
        app_frame(&mut app, &ctx, vec![key(Key::Enter)]);
        assert_eq!(app.panes.len(), 1, "Enter should activate Create workspace");
        assert_eq!(app.saved.workspaces[0].task.title, "My Claude task");
        assert_eq!(
            app.saved.workspaces[0].task.directory,
            work.display().to_string()
        );
        assert!(!app.ui.palette);
    }

    fn frame(panel: &mut Panel, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 400.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                panel.show(ctx);
            },
        )
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    fn ready(panel: &mut Panel) {
        wait(panel);
        assert!(panel.error.is_empty(), "{}", panel.error);
    }

    fn wait(panel: &mut Panel) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while panel.pending.is_some() {
            panel.poll();
            assert!(Instant::now() < deadline, "Git operation did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn typing_and_enter_switch_the_matching_branch_without_shell_input() {
        let (_temp, _, work) = crate::git::tests::fixture();
        crate::git::tests::git(&work, &["branch", "feature/search"]);
        let ctx = egui::Context::default();
        let mut panel = Panel::new(work.clone(), &ctx);
        ready(&mut panel);
        frame(&mut panel, &ctx, vec![]);
        frame(&mut panel, &ctx, vec![]);
        frame(&mut panel, &ctx, vec![egui::Event::Text("search".into())]);
        assert_eq!(panel.query, "search");
        frame(&mut panel, &ctx, vec![key(Key::Enter)]);
        assert!(panel.pending.is_some(), "Enter should start a checkout");
        ready(&mut panel);
        assert_eq!(
            crate::git::tests::git(&work, &["branch", "--show-current"]),
            "feature/search"
        );
        assert_eq!(
            panel.repository.as_ref().unwrap().current.as_deref(),
            Some("feature/search")
        );
    }

    #[test]
    fn git_dialog_blocks_workspace_shortcuts() {
        let (_temp, _, work) = crate::git::tests::fixture();
        let ctx = egui::Context::default();
        let mut app = super::super::render_tests::fixture(1);
        app.ui.git = Some(Panel::new(work, &ctx));
        ready(app.ui.git.as_mut().unwrap());
        let command = if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL | Modifiers::ALT
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: Key::L,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: command | Modifiers::SHIFT,
                }],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert!(
            !app.ui.palette,
            "Workspace shortcuts must not open another dialog over Git"
        );
    }

    #[test]
    fn search_matches_local_and_origin_branches_case_insensitively() {
        let repo = Repository {
            root: "/repo".into(),
            current: Some("main".into()),
            default_branch: Some("main".into()),
            branches: vec![
                Branch {
                    name: "main".into(),
                    remote: false,
                },
                Branch {
                    name: "feature/Search".into(),
                    remote: false,
                },
                Branch {
                    name: "feature/search-remote".into(),
                    remote: true,
                },
            ],
        };
        let matches = filtered_branches(&repo, "SEARCH");
        assert_eq!(
            matches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["feature/Search", "feature/search-remote"]
        );
        assert_eq!(filtered_branches(&repo, "origin/feature").len(), 1);
        assert!(filtered_branches(&repo, "missing").is_empty());
    }
}
