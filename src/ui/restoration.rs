use super::*;
use crate::model::Agent;
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct SavedRestoration {
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    pub(super) active: Option<Uuid>,
    #[serde(default)]
    pub panes: HashMap<Uuid, SavedPane>,
    #[serde(default)]
    pub(super) windows: Vec<windows::SavedWindow>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct SavedPane {
    pub(super) directory: String,
    output: String,
    pub(super) conversation: Option<(Agent, String)>,
    #[serde(default)]
    resume_directory: Option<String>,
}
#[derive(Default)]
pub(super) struct Restoration {
    pub pending: HashMap<Uuid, Receiver<anyhow::Result<String>>>,
    pub resumed: HashSet<Uuid>,
    pub notices: HashMap<Uuid, String>,
    pub history: HashMap<Uuid, String>,
}
fn bounded(text: &str) -> String {
    let lines: Vec<_> = text
        .lines()
        .rev()
        .take(crate::terminal::SNAPSHOT_MAX_LINES)
        .collect();
    let text = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
    let mut start = text
        .len()
        .saturating_sub(crate::terminal::SNAPSHOT_MAX_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..]
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}
impl App {
    fn pane_conversation(&self, id: Uuid) -> Option<(Agent, String)> {
        self.saved
            .sessions
            .iter()
            .filter(|s| s.pane == id)
            .max_by_key(|s| (s.updated, s.last_sequence))
            .filter(|s| s.state != SessionState::Ended)
            .map(|s| (s.agent, s.session_id.clone()))
    }
    pub(super) fn capture_restoration(&mut self) {
        self.capture_restoration_with(false);
    }
    pub(super) fn capture_restoration_with(&mut self, wait: bool) {
        self.capture_windows();
        let mut ids = Vec::new();
        for workspace in &self.saved.workspaces {
            workspace.layout.ids(&mut ids);
        }
        self.saved
            .restoration
            .panes
            .retain(|id, _| ids.contains(id));
        for id in ids {
            let conversation = self.pane_conversation(id);
            let saved = self.saved.restoration.panes.entry(id).or_default();
            // A failed resume remains retryable even after discovery marks it disconnected.
            if !self.restoration.notices.contains_key(&id)
                && !self.restoration.pending.contains_key(&id)
                && !self.restoration.resumed.contains(&id)
            {
                saved.conversation = conversation;
            }
            if !self.panes.contains_key(&id)
                && let Some(history) = self.restoration.history.get(&id)
            {
                saved.output = bounded(history);
            }
            if let Some(pane) = self.panes.get(&id) {
                if !self.restoration.pending.contains_key(&id)
                    && !pane.terminal.has_exited()
                    && let Ok(directory) = pane.terminal.current_directory()
                {
                    if saved.conversation.is_some()
                        && saved.resume_directory.is_none()
                        && !saved.directory.is_empty()
                    {
                        saved.resume_directory = Some(saved.directory.clone());
                    }
                    saved.directory = directory.to_string_lossy().into_owned();
                    if saved.conversation.is_some() && !self.restoration.notices.contains_key(&id) {
                        saved.resume_directory = Some(saved.directory.clone());
                    }
                }
                let output = if wait {
                    Some(pane.terminal.snapshot_output())
                } else {
                    pane.terminal.try_snapshot_output()
                };
                let Some(live) = output else {
                    continue;
                };
                saved.output = bounded(&format!(
                    "{}\n{}",
                    self.restoration
                        .history
                        .get(&id)
                        .map(String::as_str)
                        .unwrap_or(""),
                    live
                ));
            }
        }
        self.saved.restoration.initialized = true;
        if self.windows.is_empty()
            && let Some(workspace) = self.saved.workspaces.get(self.ui.active)
        {
            self.saved.restoration.active = Some(workspace.task.id);
        }
    }
    pub(super) fn restore_workspace(&mut self, ctx: &egui::Context) {
        self.sync_work();
        self.ui.overview = false;
        self.ui.specifications_view = false;
        let workspaces = self.saved.workspaces.clone();
        for workspace in workspaces {
            let mut ids = Vec::new();
            workspace.layout.ids(&mut ids);
            for id in ids {
                let conversation = self.pane_conversation(id);
                let saved = self
                    .saved
                    .restoration
                    .panes
                    .entry(id)
                    .or_insert_with(|| SavedPane {
                        directory: workspace.task.directory.clone(),
                        conversation,
                        ..Default::default()
                    })
                    .clone();
                if saved.conversation.is_some() && saved.resume_directory.is_none() {
                    self.saved
                        .restoration
                        .panes
                        .get_mut(&id)
                        .unwrap()
                        .resume_directory = Some(saved.directory.clone());
                }
                self.restoration.history.insert(id, bounded(&saved.output));
                if saved.conversation.is_some() {
                    self.retry_resume(id, ctx);
                } else {
                    self.fresh_restored_shell(id, ctx, &saved.directory);
                }
            }
        }
        self.ui.active = self
            .saved
            .workspaces
            .iter()
            .position(|w| Some(w.task.id) == self.saved.restoration.active)
            .unwrap_or(0);
        if self.saved.workspaces.is_empty() && !self.saved.restoration.initialized {
            self.add_workspace_in(ctx, self.ui.new_directory.clone());
        }
        for session in &mut self.saved.sessions {
            if session.state != SessionState::Ended {
                session.state = SessionState::Disconnected;
            }
        }
        self.restore_windows(ctx);
        self.saved.restoration.initialized = true;
        self.dirty = true;
    }
    fn fresh_restored_shell(&mut self, id: Uuid, ctx: &egui::Context, directory: &str) {
        let directory = crate::directories::resolve(directory);
        match directory {
            Ok(directory) if directory.is_dir() => {
                self.spawn(id, ctx, &directory.to_string_lossy());
            }
            _ => {
                self.restoration.notices.insert(id, "Saved directory is unavailable. A fresh shell was opened in your home directory; saved output and conversation identity are retained.".into());
                let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
                self.spawn(id, ctx, &home);
            }
        }
    }
    fn retry_resume(&mut self, id: Uuid, ctx: &egui::Context) {
        if self.restoration.pending.contains_key(&id) {
            return;
        }
        let Some(saved) = self.saved.restoration.panes.get(&id).cloned() else {
            return;
        };
        let directory = saved
            .resume_directory
            .clone()
            .unwrap_or_else(|| saved.directory.clone());
        let Some((agent, session)) = saved.conversation else {
            return;
        };
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        if let Some(pane) = self.panes.remove(&id) {
            let output = bounded(&format!(
                "{}\n{}",
                self.restoration
                    .history
                    .get(&id)
                    .map(String::as_str)
                    .unwrap_or(""),
                pane.terminal.snapshot_output()
            ));
            self.restoration.history.insert(id, output);
        }
        self.restoration.pending.insert(id, rx);
        self.restoration.notices.remove(&id);
        std::thread::spawn(move || {
            let result = (|| {
                let launch = crate::resume::launch(agent, &session)?;
                let directory = crate::directories::resolve(&directory)?;
                anyhow::ensure!(directory.is_dir(), "Saved directory is unavailable");
                Ok(launch)
            })();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_restoration(&mut self, ctx: &egui::Context) {
        let ready: Vec<_> = self
            .restoration
            .pending
            .iter()
            .filter_map(|(&id, rx)| match rx.try_recv() {
                Ok(result) => Some((id, result)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((
                    id,
                    Err(anyhow::anyhow!("Resume check stopped unexpectedly")),
                )),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            })
            .collect();
        for (id, result) in ready {
            self.restoration.pending.remove(&id);
            let directory = self.saved.restoration.panes[&id].directory.clone();
            let resume_directory = self.saved.restoration.panes[&id]
                .resume_directory
                .as_deref()
                .unwrap_or(&directory);
            let result = result.and_then(|launch| {
                let directory = crate::directories::resolve(resume_directory)?;
                let socket = self
                    .endpoint
                    .as_ref()
                    .map(|e| e.path.as_path())
                    .unwrap_or(std::path::Path::new(""));
                Terminal::spawn_agent(id, &directory, socket, ctx.clone(), &launch)
            });
            match result {
                Ok(terminal) => {
                    self.panes.insert(
                        id,
                        Pane {
                            terminal,
                            search: TerminalSearch::default(),
                        },
                    );
                    self.restoration.resumed.insert(id);
                }
                Err(error) => {
                    self.fresh_restored_shell(id, ctx, &directory);
                    self.restoration.notices.insert(id, format!("Conversation could not be resumed: {error}. Fresh shell ready. Saved identity and output are kept."));
                }
            }
        }
        let exited: Vec<_> = self
            .restoration
            .resumed
            .iter()
            .copied()
            .filter(|id| self.panes.get(id).is_some_and(|p| p.terminal.has_exited()))
            .collect();
        for id in exited {
            self.restoration.resumed.remove(&id);
            let pane = &self.panes[&id];
            let code = pane.terminal.exit_code();
            let output = bounded(&format!(
                "{}\n[Resume process output]\n{}",
                self.restoration
                    .history
                    .get(&id)
                    .map(String::as_str)
                    .unwrap_or(""),
                pane.terminal.snapshot_output()
            ));
            self.restoration.history.insert(id, output);
            let directory = self.saved.restoration.panes[&id].directory.clone();
            self.fresh_restored_shell(id, ctx, &directory);
            self.restoration.notices.insert(id, format!("Resumed conversation exited (status {code:?}). Fresh shell ready. Saved conversation can be retried."));
        }
    }
    pub(super) fn restoration_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: Uuid) {
        if let Some(output) = self.restoration.history.get(&id).filter(|s| !s.is_empty()) {
            egui::CollapsingHeader::new("Saved output (previous run, read only)")
                .id_salt(id)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(output).monospace())
                                    .selectable(true),
                            );
                        });
                });
        }
        if self.restoration.pending.contains_key(&id) {
            ui.label("Checking saved conversation and resuming...");
        }
        if let Some(notice) = self.restoration.notices.get(&id).cloned() {
            ui.label(notice);
            if self
                .saved
                .restoration
                .panes
                .get(&id)
                .is_some_and(|p| p.conversation.is_some())
                && ui.button("Retry saved conversation").clicked()
            {
                self.retry_resume(id, ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render_tests::fixture;
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !predicate() {
            assert!(Instant::now() < deadline, "terminal operation timed out");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn round_trip(app: &mut App, path: &std::path::Path) -> Saved {
        app.capture_restoration();
        crate::snapshot::save(path, &app.saved).unwrap();
        crate::snapshot::load(path).unwrap().unwrap()
    }

    #[test]
    fn real_shell_close_reopen_preserves_layout_focus_directory_and_history() {
        let ctx = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let child = directory.join("child");
        std::fs::create_dir(&child).unwrap();
        let mut app = fixture(0);
        assert!(app.add_workspace_in(&ctx, directory.to_string_lossy().into_owned()));
        let first = app.saved.workspaces[0].focus;
        app.split(&ctx, true);
        let second = app.saved.workspaces[0].focus;
        app.panes[&second]
            .terminal
            .input(
                format!("cd '{}'\nprintf 'RESTORED_%s\\n' OUTPUT\n", child.display()).into_bytes(),
            )
            .unwrap();
        wait_for(|| {
            app.panes[&second]
                .terminal
                .current_directory()
                .ok()
                .as_ref()
                == Some(&child)
                && app.panes[&second]
                    .terminal
                    .snapshot_output()
                    .contains("RESTORED_OUTPUT")
        });
        assert!(app.add_workspace_in(&ctx, directory.to_string_lossy().into_owned()));
        app.ui.active = 0;
        let detached_workspace = app.saved.workspaces[1].task.id;
        app.detach_workspace(&ctx, detached_workspace, None);
        let layout = serde_json::to_value(&app.saved.workspaces).unwrap();
        let alive = app.panes[&second].terminal.alive.clone();
        let saved = round_trip(&mut app, &directory.join("state.json"));
        drop(app);
        assert!(!alive.load(std::sync::atomic::Ordering::Acquire));
        let mut reopened = fixture(0);
        reopened.saved = saved;
        reopened.restore_workspace(&ctx);
        assert_eq!(
            serde_json::to_value(&reopened.saved.workspaces).unwrap(),
            layout
        );
        assert_eq!(reopened.ui.active, 0);
        assert_eq!(reopened.saved.workspaces[0].focus, second);
        assert_eq!(reopened.panes.len(), 3);
        assert_eq!(reopened.windows.len(), 2);
        assert_ne!(
            reopened.workspace_window(detached_workspace),
            egui::ViewportId::ROOT
        );
        assert!(reopened.panes.contains_key(&first));
        wait_for(|| {
            reopened.panes[&second]
                .terminal
                .current_directory()
                .ok()
                .as_ref()
                == Some(&child)
        });
        assert!(reopened.restoration.history[&second].contains("RESTORED_OUTPUT"));
        assert!(
            !reopened.panes[&second]
                .terminal
                .snapshot_output()
                .contains("RESTORED_OUTPUT")
        );
        assert!(reopened.restoration.pending.is_empty());
    }

    #[test]
    fn explicitly_closed_panes_and_final_workspace_stay_closed() {
        let ctx = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = fixture(0);
        assert!(app.add_workspace_in(&ctx, root.path().to_string_lossy().into_owned()));
        let survivor = app.saved.workspaces[0].focus;
        app.split(&ctx, false);
        let closed = app.saved.workspaces[0].focus;
        app.capture_restoration();
        app.stop_terminal(closed);
        let saved = round_trip(&mut app, &root.path().join("state.json"));
        drop(app);
        let mut reopened = fixture(0);
        reopened.saved = saved;
        reopened.restore_workspace(&ctx);
        assert_eq!(reopened.panes.len(), 1);
        assert!(reopened.panes.contains_key(&survivor));
        assert!(!reopened.saved.restoration.panes.contains_key(&closed));
        reopened.stop_terminal(survivor);
        let empty = round_trip(&mut reopened, &root.path().join("state.json"));
        let mut final_reopen = fixture(0);
        final_reopen.saved = empty;
        final_reopen.restore_workspace(&ctx);
        assert!(final_reopen.panes.is_empty());
        assert!(final_reopen.saved.workspaces.is_empty());
    }

    #[test]
    fn identities_are_per_pane_and_failed_resume_preserves_exact_identity() {
        let mut app = fixture(2);
        app.capture_restoration();
        let first = app.saved.workspaces[0].focus;
        let second = app.saved.workspaces[1].focus;
        assert_eq!(
            app.saved.restoration.panes[&first].conversation,
            Some((Agent::Claude, "fixture-0".into()))
        );
        assert_eq!(
            app.saved.restoration.panes[&second].conversation,
            Some((Agent::Codex, "fixture-1".into()))
        );
        let (sender, receiver) = channel();
        app.restoration.pending.insert(first, receiver);
        sender
            .send(Err(anyhow::anyhow!("saved session unavailable")))
            .unwrap();
        app.poll_restoration(&egui::Context::default());
        assert!(app.restoration.notices[&first].contains("saved session unavailable"));
        app.saved.sessions[0].state = SessionState::Ended;
        app.capture_restoration();
        assert_eq!(
            app.saved.restoration.panes[&first].conversation,
            Some((Agent::Claude, "fixture-0".into()))
        );
        assert_eq!(
            app.saved.restoration.panes[&second].conversation,
            Some((Agent::Codex, "fixture-1".into()))
        );
    }

    #[test]
    fn legacy_metadata_migrates_without_losing_workspace_or_session_identity() {
        let app = fixture(2);
        let mut legacy = serde_json::to_value(&app.saved).unwrap();
        legacy.as_object_mut().unwrap().remove("restoration");
        let mut migrated = fixture(0);
        migrated.saved = serde_json::from_value(legacy).unwrap();
        assert!(!migrated.saved.restoration.initialized);
        migrated.capture_restoration();
        assert_eq!(migrated.saved.workspaces.len(), 2);
        for session in &migrated.saved.sessions {
            assert_eq!(
                migrated.saved.restoration.panes[&session.pane].conversation,
                Some((session.agent, session.session_id.clone()))
            );
        }
    }

    #[test]
    fn missing_directory_opens_fresh_shell_and_retains_saved_output() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.saved.sessions.clear();
        let id = app.saved.workspaces[0].focus;
        let root = tempfile::tempdir().unwrap();
        app.saved.restoration.panes.insert(
            id,
            SavedPane {
                directory: root.path().join("removed").to_string_lossy().into_owned(),
                output: "previous conversation".into(),
                conversation: None,
                resume_directory: None,
            },
        );
        app.restore_workspace(&ctx);
        assert!(app.panes.contains_key(&id));
        assert!(app.restoration.notices[&id].contains("Saved directory is unavailable"));
        assert_eq!(app.restoration.history[&id], "previous conversation");
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .canonicalize()
            .unwrap();
        wait_for(|| app.panes[&id].terminal.current_directory().ok().as_ref() == Some(&home));
    }

    #[test]
    fn discovery_placeholder_cannot_replace_resumed_identity() {
        let mut app = fixture(1);
        app.capture_restoration();
        let id = app.saved.workspaces[0].focus;
        let expected = app.saved.restoration.panes[&id].conversation.clone();
        app.restoration.resumed.insert(id);
        let mut placeholder = Session::new(id, "process-123-456".into());
        placeholder.observed_process = Some(123);
        placeholder.updated = now() + 5;
        app.saved.sessions.push(placeholder);
        app.capture_restoration();
        assert_eq!(app.saved.restoration.panes[&id].conversation, expected);
    }

    #[test]
    fn exited_resume_keeps_output_identity_and_opens_shell_without_exit_race() {
        let ctx = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = fixture(1);
        let id = app.saved.workspaces[0].focus;
        app.saved.restoration.panes.insert(
            id,
            SavedPane {
                directory: root.path().to_string_lossy().into_owned(),
                conversation: Some((Agent::Claude, Uuid::new_v4().to_string())),
                ..Default::default()
            },
        );
        let expected = app.saved.restoration.panes[&id].conversation.clone();
        let (tx, rx) = channel();
        app.restoration.pending.insert(id, rx);
        tx.send(Ok("/bin/sh -c 'printf RESUME_FAILURE; exit 9'".into()))
            .unwrap();
        app.poll_restoration(&ctx);
        wait_for(|| app.panes[&id].terminal.has_exited());
        app.drain();
        assert!(
            app.panes.contains_key(&id),
            "normal cleanup deleted resumed pane"
        );
        app.poll_restoration(&ctx);
        assert!(app.restoration.notices[&id].contains("9"));
        assert!(app.restoration.history[&id].contains("RESUME_FAILURE"));
        assert!(!app.panes[&id].terminal.has_exited());
        app.capture_restoration();
        assert_eq!(app.saved.restoration.panes[&id].conversation, expected);
    }

    #[test]
    fn failed_worker_is_retryable_and_closed_pending_pane_never_launches() {
        let ctx = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = fixture(1);
        let id = app.saved.workspaces[0].focus;
        app.saved.restoration.panes.insert(
            id,
            SavedPane {
                directory: root.path().to_string_lossy().into_owned(),
                conversation: Some((Agent::Codex, Uuid::new_v4().to_string())),
                ..Default::default()
            },
        );
        let (tx, rx) = channel();
        app.restoration.pending.insert(id, rx);
        drop(tx);
        app.poll_restoration(&ctx);
        assert!(app.restoration.pending.is_empty());
        assert!(app.restoration.notices[&id].contains("unexpectedly"));
        assert!(app.panes.contains_key(&id));
        let (tx, rx) = channel();
        app.restoration.pending.insert(id, rx);
        app.stop_terminal(id);
        assert!(tx.send(Ok("must never execute".into())).is_err());
        app.poll_restoration(&ctx);
        assert!(app.panes.is_empty());
        assert!(app.saved.restoration.panes.is_empty());
    }

    #[test]
    fn separate_windows_keep_tab_membership_order_and_active_tabs_after_serialization() {
        let ctx = egui::Context::default();
        let mut app = fixture(4);
        let tabs: Vec<_> = app.saved.workspaces.iter().map(|w| w.task.id).collect();
        app.detach_workspace(&ctx, tabs[1], None);
        let detached = app.workspace_window(tabs[1]);
        app.workspace_windows.insert(tabs[2], detached);
        app.windows.get_mut(&egui::ViewportId::ROOT).unwrap().active = Some(tabs[3]);
        app.windows.get_mut(&detached).unwrap().active = Some(tabs[2]);
        // Outside a draw, ui belongs to the host, not to either visible window.
        app.ui.active = 0;
        app.capture_restoration();
        let mut reopened = fixture(0);
        reopened.saved = serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
        reopened.restore_windows(&ctx);
        assert_eq!(reopened.windows.len(), 2);
        let restored_detached = reopened.workspace_window(tabs[1]);
        assert_ne!(restored_detached, egui::ViewportId::ROOT);
        assert_eq!(reopened.workspace_window(tabs[2]), restored_detached);
        assert_eq!(reopened.workspace_window(tabs[0]), egui::ViewportId::ROOT);
        assert_eq!(reopened.workspace_window(tabs[3]), egui::ViewportId::ROOT);
        assert_eq!(reopened.windows[&restored_detached].active, Some(tabs[2]));
        assert_eq!(
            reopened.windows[&egui::ViewportId::ROOT].active,
            Some(tabs[3])
        );
        reopened.current_window = restored_detached;
        assert_eq!(reopened.window_workspaces(), vec![1, 2]);
        reopened.current_window = egui::ViewportId::ROOT;
        assert_eq!(reopened.window_workspaces(), vec![0, 3]);
    }

    #[test]
    fn periodic_capture_uses_current_draw_active_tab_without_changing_other_windows() {
        let ctx = egui::Context::default();
        let mut app = fixture(4);
        let tabs: Vec<_> = app.saved.workspaces.iter().map(|w| w.task.id).collect();
        app.detach_workspace(&ctx, tabs[1], None);
        let detached = app.workspace_window(tabs[1]);
        app.workspace_windows.insert(tabs[2], detached);
        app.windows.get_mut(&egui::ViewportId::ROOT).unwrap().active = Some(tabs[3]);
        app.drawing_window = Some(detached);
        app.current_window = detached;
        app.ui.active = 2;
        app.capture_restoration();
        let windows = &app.saved.restoration.windows;
        assert_eq!(
            windows.iter().find(|w| w.root).unwrap().active,
            Some(tabs[3])
        );
        assert_eq!(
            windows.iter().find(|w| !w.root).unwrap().active,
            Some(tabs[2])
        );
    }

    #[test]
    fn detached_only_restore_does_not_create_an_extra_visible_root() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        let tab = app.saved.workspaces[0].task.id;
        app.detach_workspace(&ctx, tab, None);
        assert!(!app.windows.contains_key(&egui::ViewportId::ROOT));
        app.capture_restoration();
        let mut reopened = fixture(0);
        reopened.saved = app.saved.clone();
        let output = ctx.run(Default::default(), |ctx| reopened.restore_windows(ctx));
        assert_eq!(reopened.windows.len(), 1);
        assert!(!reopened.windows.contains_key(&egui::ViewportId::ROOT));
        let owner = reopened.workspace_window(tab);
        assert_eq!(reopened.windows[&owner].active, Some(tab));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Visible(false))
        );
    }

    #[test]
    fn closed_window_tabs_stay_closed_and_old_snapshots_migrate_to_one_window() {
        let ctx = egui::Context::default();
        let mut app = fixture(3);
        let closed = app.saved.workspaces[1].task.id;
        app.detach_workspace(&ctx, closed, None);
        app.close_terminal_window(&ctx, app.workspace_window(closed));
        app.capture_restoration();
        let mut reopened = fixture(0);
        reopened.saved = app.saved.clone();
        reopened.restore_windows(&ctx);
        assert_eq!(reopened.windows.len(), 1);
        assert!(
            !reopened
                .saved
                .workspaces
                .iter()
                .any(|w| w.task.id == closed)
        );
        let mut legacy = serde_json::to_value(&reopened.saved).unwrap();
        legacy["restoration"]
            .as_object_mut()
            .unwrap()
            .remove("windows");
        let mut migrated = fixture(0);
        migrated.saved = serde_json::from_value(legacy).unwrap();
        migrated.restore_windows(&ctx);
        assert_eq!(migrated.windows.len(), 1);
        for workspace in &migrated.saved.workspaces {
            assert_eq!(
                migrated.workspace_window(workspace.task.id),
                egui::ViewportId::ROOT
            );
        }
    }

    #[test]
    fn open_empty_windows_survive_without_reviving_their_closed_tabs() {
        let ctx = egui::Context::default();
        let mut app = fixture(2);
        let tab = app.saved.workspaces[1].task.id;
        let pane = app.saved.workspaces[1].focus;
        app.detach_workspace(&ctx, tab, None);
        app.stop_terminal(pane);
        app.capture_restoration();
        let mut reopened = fixture(0);
        reopened.saved = app.saved.clone();
        reopened.restore_windows(&ctx);
        assert_eq!(reopened.windows.len(), 2);
        assert_eq!(reopened.saved.workspaces.len(), 1);
        let detached = reopened
            .windows
            .iter()
            .find(|(id, _)| **id != egui::ViewportId::ROOT)
            .unwrap()
            .1;
        assert_eq!(detached.active, None);
    }

    #[test]
    fn stale_window_metadata_cannot_duplicate_tabs_or_hide_unassigned_workspaces() {
        let ctx = egui::Context::default();
        let mut app = fixture(3);
        let tabs: Vec<_> = app.saved.workspaces.iter().map(|w| w.task.id).collect();
        let stale = Uuid::new_v4();
        app.saved.restoration.windows = vec![
            windows::SavedWindow {
                root: true,
                workspaces: vec![tabs[0], stale],
                active: Some(stale),
                bounds: Some([0.0, 0.0, -1.0, 400.0]),
            },
            windows::SavedWindow {
                root: false,
                workspaces: vec![tabs[0], tabs[1]],
                active: Some(tabs[0]),
                bounds: Some([50.0, 80.0, 900.0, 600.0]),
            },
        ];
        app.restore_windows(&ctx);
        assert_eq!(app.windows.len(), 2);
        let root = &app.windows[&egui::ViewportId::ROOT];
        assert_eq!(root.active, Some(tabs[0]));
        assert_eq!(root.bounds, None);
        let child = app.workspace_window(tabs[1]);
        assert_ne!(child, egui::ViewportId::ROOT);
        assert_eq!(app.windows[&child].active, Some(tabs[1]));
        assert_eq!(app.windows[&child].bounds, Some([50.0, 80.0, 900.0, 600.0]));
        assert_eq!(app.workspace_window(tabs[2]), egui::ViewportId::ROOT);
        app.capture_restoration();
        let saved_tabs: Vec<_> = app
            .saved
            .restoration
            .windows
            .iter()
            .flat_map(|w| w.workspaces.clone())
            .collect();
        assert_eq!(saved_tabs.len(), 3);
        for tab in tabs {
            assert_eq!(saved_tabs.iter().filter(|id| **id == tab).count(), 1);
        }
    }

    #[test]
    fn saved_history_is_bounded_utf8_and_cannot_replay_terminal_controls() {
        let long = format!(
            "{}\n{}",
            "old\n".repeat(crate::terminal::SNAPSHOT_MAX_LINES + 20),
            "é".repeat(crate::terminal::SNAPSHOT_MAX_BYTES)
        );
        let output = bounded(&long);
        assert!(output.len() <= crate::terminal::SNAPSHOT_MAX_BYTES);
        assert!(output.lines().count() <= crate::terminal::SNAPSHOT_MAX_LINES);
        assert!(!output.contains("old"));
        let controls = bounded("visible\x1b]52;c;Y29weQ==\x07\x1b[6n\r\x00text\nnext");
        assert!(controls.chars().all(|c| !c.is_control() || c == '\n'));
        assert!(controls.contains("visible"));
    }
}
