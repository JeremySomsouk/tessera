use crate::{
    integration::{Endpoint, now},
    model::{Session, SessionState, Task, TaskState},
    search::TerminalSearch,
    selection::{SelectionAction, key_action},
    terminal::{Size, Terminal, color},
};
use alacritty_terminal::{
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{TermMode, cell::Flags},
    vte::ansi::{CursorShape, NamedColor},
};
use eframe::egui::{
    self, Color32, FontId, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::Ordering,
        mpsc::{SyncSender, sync_channel},
    },
    time::Duration,
};
use uuid::Uuid;
const ACCENT: Color32 = Color32::from_rgb(130, 198, 180);
#[derive(Clone, Serialize, Deserialize)]
enum Layout {
    Pane(Uuid),
    Split {
        vertical: bool,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}
impl Layout {
    fn split(&mut self, id: Uuid, new: Uuid, vertical: bool) -> bool {
        match self {
            Self::Pane(p) if *p == id => {
                *self = Self::Split {
                    vertical,
                    ratio: 0.5,
                    a: Box::new(Self::Pane(id)),
                    b: Box::new(Self::Pane(new)),
                };
                true
            }
            Self::Split { a, b, .. } => a.split(id, new, vertical) || b.split(id, new, vertical),
            _ => false,
        }
    }
    fn ids(&self, out: &mut Vec<Uuid>) {
        match self {
            Self::Pane(id) => out.push(*id),
            Self::Split { a, b, .. } => {
                a.ids(out);
                b.ids(out);
            }
        }
    }
    fn remove(&mut self, id: Uuid) -> bool {
        if let Self::Split { a, b, .. } = self {
            if matches!(**a,Self::Pane(p) if p==id) {
                *self = (**b).clone();
                return true;
            }
            if matches!(**b,Self::Pane(p) if p==id) {
                *self = (**a).clone();
                return true;
            }
            return a.remove(id) || b.remove(id);
        }
        false
    }
    fn rects(&mut self, ui: &mut egui::Ui, rect: Rect, out: &mut Vec<(Uuid, Rect)>) {
        match self {
            Self::Pane(id) => out.push((*id, rect)),
            Self::Split {
                vertical,
                ratio,
                a,
                b,
            } => {
                let length = if *vertical {
                    rect.width()
                } else {
                    rect.height()
                };
                let cut = length * *ratio;
                let (ra, rb, handle) = if *vertical {
                    (
                        Rect::from_min_max(
                            rect.min,
                            Pos2::new(rect.left() + cut - 3.0, rect.bottom()),
                        ),
                        Rect::from_min_max(
                            Pos2::new(rect.left() + cut + 3.0, rect.top()),
                            rect.max,
                        ),
                        Rect::from_min_max(
                            Pos2::new(rect.left() + cut - 3.0, rect.top()),
                            Pos2::new(rect.left() + cut + 3.0, rect.bottom()),
                        ),
                    )
                } else {
                    (
                        Rect::from_min_max(
                            rect.min,
                            Pos2::new(rect.right(), rect.top() + cut - 3.0),
                        ),
                        Rect::from_min_max(
                            Pos2::new(rect.left(), rect.top() + cut + 3.0),
                            rect.max,
                        ),
                        Rect::from_min_max(
                            Pos2::new(rect.left(), rect.top() + cut - 3.0),
                            Pos2::new(rect.right(), rect.top() + cut + 3.0),
                        ),
                    )
                };
                let response = ui.interact(
                    handle,
                    ui.id()
                        .with((ra.min.x.to_bits(), ra.min.y.to_bits(), "split")),
                    Sense::drag(),
                );
                if response.dragged() {
                    let delta = if *vertical {
                        response.drag_delta().x
                    } else {
                        response.drag_delta().y
                    };
                    *ratio = (*ratio + delta / length).clamp(0.15, 0.85);
                }
                ui.painter().rect_filled(
                    handle,
                    2.0,
                    if response.hovered() {
                        ACCENT
                    } else {
                        ui.visuals().widgets.noninteractive.bg_stroke.color
                    },
                );
                a.rects(ui, ra, out);
                b.rects(ui, rb, out);
            }
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Workspace {
    task: Task,
    layout: Layout,
    focus: Uuid,
}
#[derive(Default, Clone, Serialize, Deserialize)]
struct Saved {
    workspaces: Vec<Workspace>,
    sessions: Vec<Session>,
    #[serde(default = "default_font")]
    font_size: f32,
    #[serde(default)]
    light: bool,
    #[serde(default)]
    skip_stop_confirmation: bool,
    #[serde(default)]
    clickable_codex_choices: bool,
}
fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        ACCENT
    } else {
        Color32::from_rgb(27, 108, 87)
    }
}
fn default_font() -> f32 {
    15.0
}
struct Pane {
    terminal: Terminal,
    search: TerminalSearch,
}
struct TerminalClose {
    pane: Uuid,
    skip_confirmation: bool,
}

struct WorkspaceRename {
    workspace: Uuid,
    title: String,
    focus: bool,
}

pub struct App {
    updater: crate::updater::Updater,
    updater_error: Option<String>,
    saved: Saved,
    panes: HashMap<Uuid, Pane>,
    active: usize,
    overview: bool,
    selected: Option<String>,
    endpoint: Option<Endpoint>,
    error: String,
    writer: SyncSender<Saved>,
    persistence: Option<std::thread::JoinHandle<()>>,
    save_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    dirty: bool,
    last_save: u64,
    font_size: f32,
    new_directory: String,
    new_workspace_name: String,
    palette: bool,
    rename: Option<WorkspaceRename>,
    closing: Option<TerminalClose>,
    filter: String,
    maximized: bool,
    category: u8,
}
fn state_path() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Tessera/workspace.json")
    } else {
        home.join(".config/tessera/workspace.json")
    }
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut path = state_path();
        let (saved, load_error) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Saved>(&bytes) {
                Ok(s) => (s, String::new()),
                Err(e) => (
                    Saved::default(),
                    format!("Could not read saved workspace: {e}. Original file kept."),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Saved::default(), String::new()),
            Err(e) => (Saved::default(), e.to_string()),
        };
        if !load_error.is_empty() && path.exists() {
            path = path.with_extension("recovered.json");
        }
        let endpoint = Endpoint::start(cc.egui_ctx.clone());
        let (writer, rx) = sync_channel::<Saved>(1);
        let persistence_ctx = cc.egui_ctx.clone();
        let save_error = std::sync::Arc::new(std::sync::Mutex::new(None));
        let worker_error = save_error.clone();
        let persistence = std::thread::spawn(move || {
            while let Ok(s) = rx.recv() {
                let result = (|| -> anyhow::Result<()> {
                    let parent = path.parent().unwrap();
                    std::fs::create_dir_all(parent)?;
                    let tmp = path.with_extension("tmp");
                    std::fs::write(&tmp, serde_json::to_vec_pretty(&s)?)?;
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
                    std::fs::rename(tmp, &path)?;
                    Ok(())
                })();
                if let Err(e) = result
                    && let Ok(mut error) = worker_error.lock()
                {
                    *error = Some(format!("Workspace save failed: {e}"));
                }
                persistence_ctx.request_repaint();
            }
        });
        let mut app = Self {
            updater: crate::updater::Updater::default(),
            updater_error: None,
            font_size: if saved.font_size > 0.0 {
                saved.font_size
            } else {
                default_font()
            },
            saved,
            panes: HashMap::new(),
            active: 0,
            overview: false,
            selected: None,
            error: load_error,
            endpoint: None,
            writer,
            persistence: Some(persistence),
            save_error,
            dirty: false,
            last_save: 0,
            new_directory: std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            new_workspace_name: String::new(),
            palette: false,
            rename: None,
            closing: None,
            filter: String::new(),
            maximized: false,
            category: 0,
        };
        match endpoint {
            Ok(e) => app.endpoint = Some(e),
            Err(e) => app.error = format!("Event endpoint unavailable: {e}"),
        }
        match crate::updater::Updater::start() {
            Ok(updater) => app.updater = updater,
            Err(error) => app.updater_error = Some(format!("Updates unavailable: {error}")),
        }
        configure_terminal_fonts(&cc.egui_ctx);
        configure_appearance(&cc.egui_ctx, app.saved.light);
        app.start_fresh_workspace(&cc.egui_ctx);
        app
    }
    fn spawn(&mut self, id: Uuid, ctx: &egui::Context, directory: &str) -> bool {
        let socket = self
            .endpoint
            .as_ref()
            .map(|e| e.path.as_path())
            .unwrap_or_else(|| std::path::Path::new(""));
        match Terminal::spawn(id, &PathBuf::from(directory), socket, ctx.clone()) {
            Ok(terminal) => {
                self.panes.insert(
                    id,
                    Pane {
                        terminal,
                        search: TerminalSearch::default(),
                    },
                );
                true
            }
            Err(e) => {
                self.error = format!("Cannot start shell: {e}");
                false
            }
        }
    }
    fn focused_directory(&self) -> anyhow::Result<String> {
        let Some(workspace) = self.saved.workspaces.get(self.active) else {
            return Ok(self.new_directory.clone());
        };
        let Some(pane) = self.panes.get(&workspace.focus) else {
            return Ok(workspace.task.directory.clone());
        };
        pane.terminal
            .current_directory()?
            .into_os_string()
            .into_string()
            .map_err(|_| anyhow::anyhow!("working directory is not valid UTF-8"))
    }
    fn add_workspace_from_terminal(&mut self, ctx: &egui::Context) {
        let directory = match self.focused_directory() {
            Ok(directory) => directory,
            Err(error) => {
                self.error = format!("Cannot open terminal in current directory: {error}");
                return;
            }
        };
        if self.add_workspace_in(ctx, directory) {
            self.palette = false;
        }
    }
    fn add_workspace(&mut self, ctx: &egui::Context) -> bool {
        if !self.add_workspace_in(ctx, self.new_directory.clone()) {
            return false;
        }
        let name = self.new_workspace_name.trim();
        if !name.is_empty() {
            self.saved.workspaces[self.active].task.title = name.to_owned();
        }
        self.new_workspace_name.clear();
        true
    }
    fn add_workspace_in(&mut self, ctx: &egui::Context, directory: String) -> bool {
        if self.saved.workspaces.len() >= 32 {
            self.error = "Workspace limit (32) reached".into();
            return false;
        }
        if !PathBuf::from(&directory).is_dir() {
            self.error =
                format!("Working directory does not exist or is not a directory: {directory}");
            return false;
        }
        let id = Uuid::new_v4();
        if !self.spawn(id, ctx, &directory) {
            return false;
        }
        let title = PathBuf::from(&directory)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Workspace".into());
        self.saved.workspaces.push(Workspace {
            task: Task {
                id: Uuid::new_v4(),
                title,
                directory,
                state: TaskState::Implementing,
            },
            layout: Layout::Pane(id),
            focus: id,
        });
        self.active = self.saved.workspaces.len() - 1;
        self.overview = false;
        self.dirty = true;
        true
    }
    fn split(&mut self, ctx: &egui::Context, vertical: bool) {
        if self.panes.len() >= 32 {
            self.error = "Live pane limit (32) reached".into();
            return;
        }
        let Some(w) = self.saved.workspaces.get(self.active) else {
            return;
        };
        let old = w.focus;
        let directory = match self.focused_directory() {
            Ok(directory) => directory,
            Err(error) => {
                self.error = format!("Cannot split terminal in current directory: {error}");
                return;
            }
        };
        let id = Uuid::new_v4();
        if self.spawn(id, ctx, &directory) {
            let w = &mut self.saved.workspaces[self.active];
            w.layout.split(old, id, vertical);
            w.focus = id;
            self.dirty = true;
            self.overview = false;
            self.maximized = false;
        }
    }
    fn request_stop_terminal(&mut self, id: Uuid) {
        if self.saved.skip_stop_confirmation {
            self.stop_terminal(id);
        } else {
            self.palette = false;
            self.closing = Some(TerminalClose {
                pane: id,
                skip_confirmation: false,
            });
        }
    }
    fn confirm_stop_terminal(&mut self) {
        if let Some(closing) = self.closing.take() {
            self.saved.skip_stop_confirmation = closing.skip_confirmation;
            self.dirty = true;
            self.stop_terminal(closing.pane);
        }
    }
    fn close_dialog(&mut self, ctx: &egui::Context) {
        let Some(closing) = &mut self.closing else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("stop-terminal")).show(ctx, |ui| {
            ui.set_width(320.0_f32.min((ctx.content_rect().width() - 40.0).max(120.0)));
            ui.heading("Stop terminal?");
            ui.label("The shell and its running processes will be stopped.");
            ui.checkbox(
                &mut closing.skip_confirmation,
                "Don’t ask again for any terminal",
            );
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                confirm = ui.button("Stop terminal").clicked();
            });
        });
        if cancel || response.should_close() {
            self.closing = None;
        } else if confirm {
            self.confirm_stop_terminal();
        }
    }
    fn stop_terminal(&mut self, id: Uuid) {
        if self
            .closing
            .as_ref()
            .is_some_and(|closing| closing.pane == id)
        {
            self.closing = None;
        }
        self.panes.remove(&id);
        let workspace = self.saved.workspaces.iter().position(|w| {
            let mut ids = Vec::new();
            w.layout.ids(&mut ids);
            ids.contains(&id)
        });
        if let Some(index) = workspace {
            if matches!(self.saved.workspaces[index].layout, Layout::Pane(_)) {
                let removed = self.saved.workspaces.remove(index);
                if self
                    .rename
                    .as_ref()
                    .is_some_and(|rename| rename.workspace == removed.task.id)
                {
                    self.rename = None;
                }
                if index < self.active {
                    self.active -= 1;
                }
                self.active = self
                    .active
                    .min(self.saved.workspaces.len().saturating_sub(1));
            } else {
                let w = &mut self.saved.workspaces[index];
                w.layout.remove(id);
                if w.focus == id {
                    let mut ids = Vec::new();
                    w.layout.ids(&mut ids);
                    w.focus = ids[0];
                }
            }
            self.maximized = false;
            self.dirty = true;
        }
        for session in &mut self.saved.sessions {
            if session.pane == id && session.state != SessionState::Ended {
                session.state = SessionState::Disconnected;
                self.dirty = true;
            }
        }
    }
    fn start_fresh_workspace(&mut self, ctx: &egui::Context) {
        // Saved pane IDs refer to processes that stopped when the app exited.
        self.saved.workspaces.clear();
        for session in &mut self.saved.sessions {
            if session.state != SessionState::Ended {
                session.state = SessionState::Disconnected;
            }
        }
        self.active = 0;
        self.overview = false;
        self.dirty = true;
        self.add_workspace(ctx);
    }
    fn drain(&mut self) {
        if let Some(endpoint) = &self.endpoint {
            for e in endpoint.receiver.try_iter().take(256) {
                if !self.panes.contains_key(&e.pane) {
                    continue;
                }
                let idx = self.saved.sessions.iter().position(|s| {
                    s.session_id == e.session_id && s.pane == e.pane && s.agent == e.agent
                });
                let idx = idx.unwrap_or_else(|| {
                    if self.saved.sessions.len() >= 256 {
                        self.saved.sessions.remove(0);
                    }
                    let mut session = Session::new(e.pane, e.session_id.clone());
                    session.agent = e.agent;
                    self.saved.sessions.push(session);
                    self.saved.sessions.len() - 1
                });
                self.dirty |= self.saved.sessions[idx].apply(e, now());
            }
        }
        let exited: Vec<_> = self
            .panes
            .iter()
            .filter_map(|(&id, pane)| pane.terminal.has_exited().then_some(id))
            .collect();
        for id in exited {
            self.stop_terminal(id);
        }
        for s in &mut self.saved.sessions {
            if s.state != SessionState::Ended
                && s.state != SessionState::Disconnected
                && self
                    .panes
                    .get(&s.pane)
                    .is_none_or(|p| !p.terminal.alive.load(Ordering::Acquire))
            {
                s.state = SessionState::Disconnected;
                self.dirty = true;
            }
        }
    }
    fn focus_session(&mut self, id: &str) {
        let Some(pane) = self
            .saved
            .sessions
            .iter()
            .find(|s| s.key() == id)
            .map(|s| s.pane)
        else {
            return;
        };
        for (i, w) in self.saved.workspaces.iter_mut().enumerate() {
            let mut ids = Vec::new();
            w.layout.ids(&mut ids);
            if ids.contains(&pane) {
                self.active = i;
                w.focus = pane;
                self.overview = false;
                self.maximized = false;
                return;
            }
        }
    }
    fn begin_rename(&mut self) {
        if let Some(workspace) = self.saved.workspaces.get(self.active) {
            self.rename = Some(WorkspaceRename {
                workspace: workspace.task.id,
                title: workspace.task.title.clone(),
                focus: true,
            });
            self.palette = false;
        }
    }
    fn finish_rename(&mut self) {
        let Some(rename) = &self.rename else {
            return;
        };
        let title = rename.title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(workspace) = self
            .saved
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.task.id == rename.workspace)
            && workspace.task.title != title
        {
            workspace.task.title = title.to_owned();
            self.dirty = true;
        }
        self.rename = None;
    }
    fn rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(rename) = &mut self.rename else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("rename-workspace")).show(ctx, |ui| {
            ui.set_width(280.0_f32.min((ctx.content_rect().width() - 40.0).max(120.0)));
            ui.heading("Rename workspace");
            let label = ui.label("Name");
            let mut field = egui::TextEdit::singleline(&mut rename.title)
                .id(egui::Id::new("workspace-name"))
                .desired_width(ui.available_width())
                .show(ui);
            field.response = field.response.labelled_by(label.id);
            if rename.focus {
                field.response.request_focus();
                field
                    .state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(rename.title.chars().count()),
                    )));
                field.state.store(ctx, field.response.id);
                rename.focus = false;
            }
            let valid = !rename.title.trim().is_empty();
            if field.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = valid;
                if !valid {
                    field.response.request_focus();
                }
            }
            ui.horizontal(|ui| {
                submit |= ui.add_enabled(valid, egui::Button::new("Rename")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel || response.should_close() {
            self.rename = None;
        } else if submit {
            self.finish_rename();
        }
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let command = if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL | Modifiers::ALT
        };
        if self.rename.is_some() || self.closing.is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::R)) {
            self.begin_rename();
            return;
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::O)) {
            self.overview = !self.overview;
        }
        if !self.overview
            && !self.palette
            && ctx.input_mut(|i| i.consume_key(command, Key::F))
            && let Some(pane) = self
                .saved
                .workspaces
                .get(self.active)
                .and_then(|w| self.panes.get_mut(&w.focus))
        {
            if pane.terminal.copy_mode
                && let Err(error) = pane.terminal.selection_action(SelectionAction::Exit)
            {
                self.error = error.to_string();
            }
            pane.search.open();
        }
        if !self.overview
            && !self.palette
            && ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::Space))
            && let Some(pane) = self
                .saved
                .workspaces
                .get(self.active)
                .and_then(|w| self.panes.get_mut(&w.focus))
        {
            pane.search.close();
            if let Err(error) = pane.terminal.selection_action(SelectionAction::Enter) {
                self.error = error.to_string();
            }
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::N) || i.consume_key(command, Key::T)) {
            self.add_workspace_from_terminal(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::D)) {
            self.split(ctx, false);
        } else if ctx.input_mut(|i| i.consume_key(command, Key::D)) {
            self.split(ctx, true);
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::Enter)) {
            self.maximized = !self.maximized;
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::P)) {
            self.palette = !self.palette;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::W))
            && let Some(id) = self.saved.workspaces.get(self.active).map(|w| w.focus)
        {
            self.request_stop_terminal(id);
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::N))
            && let Some(s) = self.saved.sessions.iter().find(|s| s.state.attention())
        {
            let id = s.key();
            self.focus_session(&id);
        }
        if let Some(index) =
            ctx.input_mut(|input| workspace_shortcut(input, command, self.saved.workspaces.len()))
        {
            self.active = index;
            self.overview = false;
            self.palette = false;
            self.maximized = false;
        }
        if !self.palette
            && !self.overview
            && ctx.input_mut(|i| i.consume_key(command | Modifiers::ALT, Key::ArrowRight))
            && let Some(w) = self.saved.workspaces.get_mut(self.active)
        {
            let mut ids = Vec::new();
            w.layout.ids(&mut ids);
            if let Some(n) = ids.iter().position(|p| *p == w.focus) {
                w.focus = ids[(n + 1) % ids.len()];
            }
        }
    }
    fn overview(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Your mosaic").size(28.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!(
                    "{} need you",
                    self.saved
                        .sessions
                        .iter()
                        .filter(|s| s.state.attention())
                        .count()
                ));
            });
        });
        ui.label(
            RichText::new("Real sessions. Clear attention. One place to return.")
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(20.0);
        if self.saved.sessions.is_empty() {
            egui::Frame::group(ui.style()).inner_margin(24.0).show(ui,|ui| {ui.heading("Your first session starts in a terminal");ui.label("Enable Tessera hooks for Claude Code or Codex, then run the agent in any pane. Sessions appear as events arrive.");ui.add_space(12.0);ui.monospace("tessera install-hooks         # Claude Code\ntessera install-codex-hooks   # Codex; trust via /hooks\nclaude  # or codex");if ui.button("Return to terminal").clicked() {self.overview=false;}});
        }
        ui.horizontal_wrapped(|ui| {
            for (category, label) in [
                (0, "All sessions"),
                (1, "Needs you"),
                (2, "Running"),
                (3, "Review requested"),
                (4, "Accepted"),
                (5, "Disconnected"),
            ] {
                if ui
                    .selectable_label(self.category == category, label)
                    .clicked()
                {
                    self.category = category;
                }
            }
        });
        ui.add_space(12.0);
        let visible: Vec<usize> = self
            .saved
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(index, s)| {
                let task = self
                    .saved
                    .workspaces
                    .iter()
                    .find(|w| {
                        let mut ids = Vec::new();
                        w.layout.ids(&mut ids);
                        ids.contains(&s.pane)
                    })
                    .map(|w| w.task.state);
                let show = match self.category {
                    1 => s.state.attention(),
                    2 => s.state == SessionState::Running,
                    3 => task == Some(TaskState::ReviewRequested),
                    4 => task == Some(TaskState::Accepted),
                    5 => s.state == SessionState::Disconnected,
                    _ => true,
                };
                show.then_some(index)
            })
            .collect();
        let narrow = ui.available_width() < 800.0;
        let list_width = if narrow {
            ui.available_width()
        } else {
            ui.available_width() * 0.56
        };
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(
                    list_width,
                    if narrow && self.selected.is_some() {
                        ui.available_height() * 0.44
                    } else {
                        ui.available_height()
                    },
                ),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sessions")
                        .max_height(ui.available_height())
                        .show_rows(ui, 145.0, visible.len(), |ui, range| {
                            for index in &visible[range] {
                                let s = &self.saved.sessions[*index];
                                let mut title = "Detached session".to_owned();
                                let mut directory = String::new();
                                for w in &self.saved.workspaces {
                                    let mut ids = Vec::new();
                                    w.layout.ids(&mut ids);
                                    if ids.contains(&s.pane) {
                                        title = w.task.title.clone();
                                        directory = w.task.directory.clone();
                                        break;
                                    }
                                }
                                let selected = self.selected.as_deref() == Some(s.key().as_str());
                                let tint = if s.state.attention() {
                                    if ui.visuals().dark_mode {
                                        Color32::from_rgb(233, 182, 118)
                                    } else {
                                        Color32::from_rgb(138, 78, 6)
                                    }
                                } else if s.state == SessionState::Running {
                                    accent(ui)
                                } else {
                                    ui.visuals().weak_text_color()
                                };
                                let frame = egui::Frame::group(ui.style())
                                    .fill(if selected {
                                        ui.visuals().selection.bg_fill.gamma_multiply(0.4)
                                    } else {
                                        ui.visuals().panel_fill
                                    })
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        if selected {
                                            ACCENT
                                        } else {
                                            ui.visuals().widgets.noninteractive.bg_stroke.color
                                        },
                                    ))
                                    .inner_margin(16.0)
                                    .corner_radius(10.0);
                                let response = frame
                                    .show(ui, |ui| {
                                        ui.set_min_width((list_width - 40.0).max(100.0));
                                        ui.set_min_height(103.0);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&title).strong().size(18.0),
                                                )
                                                .truncate(),
                                            );
                                            ui.label(RichText::new(s.state.label()).color(tint));
                                        });
                                        ui.label(
                                            RichText::new(directory)
                                                .small()
                                                .color(ui.visuals().weak_text_color()),
                                        );
                                        ui.small(format!(
                                            "{} · Session {}",
                                            s.agent.label(),
                                            s.session_id.chars().take(12).collect::<String>()
                                        ));
                                        ui.add_space(4.0);
                                        ui.label(
                                            s.history
                                                .back()
                                                .map(|a| {
                                                    if a.detail.is_empty() {
                                                        a.kind.clone()
                                                    } else {
                                                        format!("{} · {}", a.kind, a.detail)
                                                    }
                                                })
                                                .unwrap_or_else(|| "No event details".into()),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "{}s since event · {} new events",
                                                now().saturating_sub(s.updated),
                                                s.history
                                                    .iter()
                                                    .filter(|a| a.sequence > s.seen_sequence)
                                                    .count()
                                            ))
                                            .small(),
                                        );
                                    })
                                    .response;
                                if ui
                                    .interact(response.rect, ui.id().with(s.key()), Sense::click())
                                    .clicked()
                                {
                                    self.selected = Some(s.key());
                                }
                                ui.add_space(10.0);
                            }
                        });
                },
            );
            if !narrow {
                ui.separator();
                self.inspector(ui);
            }
        });
        if narrow {
            self.inspector(ui);
        }
        if !ui.ctx().wants_keyboard_input() {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.overview = false;
            }
            let ids: Vec<_> = visible
                .iter()
                .map(|index| self.saved.sessions[*index].key())
                .collect();
            if !ids.is_empty() {
                let n = self
                    .selected
                    .as_ref()
                    .and_then(|id| ids.iter().position(|i| i == id))
                    .unwrap_or(0);
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                    self.selected = Some(ids[(n + 1) % ids.len()].clone());
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                    self.selected = Some(ids[(n + ids.len() - 1) % ids.len()].clone());
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                    self.focus_session(&ids[n]);
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    self.overview = false;
                }
            }
        }
    }
    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected.clone() else {
            ui.label("Select a session to inspect its activity.");
            return;
        };
        let Some(index) = self.saved.sessions.iter().position(|s| s.key() == id) else {
            return;
        };
        let session = self.saved.sessions[index].clone();
        egui::ScrollArea::vertical().id_salt("inspector").max_height(ui.available_height()).show(ui,|ui| {
            ui.heading("Session context");ui.label(session.state.label());ui.add_space(8.0);
            let live=self.panes.contains_key(&session.pane);
            if ui.add_enabled(live,egui::Button::new("Open live terminal ↗")).clicked() {self.saved.sessions[index].seen=now();self.saved.sessions[index].seen_sequence=self.saved.sessions[index].last_sequence;self.dirty=true;self.focus_session(&id);}
            let wi=self.saved.workspaces.iter().position(|w|{let mut ids=Vec::new();w.layout.ids(&mut ids);ids.contains(&session.pane)});
            if !live {ui.label("This terminal was closed. Its session history is retained.");}
            if let Some(wi)=wi {
                ui.add_space(14.0);ui.label("Task title");let w=&mut self.saved.workspaces[wi];if ui.text_edit_singleline(&mut w.task.title).changed() {self.dirty=true;}
                ui.horizontal_wrapped(|ui| {for (state,label) in [(TaskState::Implementing,"Implementing"),(TaskState::ReviewRequested,"Request review"),(TaskState::Accepted,"Accept task")] {if ui.selectable_label(w.task.state==state,label).clicked() {w.task.state=state;self.dirty=true;}}});
            }
            ui.add_space(16.0);ui.label(RichText::new("Since your last visit").strong());
            egui::ScrollArea::vertical().max_height(380.0).id_salt("history").show(ui,|ui| {for a in session.history.iter().rev() {ui.label(RichText::new(&a.kind).color(if a.sequence>session.seen_sequence {accent(ui)}else{ui.visuals().weak_text_color()}));if !a.detail.is_empty() {ui.label(&a.detail);}ui.label(RichText::new(format!("Event at {} (Unix seconds)",a.at)).small());ui.add_space(7.0);}});
            ui.add_space(8.0);ui.label(RichText::new("Stop marks a response as finished. Review and acceptance are explicit task actions.").small());
        });
    }
}
impl App {
    fn draw(&mut self, ctx: &egui::Context) {
        self.drain();
        if let Some(error) = self.save_error.lock().ok().and_then(|mut e| e.take()) {
            self.error = error;
        }
        self.shortcuts(ctx);
        let dismiss_palette =
            self.palette && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        let workspace_submit = if self.palette {
            workspace_submission(ctx)
        } else {
            None
        };
        egui::TopBottomPanel::top("chrome").show(ctx, |ui| {
            ui.add_space(7.0);
            ui.horizontal_wrapped(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                for (x, y) in [(0.0, 0.0), (12.0, 0.0), (0.0, 12.0), (12.0, 12.0)] {
                    ui.painter().rect_filled(
                        Rect::from_min_size(r.min + Vec2::new(x, y), Vec2::splat(9.0)),
                        2.0,
                        ACCENT.gamma_multiply(if x == y { 1.0 } else { 0.6 }),
                    );
                }
                ui.label(RichText::new("TESSERA").strong());
                ui.separator();
                if ui
                    .selectable_label(self.overview, "Overview")
                    .on_hover_text(shortcut("Overview", "Shift+O"))
                    .clicked()
                {
                    self.overview = !self.overview;
                }
                if ui
                    .button("+ Workspace")
                    .on_hover_text(shortcut("Workspace settings", "Shift+P"))
                    .clicked()
                {
                    self.palette = true;
                }
                if ui
                    .button("Split ↔")
                    .on_hover_text(shortcut("Side-by-side split", "D"))
                    .clicked()
                {
                    self.split(ctx, true);
                }
                if ui
                    .button("Split ↕")
                    .on_hover_text(shortcut("Stacked split", "Shift+D"))
                    .clicked()
                {
                    self.split(ctx, false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(if self.saved.light { "Dark" } else { "Light" })
                        .on_hover_text("Switch appearance")
                        .clicked()
                    {
                        self.saved.light = !self.saved.light;
                        configure_appearance(ctx, self.saved.light);
                        self.dirty = true;
                    }
                    if ui
                        .add(egui::Slider::new(&mut self.font_size, 11.0..=24.0).show_value(false))
                        .on_hover_text("Terminal font size")
                        .changed()
                    {
                        self.saved.font_size = self.font_size;
                        self.dirty = true;
                    }
                });
            });
            ui.add_space(5.0);
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, w) in self.saved.workspaces.iter().enumerate() {
                        if ui
                            .selectable_label(
                                index == self.active && !self.overview,
                                format!("{}  {}", index + 1, w.task.title),
                            )
                            .on_hover_text(if index < 9 {
                                shortcut("Switch workspace", &(index + 1).to_string())
                            } else {
                                "Switch workspace".into()
                            })
                            .clicked()
                        {
                            self.active = index;
                            self.overview = false;
                            self.maximized = false;
                        }
                    }
                });
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.error.is_empty() {
                    ui.label(
                        RichText::new(if cfg!(target_os = "macos") { "Cmd+D split · Cmd+Shift+D stack · Cmd+Alt+Right next pane · Cmd+Shift+P commands" } else { "Ctrl+Alt+D split · Ctrl+Alt+Shift+D stack · Ctrl+Alt+Right next pane · Ctrl+Alt+Shift+P commands" })
                            .small(),
                    );
                } else {
                    ui.colored_label(Color32::from_rgb(233, 150, 150), &self.error);
                    if ui.small_button("Dismiss").clicked() {
                        self.error.clear();
                    }
                }
            });
        });
        let mut stop = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.overview {
                self.overview(ui);
                return;
            }
            let Some(w) = self.saved.workspaces.get_mut(self.active) else {
                ui.label("Create a workspace to start a shell.");
                return;
            };
            let mut rects = Vec::new();
            let rect = ui.available_rect_before_wrap();
            if self.maximized {
                rects.push((w.focus, rect));
            } else {
                w.layout.rects(ui, rect, &mut rects);
            }
            let mut focused = w.focus;
            if !self.palette && self.rename.is_none() && self.closing.is_none() {
                let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
                if !dropped.is_empty() {
                    let pointer = ctx.input(|i| i.pointer.latest_pos());
                    let target = pointer.map_or(Some(focused), |pos| {
                        rects
                            .iter()
                            .find(|(_, rect)| rect.contains(pos))
                            .map(|(id, _)| *id)
                    });
                    let result = target
                        .and_then(|id| self.panes.get_mut(&id).map(|pane| (id, pane)))
                        .ok_or_else(|| anyhow::anyhow!("Drop files onto a running terminal pane"))
                        .and_then(|(id, pane)| {
                            if pane.search.is_open() || pane.terminal.copy_mode {
                                anyhow::bail!("Close Find or copy mode before dropping files");
                            }
                            pane.terminal.input_at_cursor(dropped_file_input(
                                &dropped,
                                pane.terminal.mode(),
                            )?)?;
                            self.saved.workspaces[self.active].focus = id;
                            focused = id;
                            self.dirty = true;
                            Ok(())
                        });
                    if let Err(error) = result {
                        self.error = error.to_string();
                    }
                }
            }
            for (id, rect) in rects {
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect).id_salt(id), |ui| {
                    if let Some(pane) = self.panes.get_mut(&id) {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if focused == id {
                                    "● Terminal"
                                } else {
                                    "Terminal"
                                })
                                .color(if focused == id {
                                    ACCENT
                                } else {
                                    ui.visuals().weak_text_color()
                                }),
                            );
                            if ui
                                .small_button("Select")
                                .on_hover_text(shortcut("Keyboard selection", "Shift+Space"))
                                .clicked()
                            {
                                pane.search.close();
                                if let Err(error) =
                                    pane.terminal.selection_action(SelectionAction::Enter)
                                {
                                    self.error = error.to_string();
                                }
                                self.saved.workspaces[self.active].focus = id;
                                self.dirty = true;
                            }
                            if ui
                                .small_button("Find")
                                .on_hover_text(shortcut("Find in terminal", "F"))
                                .clicked()
                            {
                                if pane.terminal.copy_mode
                                    && let Err(error) =
                                        pane.terminal.selection_action(SelectionAction::Exit)
                                {
                                    self.error = error.to_string();
                                }
                                pane.search.open();
                                self.saved.workspaces[self.active].focus = id;
                                self.dirty = true;
                            }
                            if ui
                                .small_button("×")
                                .on_hover_text(shortcut("Stop terminal", "W"))
                                .clicked()
                            {
                                stop = Some(id);
                            }
                            if !pane.terminal.alive.load(Ordering::Acquire) {
                                ui.label("Shell exited");
                            }
                        });
                        if let Some(err) =
                            pane.terminal.error.try_lock().ok().and_then(|e| e.clone())
                        {
                            ui.small(err);
                        }
                        let (clicked, result) = terminal_view(
                            ui,
                            &mut pane.terminal,
                            &mut pane.search,
                            focused == id
                                && !self.palette
                                && self.rename.is_none()
                                && self.closing.is_none()
                                && stop.is_none(),
                            self.font_size,
                            self.saved.clickable_codex_choices
                                && !self.palette
                                && self.rename.is_none()
                                && self.closing.is_none(),
                        );
                        if clicked {
                            self.saved.workspaces[self.active].focus = id;
                            self.dirty = true;
                        }
                        if let Err(e) = result {
                            self.error = e.to_string();
                        }
                    } else {
                        ui.label("Terminal unavailable. Create a new workspace to start a shell.");
                    }
                });
            }
        });
        if self.palette {
            let response = egui::Modal::new(egui::Id::new("workspace-commands")).show(ctx, |ui| {
                ui.set_width(380.0_f32.min((ctx.content_rect().width() - 40.0).max(120.0)));
                ui.heading("Workspace & commands");
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 100.0).max(100.0))
                    .show(ui, |ui| {
                        ui.label("Working directory");
                        let directory = ui.add(
                            egui::TextEdit::singleline(&mut self.new_directory)
                                .id(egui::Id::new("workspace-directory")),
                        );
                        let name_label = ui.label("Name (optional)");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_workspace_name)
                                .id(egui::Id::new("new-workspace-name"))
                                .hint_text("Defaults to the directory name"),
                        )
                        .labelled_by(name_label.id);
                        let create = ui.add_enabled(
                            !self.new_directory.trim().is_empty(),
                            egui::Button::new("Create workspace"),
                        );
                        if create.clicked()
                            || workspace_submit == Some(egui::Id::new("workspace-directory"))
                            || workspace_submit == Some(egui::Id::new("new-workspace-name"))
                        {
                            if self.add_workspace(ctx) {
                                self.palette = false;
                            } else {
                                directory.request_focus();
                            }
                        }
                        if !self.error.is_empty() {
                            ui.colored_label(ui.visuals().error_fg_color, &self.error);
                        }
                        ui.separator();
                        ui.label("Find workspace");
                        let filter = ui.add(
                            egui::TextEdit::singleline(&mut self.filter)
                                .id(egui::Id::new("workspace-filter")),
                        );
                        let open_first =
                            workspace_submit == Some(egui::Id::new("workspace-filter"));
                        let filter_text = self.filter.to_lowercase();
                        let mut first = true;
                        for (i, w) in self.saved.workspaces.iter().enumerate() {
                            if !w.task.title.to_lowercase().contains(&filter_text) {
                                continue;
                            }
                            let button = ui.button(&w.task.title);
                            if button.clicked() || (open_first && first) {
                                self.active = i;
                                self.overview = false;
                                self.palette = false;
                            }
                            first = false;
                        }
                        if first {
                            ui.small("No matching workspace");
                            if open_first {
                                filter.request_focus();
                            }
                        }
                        ui.separator();
                        if ui
                            .button("Toggle Overview")
                            .on_hover_text(shortcut("Overview", "Shift+O"))
                            .clicked()
                        {
                            self.overview = !self.overview;
                            self.palette = false;
                        }
                        if ui
                            .button("Maximize / restore pane")
                            .on_hover_text(shortcut("Maximize / restore pane", "Shift+Enter"))
                            .clicked()
                        {
                            self.maximized = !self.maximized;
                            self.palette = false;
                        }
                        if ui
                            .add_enabled(
                                !self.saved.workspaces.is_empty(),
                                egui::Button::new("Rename workspace"),
                            )
                            .on_hover_text(shortcut("Rename workspace", "Shift+R"))
                            .clicked()
                        {
                            self.begin_rename();
                        }
                        let mut confirm_stop = !self.saved.skip_stop_confirmation;
                        if ui
                            .checkbox(&mut confirm_stop, "Confirm before stopping terminals")
                            .changed()
                        {
                            self.saved.skip_stop_confirmation = !confirm_stop;
                            self.dirty = true;
                        }
                        if ui
                            .checkbox(
                                &mut self.saved.clickable_codex_choices,
                                "Clickable Codex questions",
                            )
                            .on_hover_text("Click an option to select it. Press Enter to submit.")
                            .changed()
                        {
                            self.dirty = true;
                        }
                        ui.separator();
                        ui.label(format!("Tessera {}", env!("CARGO_PKG_VERSION")));
                        if self.updater.available() {
                            if ui
                                .add_enabled(
                                    self.updater.can_check(),
                                    egui::Button::new("Check for updates…"),
                                )
                                .clicked()
                            {
                                self.updater.check();
                            }
                            let mut automatic = self.updater.automatic_checks();
                            if ui
                                .checkbox(&mut automatic, "Check for updates automatically")
                                .changed()
                            {
                                self.updater.set_automatic_checks(automatic);
                            }
                            let mut download = self.updater.automatic_downloads();
                            if ui
                                .add_enabled(
                                    automatic,
                                    egui::Checkbox::new(
                                        &mut download,
                                        "Download updates automatically",
                                    ),
                                )
                                .changed()
                            {
                                self.updater.set_automatic_downloads(download);
                            }
                            ui.small("Downloaded updates install when Tessera quits.");
                        } else if let Some(error) = &self.updater_error {
                            ui.add_enabled(false, egui::Button::new("Check for updates…"));
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        } else {
                            ui.small("Automatic updates require the macOS application bundle.");
                        }
                        if ui.button("Close").clicked() {
                            self.palette = false;
                        }
                    });
            });
            if dismiss_palette || response.should_close() {
                self.palette = false;
            }
        }
        self.rename_dialog(ctx);
        if let Some(id) = stop {
            self.request_stop_terminal(id);
        }
        self.close_dialog(ctx);
        if self.dirty {
            if now().saturating_sub(self.last_save) >= 2 {
                match self.writer.try_send(self.saved.clone()) {
                    Ok(()) => {
                        self.dirty = false;
                        self.last_save = now();
                    }
                    Err(_) => ctx.request_repaint_after(Duration::from_millis(100)),
                }
            } else {
                ctx.request_repaint_after(Duration::from_secs(2));
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.draw(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.writer.send(self.saved.clone());
        let (dummy, _) = sync_channel(1);
        drop(std::mem::replace(&mut self.writer, dummy));
        if let Some(handle) = self.persistence.take() {
            let _ = handle.join();
        }
    }
}
fn configure_terminal_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    const SYMBOLS: &str = "NotoSansSymbols2";
    fonts.font_data.insert(
        SYMBOLS.into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols2-Regular.ttf"
        ))),
    );
    // Keep Hack's text and cell metrics, using this only for missing symbols.
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push(SYMBOLS.into());
    ctx.set_fonts(fonts);
}

fn configure_appearance(ctx: &egui::Context, light: bool) {
    let mut visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(6);
    }
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.button_padding = Vec2::new(9.0, 5.0);
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    });
}

fn shortcut(action: &str, keys: &str) -> String {
    let command = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl+Alt"
    };
    format!("{action} — {command}+{keys}")
}

fn workspace_shortcut(
    input: &mut egui::InputState,
    command: Modifiers,
    count: usize,
) -> Option<usize> {
    let number = |key| {
        [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ]
        .iter()
        .position(|candidate| *candidate == key)
    };
    let mut selected = None;
    input.events.retain(|event| {
        if let egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            modifiers,
            ..
        } = event
            && modifiers.matches_logically(command)
            && let Some(index) = physical_key.and_then(number).or_else(|| number(*key))
            && index < count
        {
            selected = Some(index);
            return false;
        }
        true
    });
    selected
}

fn workspace_submission(ctx: &egui::Context) -> Option<egui::Id> {
    let focused = ctx.memory(|memory| memory.focused())?;
    if focused != egui::Id::new("workspace-directory")
        && focused != egui::Id::new("workspace-filter")
        && focused != egui::Id::new("new-workspace-name")
    {
        return None;
    }
    ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
        .then_some(focused)
}

fn dropped_file_input(files: &[egui::DroppedFile], mode: TermMode) -> anyhow::Result<Vec<u8>> {
    let mut text = String::new();
    for file in files {
        let path = file.path.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Save the screenshot as a file, then drop it onto the terminal")
        })?;
        let path = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Dropped file path is not valid UTF-8"))?;
        if path.is_empty() || path.chars().any(char::is_control) {
            anyhow::bail!("Dropped file path contains unsupported control characters");
        }
        if path.len() > 60 * 1024 {
            anyhow::bail!("Dropped file path exceeds the terminal paste limit");
        }
        if !text.is_empty() {
            text.push(' ');
        }
        if mode.contains(TermMode::BRACKETED_PASTE) {
            text.push_str("\x1b[200~");
        }
        text.push('\'');
        text.push_str(&path.replace('\'', "'\\''"));
        text.push('\'');
        if mode.contains(TermMode::BRACKETED_PASTE) {
            text.push_str("\x1b[201~");
        }
        if text.len() > 60 * 1024 {
            anyhow::bail!("Dropped file paths exceed the terminal paste limit; drop fewer files");
        }
    }
    Ok(text.into_bytes())
}

fn cursor_rect(shape: CursorShape, pos: Pos2, cell: Vec2) -> Option<Rect> {
    match shape {
        CursorShape::Beam => Some(Rect::from_min_size(
            pos,
            Vec2::new(2.0_f32.min(cell.x), cell.y),
        )),
        CursorShape::Underline => Some(Rect::from_min_size(
            pos + Vec2::new(0.0, cell.y - 2.0_f32.min(cell.y)),
            Vec2::new(cell.x, 2.0_f32.min(cell.y)),
        )),
        CursorShape::Block | CursorShape::HollowBlock => Some(Rect::from_min_size(pos, cell)),
        CursorShape::Hidden => None,
    }
}

fn paint_cursor(
    painter: &egui::Painter,
    shape: CursorShape,
    pos: Pos2,
    cell: Vec2,
    color: Color32,
    font: &FontId,
    glyph: &str,
) {
    let Some(rect) = cursor_rect(shape, pos, cell) else {
        return;
    };
    match shape {
        CursorShape::HollowBlock => {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0_f32, color),
                egui::StrokeKind::Inside,
            );
        }
        CursorShape::Block => {
            painter.rect_filled(rect, 0.0, color);
            // Repaint the glyph over an opaque cursor rather than tinting/obscuring it.
            let light = u32::from(color.r()) * 299
                + u32::from(color.g()) * 587
                + u32::from(color.b()) * 114;
            let foreground = if light >= 128_000 {
                Color32::BLACK
            } else {
                Color32::WHITE
            };
            painter.text(pos, egui::Align2::LEFT_TOP, glyph, font.clone(), foreground);
        }
        _ => {
            painter.rect_filled(rect, 0.0, color);
        }
    }
}

fn terminal_view(
    ui: &mut egui::Ui,
    terminal: &mut Terminal,
    search: &mut TerminalSearch,
    focused: bool,
    font_size: f32,
    clickable_choices: bool,
) -> (bool, anyhow::Result<()>) {
    let copying = terminal.copy_mode;
    let searching = search.is_open();
    if copying {
        ui.small(
            "Copy mode · arrows move · Shift extends · Space selects · Enter copies · Esc exits",
        );
    }
    search.show(ui, terminal, focused);
    let font = FontId::monospace(font_size);
    let cell = ui.fonts_mut(|f| Vec2::new(f.glyph_width(&font, 'M'), f.row_height(&font)));
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    if !searching && (focused || response.clicked()) {
        if !response.has_focus() {
            response.request_focus();
        }
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
    }
    let grid_rect = rect.shrink(8.0);
    let cols = ((grid_rect.width() / cell.x).floor() as usize).clamp(2, 500);
    let rows = ((grid_rect.height() / cell.y).floor() as usize).clamp(1, 200);
    terminal.resize(Size { cols, rows });
    ui.painter()
        .rect_filled(rect, 6.0, Color32::from_rgb(24, 27, 34));
    if focused {
        ui.painter().rect_stroke(
            rect,
            6.0,
            Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.5)),
            egui::StrokeKind::Inside,
        );
    }
    let mut mode = terminal.mode();
    let mut selected = None;
    let mut choice_prompt = None;
    let mut choice_revision = 0;
    if let Ok(mut term) = terminal.term.try_lock() {
        search.invalidate(terminal.revision());
        mode = *term.mode();
        let reporting =
            !copying && mode.intersects(TermMode::MOUSE_MODE) && !ui.input(|i| i.modifiers.shift);
        if !reporting && let Some(pos) = response.interact_pointer_pos() {
            let point = Point::new(
                Line(
                    ((pos.y - grid_rect.top()) / cell.y)
                        .floor()
                        .clamp(0.0, rows as f32 - 1.0) as i32
                        - term.grid().display_offset() as i32,
                ),
                Column(
                    ((pos.x - grid_rect.left()) / cell.x)
                        .floor()
                        .clamp(0.0, cols as f32 - 1.0) as usize,
                ),
            );
            if response.drag_started() {
                term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
            }
            if response.dragged()
                && let Some(s) = &mut term.selection
            {
                s.update(point, Side::Right);
            }
        }
        if clickable_choices
            && !copying
            && !searching
            && terminal.alive.load(Ordering::Acquire)
            && !mode.intersects(TermMode::MOUSE_MODE)
            && term.grid().display_offset() == 0
        {
            choice_revision = terminal.revision();
            choice_prompt = ui.ctx().data_mut(|data| {
                let cache =
                    data.get_temp_mut_or_default::<ChoiceCache>(response.id.with("choices"));
                if cache.revision != Some(choice_revision) {
                    let mut lines = vec![String::with_capacity(cols); rows];
                    for indexed in term.grid().display_iter() {
                        let row = indexed.point.line.0;
                        if row >= 0 && (row as usize) < rows {
                            lines[row as usize].push(indexed.cell.c);
                        }
                    }
                    cache.prompt = crate::choices::Prompt::parse(&lines).map(std::sync::Arc::new);
                    cache.revision = Some(choice_revision);
                }
                cache.prompt.clone()
            });
        }
        let content = term.renderable_content();
        let painter = ui.painter().with_clip_rect(grid_rect);
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + content.display_offset as i32;
            if row < 0 || row >= rows as i32 {
                continue;
            }
            let c = indexed.cell;
            let pos = grid_rect.min
                + Vec2::new(indexed.point.column.0 as f32 * cell.x, row as f32 * cell.y);
            let resolve = |c| {
                let n = match c {
                    alacritty_terminal::vte::ansi::Color::Indexed(i) => Some(i as usize),
                    alacritty_terminal::vte::ansi::Color::Named(n) => Some(n as usize),
                    _ => None,
                };
                n.and_then(|n| content.colors[n])
                    .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
                    .unwrap_or_else(|| color(c))
            };
            let (mut fg, mut bg) = (resolve(c.fg), resolve(c.bg));
            if c.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if content.selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = Color32::from_rgb(55, 85, 91);
            }
            if search.contains(indexed.point) {
                bg = Color32::from_rgb(184, 151, 64);
                fg = Color32::from_rgb(24, 27, 34);
            }
            if c.flags.contains(Flags::DIM) {
                fg = fg.gamma_multiply(0.65);
            }
            if bg != Color32::from_rgb(24, 27, 34) {
                painter.rect_filled(Rect::from_min_size(pos, cell), 0.0, bg);
            }
            if !c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::HIDDEN) && c.c != ' ' {
                let mut text = c.c.to_string();
                if let Some(z) = c.zerowidth() {
                    text.extend(z);
                }
                painter.text(pos, egui::Align2::LEFT_TOP, text, font.clone(), fg);
            }
            if c.flags.contains(Flags::UNDERLINE) {
                painter.line_segment(
                    [
                        pos + Vec2::new(0.0, cell.y - 2.0),
                        pos + Vec2::new(cell.x, cell.y - 2.0),
                    ],
                    Stroke::new(1.0_f32, fg),
                );
            }
        }
        if focused
            && !searching
            && ((copying && mode.contains(TermMode::VI))
                || (!copying
                    && !mode.contains(TermMode::VI)
                    && mode.contains(TermMode::SHOW_CURSOR)
                    && content.display_offset == 0))
        {
            let p = content.cursor.point;
            let row = p.line.0 + content.display_offset as i32;
            if row >= 0 && row < rows as i32 && p.column.0 < cols {
                let pos =
                    grid_rect.min + Vec2::new(p.column.0 as f32 * cell.x, row as f32 * cell.y);
                let cursor_cell = &term.grid()[p];
                let width = if cursor_cell.flags.contains(Flags::WIDE_CHAR) {
                    2.0
                } else {
                    1.0
                };
                let cursor_color = content.colors[NamedColor::Cursor as usize]
                    .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
                    .unwrap_or(ACCENT);
                let mut glyph = String::new();
                if !cursor_cell
                    .flags
                    .intersects(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER)
                {
                    glyph.push(cursor_cell.c);
                    if let Some(marks) = cursor_cell.zerowidth() {
                        glyph.extend(marks);
                    }
                }
                paint_cursor(
                    &painter,
                    content.cursor.shape,
                    pos,
                    Vec2::new(cell.x * width, cell.y),
                    cursor_color,
                    &font,
                    &glyph,
                );
            }
        }
        selected = term.selection_to_string();
    } else {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let reporting =
        !copying && mode.intersects(TermMode::MOUSE_MODE) && !ui.input(|i| i.modifiers.shift);
    let mut result = Ok(());
    if response.hovered()
        && let Some(pos) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|pos| grid_rect.contains(*pos))
    {
        let (delta, modifiers) = ui.input(|i| (i.raw_scroll_delta.y, i.modifiers));
        let route = scroll_route(
            mode,
            copying || modifiers.shift || !terminal.alive.load(Ordering::Acquire),
        );
        let lines = ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<ScrollState>(response.id.with("scroll"))
                .lines(delta, cell.y, route)
        });
        if lines != 0 {
            match route {
                ScrollRoute::History => result = terminal.scroll(lines),
                ScrollRoute::Mouse => {
                    let col = ((pos.x - grid_rect.left()) / cell.x).floor() as usize + 1;
                    let row = ((pos.y - grid_rect.top()) / cell.y).floor() as usize + 1;
                    let bytes = wheel_input(lines, col, row, modifiers, mode);
                    if !bytes.is_empty() {
                        result = terminal.input(bytes);
                    }
                }
                ScrollRoute::Arrows => {
                    let key = if lines > 0 {
                        Key::ArrowUp
                    } else {
                        Key::ArrowDown
                    };
                    if let Some(bytes) = encode_key(key, Modifiers::NONE, mode) {
                        result = terminal.input(bytes.repeat(lines.unsigned_abs() as usize));
                    }
                }
            }
        }
    }
    let hovered_choice = choice_prompt.as_ref().and_then(|prompt| {
        let pos = response.hover_pos()?;
        if ui.input(|i| i.modifiers != Modifiers::NONE) {
            return None;
        }
        prompt
            .choices
            .iter()
            .find(|choice| {
                Rect::from_min_max(
                    grid_rect.min
                        + Vec2::new(choice.start as f32 * cell.x, choice.row as f32 * cell.y),
                    grid_rect.min
                        + Vec2::new(choice.end as f32 * cell.x, (choice.row + 1) as f32 * cell.y),
                )
                .contains(pos)
            })
            .map(|choice| choice.number)
    });
    if hovered_choice.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let (pressed, released) =
        ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
    let was_clicked = response.clicked();
    let drag_started = response.drag_started();
    let choice_click = ui.ctx().data_mut(|data| {
        let press = data.get_temp_mut_or_default::<ChoicePress>(response.id.with("choice_press"));
        if pressed {
            press.target = hovered_choice.map(|number| (choice_revision, number));
        }
        if drag_started {
            press.target = None;
        }
        if released {
            let target = press.target.take();
            if was_clicked && target == hovered_choice.map(|number| (choice_revision, number)) {
                return hovered_choice;
            }
        }
        None
    });
    if let Some(target) = choice_click
        && let Some(prompt) = &choice_prompt
        && terminal.revision() == choice_revision
    {
        let delta = target as i32 - prompt.selected as i32;
        if delta != 0 {
            let key = if delta > 0 {
                Key::ArrowDown
            } else {
                Key::ArrowUp
            };
            if let Some(bytes) = encode_key(key, Modifiers::NONE, mode) {
                result = terminal.input_at_cursor(bytes.repeat(delta.unsigned_abs() as usize));
            }
        }
    }
    let clicked = response.clicked() || response.drag_started();
    if searching
        || (!focused && !clicked)
        || (!response.has_focus() && ui.ctx().wants_keyboard_input())
    {
        return (clicked, result);
    }
    if copying {
        let copy_result = copy_input(terminal, ui.input(|i| i.events.clone()));
        return (clicked, result.and(copy_result));
    }
    for event in ui.input(|i| i.events.clone()) {
        let follow_cursor = !matches!(event, egui::Event::PointerButton { .. });
        let input = match event {
            egui::Event::Text(s) => Some(s.into_bytes()),
            egui::Event::Paste(s) => {
                if let Err(e) = terminal.paste(&s) {
                    result = Err(e);
                }
                None
            }
            egui::Event::Copy => {
                if let Some(s) = &selected {
                    ui.ctx().copy_text(s.clone());
                }
                None
            }
            egui::Event::Ime(egui::ImeEvent::Commit(s)) => Some(s.into_bytes()),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.command && (!modifiers.ctrl || cfg!(target_os = "macos")) {
                    None
                } else {
                    encode_key(key, modifiers, mode)
                }
            }
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } if reporting && grid_rect.contains(pos) => {
                let code = match button {
                    egui::PointerButton::Primary => 0,
                    egui::PointerButton::Middle => 1,
                    egui::PointerButton::Secondary => 2,
                    _ => continue,
                };
                let col = ((pos.x - grid_rect.left()) / cell.x).floor() as usize + 1;
                let row = ((pos.y - grid_rect.top()) / cell.y).floor() as usize + 1;
                if mode.contains(TermMode::SGR_MOUSE) {
                    Some(
                        format!(
                            "\x1b[<{code};{col};{row}{}",
                            if pressed { 'M' } else { 'm' }
                        )
                        .into_bytes(),
                    )
                } else if col < 224 && row < 224 {
                    Some(vec![
                        27,
                        b'[',
                        b'M',
                        if pressed { code + 32 } else { 35 },
                        (col + 32) as u8,
                        (row + 32) as u8,
                    ])
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(bytes) = input {
            let sent = if follow_cursor {
                terminal.input_at_cursor(bytes)
            } else {
                terminal.input(bytes)
            };
            if let Err(e) = sent {
                result = Err(e);
            }
        }
    }
    (clicked, result)
}
#[derive(Clone, Default)]
struct ChoiceCache {
    revision: Option<u64>,
    prompt: Option<std::sync::Arc<crate::choices::Prompt>>,
}
#[derive(Clone, Default)]
struct ChoicePress {
    target: Option<(u64, usize)>,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ScrollRoute {
    #[default]
    History,
    Mouse,
    Arrows,
}
fn scroll_route(mode: TermMode, bypass: bool) -> ScrollRoute {
    if bypass {
        ScrollRoute::History
    } else if mode.intersects(TermMode::MOUSE_MODE) {
        ScrollRoute::Mouse
    } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        ScrollRoute::Arrows
    } else {
        ScrollRoute::History
    }
}
#[derive(Clone, Default)]
struct ScrollState {
    pending: f32,
    route: ScrollRoute,
}
impl ScrollState {
    fn lines(&mut self, delta: f32, height: f32, route: ScrollRoute) -> i32 {
        if route != self.route {
            self.pending = 0.0;
            self.route = route;
        }
        if !delta.is_finite() || !height.is_finite() || height <= 0.0 {
            return 0;
        }
        // Bound a single frame's input so extreme device deltas cannot flood the PTY.
        self.pending = (self.pending + delta / height).clamp(-128.0, 128.0);
        let lines = self.pending.trunc() as i32;
        self.pending -= lines as f32;
        lines
    }
}
fn wheel_input(
    lines: i32,
    col: usize,
    row: usize,
    modifiers: Modifiers,
    mode: TermMode,
) -> Vec<u8> {
    let code = if lines > 0 { 64 } else { 65 }
        + if modifiers.shift { 4 } else { 0 }
        + if modifiers.alt { 8 } else { 0 }
        + if modifiers.ctrl { 16 } else { 0 };
    let event = if mode.contains(TermMode::SGR_MOUSE) {
        format!("\x1b[<{code};{col};{row}M").into_bytes()
    } else if col < 224 && row < 224 {
        vec![
            27,
            b'[',
            b'M',
            code as u8 + 32,
            (col + 32) as u8,
            (row + 32) as u8,
        ]
    } else {
        return Vec::new();
    };
    event.repeat(lines.unsigned_abs() as usize)
}

fn copy_input(terminal: &mut Terminal, events: Vec<egui::Event>) -> anyhow::Result<()> {
    for event in events {
        let action = match event {
            egui::Event::Copy => Some(SelectionAction::Copy { exit: false }),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => key_action(key, modifiers),
            _ => None,
        };
        if let Some(action) = action {
            terminal.selection_action(action)?;
            if matches!(
                action,
                SelectionAction::Exit | SelectionAction::Copy { exit: true }
            ) {
                break;
            }
        }
    }
    Ok(())
}

fn encode_key(key: Key, m: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    if m.ctrl {
        let c = match key {
            Key::A => 1,
            Key::B => 2,
            Key::C => 3,
            Key::D => 4,
            Key::E => 5,
            Key::F => 6,
            Key::G => 7,
            Key::H => 8,
            Key::I => 9,
            Key::J => 10,
            Key::K => 11,
            Key::L => 12,
            Key::M => 13,
            Key::N => 14,
            Key::O => 15,
            Key::P => 16,
            Key::Q => 17,
            Key::R => 18,
            Key::S => 19,
            Key::T => 20,
            Key::U => 21,
            Key::V => 22,
            Key::W => 23,
            Key::X => 24,
            Key::Y => 25,
            Key::Z => 26,
            Key::Space => 0,
            Key::OpenBracket => 27,
            Key::Backslash => 28,
            Key::CloseBracket => 29,
            _ => 255,
        };
        if c != 255 {
            return Some(vec![c]);
        }
    }
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    let sequence = match key {
        Key::Enter => "\r".into(),
        Key::Backspace => "\x7f".into(),
        Key::Escape => "\x1b".into(),
        Key::Tab => if m.shift { "\x1b[Z" } else { "\t" }.into(),
        Key::ArrowUp | Key::ArrowDown | Key::ArrowRight | Key::ArrowLeft | Key::Home | Key::End => {
            let c = match key {
                Key::ArrowUp => 'A',
                Key::ArrowDown => 'B',
                Key::ArrowRight => 'C',
                Key::ArrowLeft => 'D',
                Key::Home => 'H',
                _ => 'F',
            };
            if modifier > 1 {
                format!("\x1b[1;{modifier}{c}")
            } else {
                format!(
                    "\x1b{}{c}",
                    if mode.contains(TermMode::APP_CURSOR) {
                        'O'
                    } else {
                        '['
                    }
                )
            }
        }
        Key::Insert | Key::Delete | Key::PageUp | Key::PageDown => {
            let n = match key {
                Key::Insert => 2,
                Key::Delete => 3,
                Key::PageUp => 5,
                _ => 6,
            };
            if modifier > 1 {
                format!("\x1b[{n};{modifier}~")
            } else {
                format!("\x1b[{n}~")
            }
        }
        Key::F1 | Key::F2 | Key::F3 | Key::F4 => format!(
            "\x1bO{}",
            match key {
                Key::F1 => 'P',
                Key::F2 => 'Q',
                Key::F3 => 'R',
                _ => 'S',
            }
        ),
        _ => return None,
    };
    Some(if m.alt && matches!(key, Key::Enter | Key::Backspace) {
        format!("\x1b{sequence}").into_bytes()
    } else {
        sequence.into_bytes()
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layouts_preserve_pane_identity_when_splitting_and_removing() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let mut l = Layout::Pane(a);
        l.split(a, b, true);
        l.split(b, c, false);
        assert!(l.remove(b));
        let mut ids = Vec::new();
        l.ids(&mut ids);
        assert_eq!(ids, vec![a, c]);
    }
    #[test]
    fn trackpad_scroll_accumulates_small_deltas_without_inventing_lines() {
        let mut scroll = ScrollState::default();
        for _ in 0..3 {
            assert_eq!(scroll.lines(4.0, 20.0, ScrollRoute::History), 0);
        }
        assert_eq!(scroll.lines(8.0, 20.0, ScrollRoute::History), 1);
        assert_eq!(scroll.lines(-10.0, 20.0, ScrollRoute::History), 0);
        assert_eq!(scroll.lines(-10.0, 20.0, ScrollRoute::History), -1);
        assert_eq!(scroll.lines(10.0, 20.0, ScrollRoute::History), 0);
        assert_eq!(
            scroll.lines(10.0, 20.0, ScrollRoute::Mouse),
            0,
            "remainder crossed input mode boundary"
        );
        assert_eq!(scroll.lines(f32::NAN, 20.0, ScrollRoute::Mouse), 0);
        assert_eq!(scroll.lines(f32::INFINITY, 20.0, ScrollRoute::Mouse), 0);
        assert_eq!(scroll.lines(20.0, 0.0, ScrollRoute::Mouse), 0);
        assert_eq!(scroll.lines(100_000.0, 20.0, ScrollRoute::Mouse), 128);
    }

    #[test]
    fn scrolling_routes_mouse_alternate_screen_and_native_history() {
        let alt = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert!(matches!(
            scroll_route(TermMode::NONE, false),
            ScrollRoute::History
        ));
        assert!(matches!(scroll_route(alt, false), ScrollRoute::Arrows));
        assert!(matches!(
            scroll_route(TermMode::ALT_SCREEN, false),
            ScrollRoute::History
        ));
        assert!(matches!(
            scroll_route(alt | TermMode::MOUSE_REPORT_CLICK, false),
            ScrollRoute::Mouse
        ));
        assert!(matches!(
            scroll_route(alt | TermMode::MOUSE_REPORT_CLICK, true),
            ScrollRoute::History
        ));
        assert_eq!(
            wheel_input(2, 10, 5, Modifiers::NONE, TermMode::SGR_MOUSE),
            b"\x1b[<64;10;5M\x1b[<64;10;5M"
        );
        assert_eq!(
            wheel_input(
                -1,
                2,
                3,
                Modifiers::CTRL | Modifiers::ALT,
                TermMode::SGR_MOUSE
            ),
            b"\x1b[<89;2;3M"
        );
        assert_eq!(
            wheel_input(-1, 2, 3, Modifiers::NONE, TermMode::NONE),
            vec![27, b'[', b'M', 97, 34, 35]
        );
        assert!(wheel_input(1, 224, 3, Modifiers::NONE, TermMode::NONE).is_empty());
        assert_eq!(
            wheel_input(1, 300, 3, Modifiers::NONE, TermMode::SGR_MOUSE),
            b"\x1b[<64;300;3M"
        );
    }

    #[test]
    fn terminal_keys_respect_application_cursor_and_ctrl() {
        assert_eq!(
            encode_key(Key::ArrowUp, Modifiers::NONE, TermMode::APP_CURSOR).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(Key::C, Modifiers::CTRL, TermMode::NONE).unwrap(),
            vec![3]
        );
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::model::HookEvent;
    use serde_json::json;
    fn start_fixture_terminal(app: &mut App, ctx: &egui::Context) {
        let workspace = &app.saved.workspaces[0];
        let pane = workspace.focus;
        let directory = workspace.task.directory.clone();
        assert!(app.spawn(pane, ctx, &directory));
    }

    #[test]
    fn terminal_font_covers_prompt_cross_and_preserves_cell_metrics() {
        let ctx = egui::Context::default();
        let font = FontId::monospace(15.0);
        let mut original_metrics = Vec2::ZERO;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            original_metrics = ctx.fonts_mut(|fonts| {
                Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font))
            });
        });
        configure_terminal_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            ctx.fonts_mut(|fonts| {
                for symbol in ['➜', '✗', '✘', '✓', '✔'] {
                    assert!(
                        fonts.has_glyph(&font, symbol),
                        "missing prompt symbol {symbol}"
                    );
                }
                assert!(fonts.has_glyphs(&font, "➜  tessera git:(main) ✗ gs"));
                assert_eq!(
                    Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font)),
                    original_metrics,
                );
            });
        });
    }

    #[test]
    fn startup_opens_one_fresh_terminal_and_preserves_preferences_and_history() {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        for count in [0, 3] {
            let mut app = fixture(count);
            let previous_panes: Vec<_> = app.saved.workspaces.iter().map(|w| w.focus).collect();
            if count > 0 {
                let extra = Uuid::new_v4();
                app.saved.workspaces[0]
                    .layout
                    .split(previous_panes[0], extra, true);
                app.saved.sessions[0].state = SessionState::Ended;
            }
            app.saved.light = true;
            app.saved.font_size = 18.0;
            app.saved.skip_stop_confirmation = true;
            app.saved.clickable_codex_choices = true;
            // Exercise loading the legacy state format, including missing directories.
            app.saved = serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
            let history: Vec<_> = app
                .saved
                .sessions
                .iter()
                .map(|s| (s.key(), s.history.len()))
                .collect();
            app.new_directory = directory.path().to_string_lossy().into_owned();
            app.start_fresh_workspace(&ctx);

            assert_eq!(app.saved.workspaces.len(), 1);
            assert_eq!(app.panes.len(), 1);
            assert_eq!(app.active, 0);
            assert!(!app.overview);
            assert!(app.dirty);
            let workspace = &app.saved.workspaces[0];
            assert!(matches!(workspace.layout, Layout::Pane(id) if id == workspace.focus));
            assert!(!previous_panes.contains(&workspace.focus));
            assert_eq!(workspace.task.directory, app.new_directory);
            let terminal = &app.panes[&workspace.focus].terminal;
            assert!(terminal.alive.load(Ordering::Acquire));
            assert_eq!(
                terminal.current_directory().unwrap(),
                directory.path().canonicalize().unwrap()
            );
            assert!(app.saved.light);
            assert_eq!(app.saved.font_size, 18.0);
            assert!(app.saved.skip_stop_confirmation);
            assert!(app.saved.clickable_codex_choices);
            assert_eq!(
                history,
                app.saved
                    .sessions
                    .iter()
                    .map(|s| (s.key(), s.history.len()))
                    .collect::<Vec<_>>()
            );
            for (index, session) in app.saved.sessions.iter().enumerate() {
                assert_eq!(
                    session.state,
                    if index == 0 {
                        SessionState::Ended
                    } else {
                        SessionState::Disconnected
                    }
                );
            }
        }
    }

    #[test]
    fn startup_failure_discards_old_layout_and_reports_the_error() {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = fixture(3);
        app.new_directory = directory
            .path()
            .join("missing")
            .to_string_lossy()
            .into_owned();
        app.start_fresh_workspace(&ctx);
        assert!(app.saved.workspaces.is_empty());
        assert!(app.panes.is_empty());
        assert!(!app.overview);
        assert!(app.dirty);
        assert!(app.error.contains("Working directory does not exist"));
        assert!(
            app.saved
                .sessions
                .iter()
                .all(|s| s.state == SessionState::Disconnected)
        );
        app.new_directory = directory.path().to_string_lossy().into_owned();
        assert!(app.add_workspace(&ctx));
        assert_eq!(app.saved.workspaces.len(), 1);
        assert_eq!(app.panes.len(), 1);
    }

    fn fixture(count: usize) -> App {
        let (writer, _) = sync_channel(1);
        let mut saved = Saved::default();
        for n in 0..count {
            let pane = Uuid::new_v4();
            saved.workspaces.push(Workspace {
                task: Task {
                    id: Uuid::new_v4(),
                    title: [
                        "Tessera · terminal grid",
                        "Mélimo · audio queue",
                        "PRCtrl · review navigation",
                    ][n % 3]
                        .into(),
                    directory: format!("~/Projects/{}", ["tessera", "melimo", "prctrl"][n % 3]),
                    state: TaskState::Implementing,
                },
                layout: Layout::Pane(pane),
                focus: pane,
            });
            let mut s = Session::new(pane, format!("fixture-{n}"));
            if n % 2 != 0 {
                s.agent = crate::model::Agent::Codex;
            }
            for seq in 1..4 {
                s.apply(
                    HookEvent {
                        id: Uuid::new_v4(),
                        pane,
                        sequence: seq,
                        session_id: format!("fixture-{n}"),
                        agent: if n % 2 == 0 {
                            crate::model::Agent::Claude
                        } else {
                            crate::model::Agent::Codex
                        },
                        hook_event_name: if n % 3 == 0 {
                            "PermissionRequest"
                        } else if n % 3 == 1 {
                            "PreToolUse"
                        } else {
                            "Stop"
                        }
                        .into(),
                        detail: if n % 3 == 0 {
                            "Bash"
                        } else if n % 3 == 1 {
                            "Edit"
                        } else {
                            ""
                        }
                        .into(),
                    },
                    now(),
                );
            }
            saved.sessions.push(s);
        }
        let selected = saved.sessions.first().map(Session::key);
        App {
            updater: crate::updater::Updater::default(),
            updater_error: None,
            saved,
            panes: HashMap::new(),
            active: 0,
            overview: true,
            selected,
            endpoint: None,
            error: String::new(),
            writer,
            persistence: None,
            save_error: std::sync::Arc::new(std::sync::Mutex::new(None)),
            dirty: false,
            last_save: 0,
            font_size: 15.0,
            new_directory: String::new(),
            new_workspace_name: String::new(),
            palette: false,
            rename: None,
            closing: None,
            filter: String::new(),
            maximized: false,
            category: 0,
        }
    }
    fn export(ctx: &egui::Context, out: egui::FullOutput, path: &std::path::Path) {
        let textures:Vec<_>=out.textures_delta.set.iter().map(|(id,delta)| {
            let egui::ImageData::Color(image)=&delta.image;
            json!({"id":format!("{id:?}"),"size":image.size,"pos":delta.pos,"pixels":image.pixels.iter().map(|c|c.to_array()).collect::<Vec<_>>()})
        }).collect();
        let meshes:Vec<_>=ctx.tessellate(out.shapes,out.pixels_per_point).into_iter().filter_map(|p| {
            let egui::epaint::Primitive::Mesh(m)=p.primitive else{return None;};
            Some(json!({"clip":[p.clip_rect.min.x,p.clip_rect.min.y,p.clip_rect.max.x,p.clip_rect.max.y],"texture":format!("{:?}",m.texture_id),"indices":m.indices,"vertices":m.vertices.iter().map(|v|json!([v.pos.x,v.pos.y,v.uv.x,v.uv.y,v.color.to_array()])).collect::<Vec<_>>()}))
        }).collect();
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"textures":textures,"meshes":meshes})).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn overview_renders_both_themes_and_narrow_windows() {
        for (name, width, height, light) in [
            ("dark", 1180.0, 760.0, false),
            ("light", 1180.0, 760.0, true),
            ("narrow", 640.0, 700.0, false),
        ] {
            let ctx = egui::Context::default();
            configure_appearance(&ctx, light);
            let mut app = fixture(6);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height))),
                ..Default::default()
            };
            app.saved.light = light;
            let first = ctx.run(input.clone(), |ctx| app.draw(ctx));
            let mut out = ctx.run(input, |ctx| app.draw(ctx));
            let mut textures = first.textures_delta;
            textures.append(out.textures_delta);
            out.textures_delta = textures;
            assert!(!out.shapes.is_empty());
            if let Ok(dir) = std::env::var("TESSERA_RENDER_DIR") {
                let path = PathBuf::from(dir);
                std::fs::create_dir_all(&path).unwrap();
                export(&ctx, out, &path.join(format!("{name}.json")));
            }
        }
    }
    fn command() -> Modifiers {
        if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL | Modifiers::ALT
        }
    }

    fn key_event(key: Key, physical_key: Option<Key>, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn custom_workspace_names_and_layout_keys_do_not_break_number_shortcuts() {
        let ctx = egui::Context::default();
        let mut app = fixture(5);
        app.saved.workspaces[3].task.title = "Custom workspace".into();
        app.palette = true;
        app.maximized = true;
        let _ = ctx.run(
            egui::RawInput {
                modifiers: command(),
                events: vec![key_event(Key::Quote, Some(Key::Num4), command())],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert_eq!(app.active, 3);
        assert!(!app.overview && !app.palette && !app.maximized);
        assert_eq!(
            app.saved.workspaces[app.active].task.title,
            "Custom workspace"
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::Num2, None, command() | Modifiers::SHIFT)],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert_eq!(app.active, 1);
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    key_event(Key::Num3, Some(Key::Num3), Modifiers::NONE),
                    key_event(Key::Num9, None, command()),
                ],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert_eq!(
            app.active, 1,
            "ordinary typing or unavailable workspace changed focus"
        );
    }

    #[test]
    fn rename_shortcut_selects_name_and_escape_cancels() {
        let ctx = egui::Context::default();
        let mut app = fixture(2);
        app.active = 1;
        app.palette = true;
        let original = app.saved.workspaces[1].task.title.clone();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::R, None, command() | Modifiers::SHIFT)],
                ..Default::default()
            },
            |ctx| {
                app.shortcuts(ctx);
                assert!(!ctx.input(|i| i.key_pressed(Key::R)));
                app.rename_dialog(ctx);
            },
        );
        assert!(!app.palette);
        assert_eq!(app.rename.as_ref().unwrap().title, original);
        let state = egui::TextEdit::load_state(&ctx, egui::Id::new("workspace-name")).unwrap();
        let range = state.cursor.char_range().unwrap();
        assert_eq!(range.sorted_cursors()[0].index, 0);
        assert_eq!(range.sorted_cursors()[1].index, original.chars().count());
        app.rename.as_mut().unwrap().title = "Cancelled".into();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::Escape, None, Modifiers::NONE)],
                ..Default::default()
            },
            |ctx| app.rename_dialog(ctx),
        );
        assert!(app.rename.is_none());
        assert_eq!(app.saved.workspaces[1].task.title, original);
        assert!(!app.dirty);
    }

    #[test]
    fn dropped_paths_are_quoted_and_each_image_gets_its_own_paste() {
        let files =
            ["/tmp/Screen shot é.png", "/tmp/a'$(touch nope).png"].map(|path| egui::DroppedFile {
                path: Some(PathBuf::from(path)),
                ..Default::default()
            });
        let plain = "'/tmp/Screen shot é.png' '/tmp/a'\\''$(touch nope).png'";
        assert_eq!(
            dropped_file_input(&files, TermMode::empty()).unwrap(),
            plain.as_bytes()
        );
        assert_eq!(dropped_file_input(&files, TermMode::BRACKETED_PASTE).unwrap(),
            "\x1b[200~'/tmp/Screen shot é.png'\x1b[201~ \x1b[200~'/tmp/a'\\''$(touch nope).png'\x1b[201~".as_bytes());
        for path in ["/tmp/bad\nname.png", "/tmp/bad\x1bname.png", ""] {
            assert!(
                dropped_file_input(
                    &[egui::DroppedFile {
                        path: Some(path.into()),
                        ..Default::default()
                    }],
                    TermMode::empty()
                )
                .is_err()
            );
        }
        assert!(
            dropped_file_input(&[egui::DroppedFile::default()], TermMode::empty())
                .unwrap_err()
                .to_string()
                .contains("Save the screenshot")
        );
        assert!(
            dropped_file_input(
                &[egui::DroppedFile {
                    path: Some("x".repeat(64 * 1024).into()),
                    ..Default::default()
                }],
                TermMode::empty()
            )
            .is_err()
        );
    }

    #[test]
    fn file_drop_targets_the_hovered_split_and_reaches_pty_without_enter() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.overview = false;
        start_fixture_terminal(&mut app, &ctx);
        let target = app.saved.workspaces[0].focus;
        app.split(&ctx, true);
        let other = app.saved.workspaces[0].focus;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("received");
        let ready = directory.path().join("ready");
        let files = vec![egui::DroppedFile {
            path: Some("/tmp/Screenshot é '1.png".into()),
            ..Default::default()
        }];
        let expected = dropped_file_input(&files, TermMode::BRACKETED_PASTE).unwrap();
        app.panes[&target].terminal.input(format!(
            "printf '\\033[?2004h'; stty raw -echo; touch '{}'; dd bs=1 count={} of='{}' 2>/dev/null\r",
            ready.display(), expected.len(), output.display()
        ).into_bytes()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready.exists()
            || !app.panes[&target]
                .terminal
                .mode()
                .contains(TermMode::BRACKETED_PASTE)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "PTY did not become ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let frame = |app: &mut App, dropped_files| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                    events: vec![egui::Event::PointerMoved(Pos2::new(150.0, 250.0))],
                    dropped_files,
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        };
        frame(&mut app, vec![]);
        app.panes.get_mut(&target).unwrap().search.open();
        frame(&mut app, files.clone());
        assert!(app.error.contains("Close Find"));
        assert_eq!(app.saved.workspaces[0].focus, other);
        app.panes.get_mut(&target).unwrap().search.close();
        frame(&mut app, files);
        assert_eq!(app.saved.workspaces[0].focus, target);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !std::fs::read(&output).is_ok_and(|bytes| bytes.len() == expected.len()) {
            assert!(
                std::time::Instant::now() < deadline,
                "Drop did not reach target PTY"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read(output).unwrap(), expected);
        assert!(!expected.contains(&b'\r') && !expected.contains(&b'\n'));
    }

    #[test]
    fn workspace_commands_dismisses_with_escape_or_backdrop_and_keeps_inside_clicks() {
        for light in [false, true] {
            let ctx = egui::Context::default();
            configure_appearance(&ctx, light);
            let mut app = fixture(0);
            let frame = |app: &mut App, events| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.draw(ctx),
                )
            };
            let click = |pos| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    },
                ]
            };
            let output = frame(&mut app, vec![]);
            let opener = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == "+ Workspace" => {
                        Some(text.pos + Vec2::new(10.0, 5.0))
                    }
                    _ => None,
                })
                .unwrap();
            frame(&mut app, click(opener));
            assert!(app.palette, "opening click dismissed the panel");
            let mut output = frame(&mut app, vec![]);
            for _ in 0..2 {
                output = frame(&mut app, vec![]);
            }
            let heading = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text)
                        if text.galley.text() == "Workspace & commands" =>
                    {
                        Some(text.pos + Vec2::new(10.0, 5.0))
                    }
                    _ => None,
                })
                .unwrap();
            frame(&mut app, click(heading));
            assert!(app.palette, "inside click dismissed the panel");
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("new-workspace-name")));
            frame(
                &mut app,
                vec![key_event(Key::Escape, None, Modifiers::NONE)],
            );
            assert!(!app.palette);
            assert!(app.overview, "Escape escaped through to Overview");
            app.palette = true;
            for _ in 0..3 {
                frame(&mut app, vec![]);
            }
            frame(&mut app, click(opener));
            assert!(!app.palette, "outside click did not dismiss the panel");
            assert!(app.saved.workspaces.is_empty());
        }
    }

    #[test]
    fn updater_failure_remains_visible_after_other_errors_change() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.palette = true;
        app.updater_error =
            Some("Updates unavailable: Cannot load Sparkle; reinstall Tessera".into());
        app.error = "Unrelated terminal error".into();
        let mut output = None;
        for _ in 0..3 {
            output = Some(ctx.run(egui::RawInput::default(), |ctx| app.draw(ctx)));
        }
        let output = output.unwrap();
        for expected in ["Check for updates…", app.updater_error.as_ref().unwrap()] {
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text() == expected)
            }), "Missing updater diagnostic: {expected}");
        }
    }

    #[test]
    fn workspace_dialogs_fit_narrow_windows_in_both_themes() {
        for light in [false, true] {
            for rename in [false, true] {
                let ctx = egui::Context::default();
                configure_appearance(&ctx, light);
                let mut app = fixture(1);
                if rename {
                    app.begin_rename();
                } else {
                    app.request_stop_terminal(app.saved.workspaces[0].focus);
                }
                let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(480.0, 400.0));
                for _ in 0..3 {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(screen),
                            ..Default::default()
                        },
                        |ctx| {
                            app.rename_dialog(ctx);
                            app.close_dialog(ctx);
                        },
                    );
                }
                let id = egui::Id::new(if rename {
                    "rename-workspace"
                } else {
                    "stop-terminal"
                });
                let rect = ctx.memory(|memory| memory.area_rect(id)).unwrap();
                assert!(screen.contains_rect(rect), "Dialog overflows: {rect:?}");
            }
        }
    }

    #[test]
    fn rename_dialog_enter_saves_typed_name() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.begin_rename();
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.draw(ctx));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("New workspace name".into())],
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        );
        assert_eq!(app.rename.as_ref().unwrap().title, "New workspace name");
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::Enter, None, Modifiers::NONE)],
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        );
        assert!(app.rename.is_none());
        assert_eq!(app.saved.workspaces[0].task.title, "New workspace name");
    }

    #[test]
    fn rename_rejects_blank_names_and_saves_original_workspace() {
        let mut app = fixture(2);
        app.begin_rename();
        app.rename.as_mut().unwrap().title = "  ".into();
        app.finish_rename();
        assert!(app.rename.is_some());
        assert!(!app.dirty);
        app.active = 1;
        app.rename.as_mut().unwrap().title = "  Custom name  ".into();
        app.finish_rename();
        assert_eq!(app.saved.workspaces[0].task.title, "Custom name");
        assert!(app.rename.is_none());
        assert!(app.dirty);
        let saved: Saved =
            serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
        assert_eq!(saved.workspaces[0].task.title, "Custom name");
        app.saved.workspaces.clear();
        app.begin_rename();
        assert!(app.rename.is_none());
    }

    #[test]
    fn clicking_codex_choice_sends_arrow_without_submitting() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choice-input");
        let mut terminal = Terminal::spawn(
            Uuid::new_v4(),
            dir.path(),
            std::path::Path::new(""),
            ctx.clone(),
        )
        .unwrap();
        let mut search = TerminalSearch::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 480.0));
        let mut pos = Pos2::ZERO;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let font = FontId::monospace(15.0);
                    let cell =
                        ui.fonts_mut(|f| Vec2::new(f.glyph_width(&font, 'M'), f.row_height(&font)));
                    pos = ui.available_rect_before_wrap().shrink(8.0).min
                        + Vec2::new(6.0 * cell.x, 3.5 * cell.y);
                    terminal_view(ui, &mut terminal, &mut search, true, 15.0, true)
                        .1
                        .unwrap();
                });
            },
        );
        terminal.input(format!("stty raw -echo; printf '\\033[?1049h\\033[2J\\033[HQuestion 1/1 (1 unanswered)\\r\\nChoose an option.\\r\\n › 1. First\\r\\n   2. Second\\r\\n\\r\\ntab to add notes | enter to submit answer | esc to interrupt'; dd bs=1 count=3 of='{}' 2>/dev/null; stty sane\r", path.display()).into_bytes()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !terminal.mode().contains(TermMode::ALT_SCREEN) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        terminal_view(ui, &mut terminal, &mut search, true, 15.0, true)
                            .1
                            .unwrap();
                    });
                },
            );
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !std::fs::read(&path).is_ok_and(|bytes| bytes.len() == 3) {
            assert!(
                std::time::Instant::now() < deadline,
                "click did not select through PTY"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read(path).unwrap(), b"\x1b[B");
    }

    #[test]
    fn trackpad_gesture_reaches_mouse_application_through_pty() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wheel-input");
        let mut terminal = Terminal::spawn(
            Uuid::new_v4(),
            dir.path(),
            std::path::Path::new(""),
            ctx.clone(),
        )
        .unwrap();
        let mut search = TerminalSearch::default();
        let pos = Pos2::new(80.0, 80.0);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0));
        let mut expected = Vec::new();
        let mut height = 0.0;
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(pos)],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let font = FontId::monospace(15.0);
                        let cell = ui.fonts_mut(|fonts| {
                            Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font))
                        });
                        height = cell.y;
                        let grid = ui.available_rect_before_wrap().shrink(8.0);
                        let col = ((pos.x - grid.left()) / cell.x).floor() as usize + 1;
                        let row = ((pos.y - grid.top()) / cell.y).floor() as usize + 1;
                        expected = wheel_input(1, col, row, Modifiers::NONE, TermMode::SGR_MOUSE);
                        terminal_view(ui, &mut terminal, &mut search, true, 15.0, false)
                            .1
                            .unwrap();
                    });
                },
            );
        }
        terminal.input(format!("stty raw -echo; printf '\\033[?1049h\\033[?1000h\\033[?1006h'; dd bs=1 count={} of='{}' 2>/dev/null; stty sane\r", expected.len(), path.display()).into_bytes()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !terminal
            .mode()
            .contains(TermMode::ALT_SCREEN | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "mouse application did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        for _ in 0..4 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: Vec2::new(0.0, height / 4.0),
                            modifiers: Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        terminal_view(ui, &mut terminal, &mut search, true, 15.0, false)
                            .1
                            .unwrap();
                    });
                },
            );
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !std::fs::read(&path).is_ok_and(|bytes| bytes.len() == expected.len()) {
            assert!(
                std::time::Instant::now() < deadline,
                "trackpad wheel input did not reach PTY"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read(path).unwrap(), expected);
    }

    #[test]
    fn command_w_requests_confirmation_and_saved_opt_out_closes_immediately() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        let pane = app.saved.workspaces[0].focus;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::W, None, command())],
                ..Default::default()
            },
            |ctx| {
                app.shortcuts(ctx);
                assert!(!ctx.input(|i| i.key_pressed(Key::W)));
            },
        );
        assert_eq!(app.closing.as_ref().unwrap().pane, pane);
        assert_eq!(app.saved.workspaces.len(), 1);
        app.closing.as_mut().unwrap().skip_confirmation = true;
        app.confirm_stop_terminal();
        assert!(app.saved.skip_stop_confirmation);
        assert!(app.saved.workspaces.is_empty());
        let saved: Saved =
            serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
        assert!(saved.skip_stop_confirmation);
        let mut next = fixture(1);
        next.saved.skip_stop_confirmation = saved.skip_stop_confirmation;
        next.request_stop_terminal(next.saved.workspaces[0].focus);
        assert!(next.closing.is_none());
        assert!(next.saved.workspaces.is_empty());
        assert_eq!(app.saved.sessions[0].pane, pane);
        assert_eq!(app.saved.sessions[0].state, SessionState::Disconnected);
    }

    #[test]
    fn cancel_terminal_close_does_not_save_checkbox_or_stop_terminal() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.request_stop_terminal(app.saved.workspaces[0].focus);
        app.closing.as_mut().unwrap().skip_confirmation = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.close_dialog(ctx));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::Escape, None, Modifiers::NONE)],
                ..Default::default()
            },
            |ctx| app.close_dialog(ctx),
        );
        assert!(app.closing.is_none());
        assert_eq!(app.saved.workspaces.len(), 1);
        assert!(!app.saved.skip_stop_confirmation);
        assert!(!app.dirty);
        let mut old = serde_json::to_value(&app.saved).unwrap();
        old.as_object_mut()
            .unwrap()
            .remove("skip_stop_confirmation");
        let restored: Saved = serde_json::from_value(old).unwrap();
        assert!(!restored.skip_stop_confirmation);
    }

    #[test]
    fn both_split_directions_inherit_the_focused_shell_directory_after_cd() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested folder α");
        std::fs::create_dir(&nested).unwrap();
        let nested = nested.canonicalize().unwrap();
        let mut app = fixture(1);
        app.saved.workspaces[0].task.directory = dir.path().to_string_lossy().into_owned();
        start_fixture_terminal(&mut app, &ctx);
        // Keep a second pane at the original path to verify focus determines the source.
        app.split(&ctx, true);
        for vertical in [true, false] {
            let source = &app.panes[&app.saved.workspaces[0].focus].terminal;
            let quoted = nested.to_string_lossy().replace('\'', "'\\''");
            source
                .input(format!("cd -- '{quoted}'\r").into_bytes())
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !source.current_directory().is_ok_and(|path| path == nested) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "source shell did not change directory"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            app.split(&ctx, vertical);
            let new = &app.panes[&app.saved.workspaces[0].focus].terminal;
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !new.current_directory().is_ok_and(|path| path == nested) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "split shell started in wrong directory"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert_eq!(app.panes.len(), 4);
        assert_eq!(app.saved.workspaces.len(), 1);
    }

    #[test]
    fn command_n_inherits_focused_shell_directory_after_cd() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested folder α");
        std::fs::create_dir(&nested).unwrap();
        let nested = nested.canonicalize().unwrap();
        let mut app = fixture(1);
        app.saved.workspaces[0].task.directory = dir.path().to_string_lossy().into_owned();
        start_fixture_terminal(&mut app, &ctx);
        let original = app.saved.workspaces[0].focus;
        app.split(&ctx, true);
        let focused = app.saved.workspaces[0].focus;
        assert_ne!(original, focused);
        let source = &app.panes[&focused].terminal;
        let quoted = nested.to_string_lossy().replace('\'', "'\\''");
        source
            .input(format!("cd -- '{quoted}'\r").into_bytes())
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !source.current_directory().is_ok_and(|path| path == nested) {
            assert!(
                std::time::Instant::now() < deadline,
                "shell did not change directory"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        app.new_directory = dir.path().join("unrelated").to_string_lossy().into_owned();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::N, None, command())],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert_eq!(app.saved.workspaces.len(), 2);
        assert_eq!(app.active, 1);
        assert_eq!(
            PathBuf::from(&app.saved.workspaces[1].task.directory),
            nested
        );
        assert_eq!(app.saved.workspaces[0].focus, focused);
        let new = &app.panes[&app.saved.workspaces[1].focus].terminal;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !new.current_directory().is_ok_and(|path| path == nested) {
            assert!(
                std::time::Instant::now() < deadline,
                "new shell started in wrong directory"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.dirty);
    }

    #[test]
    fn ctrl_d_closes_exited_panes_and_last_workspace_without_confirmation() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(1);
        app.saved.workspaces[0].task.directory = dir.path().to_string_lossy().into_owned();
        start_fixture_terminal(&mut app, &ctx);
        let original = app.saved.workspaces[0].focus;
        app.split(&ctx, true);
        let split = app.saved.workspaces[0].focus;
        assert!(app.add_workspace_in(&ctx, dir.path().to_string_lossy().into_owned()));
        let surviving_task = app.saved.workspaces[1].task.id;
        let surviving_pane = app.saved.workspaces[1].focus;
        app.active = 0;
        app.maximized = true;
        // A stale stop confirmation must also disappear when its process exits.
        app.request_stop_terminal(split);
        assert!(app.closing.is_some());
        for (index, pane) in [split, original].into_iter().enumerate() {
            let ready = dir.path().join(format!("ready-{index}"));
            app.panes[&pane]
                .terminal
                .input(format!("printf ready > '{}'\r", ready.display()).into_bytes())
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !ready.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "shell did not become ready"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            app.panes[&pane].terminal.input(vec![4]).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                app.drain();
                if !app.panes.contains_key(&pane) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Ctrl+D did not close the pane"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(app.closing.is_none());
            assert!(!app.saved.skip_stop_confirmation);
            assert!(!app.maximized);
            if index == 0 {
                assert_eq!(app.saved.workspaces.len(), 2);
                assert!(
                    matches!(app.saved.workspaces[0].layout, Layout::Pane(id) if id == original)
                );
                assert_eq!(app.saved.workspaces[0].focus, original);
                app.begin_rename();
            }
        }
        assert_eq!(app.saved.workspaces.len(), 1);
        assert_eq!(app.panes.len(), 1);
        assert_eq!(app.saved.workspaces[app.active].task.id, surviving_task);
        assert!(
            app.panes[&surviving_pane]
                .terminal
                .alive
                .load(Ordering::Acquire)
        );
        assert_eq!(app.saved.sessions.len(), 1);
        assert_eq!(app.saved.sessions[0].state, SessionState::Disconnected);
        assert!(app.rename.is_none());
        assert!(app.dirty);
        app.drain();
        assert_eq!(app.saved.workspaces.len(), 1);
    }

    #[test]
    fn stopping_last_terminal_removes_workspace_and_stops_shell() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.saved.workspaces[0].task.directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        start_fixture_terminal(&mut app, &ctx);
        let pane = app.saved.workspaces[0].focus;
        let alive = app.panes[&pane].terminal.alive.clone();
        app.maximized = true;
        app.stop_terminal(pane);
        assert!(!alive.load(Ordering::Acquire));
        assert!(app.panes.is_empty());
        assert!(app.saved.workspaces.is_empty());
        assert_eq!(app.active, 0);
        assert!(!app.maximized);
        assert!(app.dirty);
        assert_eq!(app.saved.sessions.len(), 1);
        assert_eq!(app.saved.sessions[0].state, SessionState::Disconnected);
        let restored: Saved =
            serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
        assert!(restored.workspaces.is_empty());
        app.stop_terminal(pane); // Repeated close is harmless.
    }

    #[test]
    fn stopping_workspace_preserves_active_tab_identity() {
        let mut app = fixture(3);
        app.active = 1;
        let active_task = app.saved.workspaces[1].task.id;
        app.stop_terminal(app.saved.workspaces[0].focus);
        assert_eq!(app.active, 0);
        assert_eq!(app.saved.workspaces[app.active].task.id, active_task);
        app.active = 1;
        app.stop_terminal(app.saved.workspaces[1].focus);
        assert_eq!(app.active, 0);
        assert_eq!(app.saved.workspaces[0].task.id, active_task);
    }

    #[test]
    fn stopping_split_pane_preserves_survivors_and_focus() {
        let mut app = fixture(1);
        let a = app.saved.workspaces[0].focus;
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        app.saved.workspaces[0].layout.split(a, b, true);
        app.saved.workspaces[0].layout.split(b, c, false);
        app.saved.workspaces[0].focus = c;
        app.stop_terminal(b);
        let mut ids = Vec::new();
        app.saved.workspaces[0].layout.ids(&mut ids);
        assert_eq!(ids, vec![a, c]);
        assert_eq!(app.saved.workspaces[0].focus, c);
        app.stop_terminal(c);
        assert_eq!(app.saved.workspaces[0].focus, a);
        assert!(matches!(app.saved.workspaces[0].layout, Layout::Pane(id) if id == a));
        app.stop_terminal(a);
        assert!(app.saved.workspaces.is_empty());
    }

    #[test]
    fn stacked_split_shortcut_creates_one_stacked_split() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.saved.workspaces[0].task.directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        start_fixture_terminal(&mut app, &ctx);
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::D, None, command() | Modifiers::SHIFT)],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert_eq!(app.panes.len(), 2);
        assert!(matches!(
            app.saved.workspaces[0].layout,
            Layout::Split {
                vertical: false,
                ..
            }
        ));
    }

    #[test]
    fn named_workspace_creation_retries_invalid_directory_and_submits_from_name() {
        let ctx = egui::Context::default();
        let mut app = fixture(0);
        let dir = tempfile::tempdir().unwrap();
        app.new_directory = dir.path().join("missing").to_string_lossy().into_owned();
        app.new_workspace_name = "  Review changes  ".into();
        app.palette = true;
        let frame = |app: &mut App, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 760.0))),
                    events,
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        };
        frame(&mut app, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("new-workspace-name")));
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert!(app.saved.workspaces.is_empty());
        assert!(app.palette);
        assert_eq!(app.new_workspace_name, "  Review changes  ");
        app.new_directory = dir.path().to_string_lossy().into_owned();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("new-workspace-name")));
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert_eq!(app.saved.workspaces.len(), 1);
        assert_eq!(app.saved.workspaces[0].task.title, "Review changes");
        assert_eq!(app.saved.workspaces[0].task.directory, app.new_directory);
        assert_eq!(app.panes.len(), 1);
        assert!(!app.palette);
        assert!(app.new_workspace_name.is_empty());
        assert!(app.add_workspace(&ctx));
        assert_eq!(
            app.saved.workspaces[1].task.title,
            dir.path().file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn enter_in_workspace_fields_creates_or_opens_and_preserves_invalid_input() {
        let ctx = egui::Context::default();
        let mut app = fixture(2);
        let dir = tempfile::tempdir().unwrap();
        app.new_directory = dir.path().to_string_lossy().into_owned();
        app.palette = true;
        let frame = |app: &mut App, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 760.0))),
                    events,
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        };
        frame(&mut app, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("workspace-directory")));
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert_eq!(app.saved.workspaces.len(), 3);
        assert_eq!(app.panes.len(), 1);
        assert!(!app.palette);

        app.palette = true;
        app.new_directory = dir.path().join("missing").to_string_lossy().into_owned();
        frame(&mut app, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("workspace-directory")));
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert!(app.palette);
        assert_eq!(app.saved.workspaces.len(), 3);
        assert_eq!(app.panes.len(), 1);
        assert!(app.error.contains("not a directory"));

        app.saved.workspaces[1].task.title = "Unique match".into();
        app.filter = "Unique".into();
        frame(&mut app, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("workspace-filter")));
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert_eq!(app.active, 1);
        assert!(!app.palette);
    }

    #[test]
    fn workspace_enter_takes_priority_over_overview_and_live_terminal() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        let dir = tempfile::tempdir().unwrap();
        app.saved.workspaces[0].task.directory = dir.path().to_string_lossy().into_owned();
        start_fixture_terminal(&mut app, &ctx);
        let original = app.saved.workspaces[0].focus;
        let frame = |app: &mut App, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 760.0))),
                    events,
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        };
        for overview in [false, true] {
            app.overview = overview;
            app.palette = true;
            app.new_directory = dir.path().join("missing").to_string_lossy().into_owned();
            frame(&mut app, vec![]);
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("workspace-directory")));
            frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
            assert!(app.palette);
            assert_eq!(app.overview, overview);
            assert_eq!(app.saved.workspaces.len(), 1);
            assert!(app.error.contains("not a directory"));
            assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new("workspace-directory"))));
        }
        app.new_directory = dir.path().to_string_lossy().into_owned();
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert!(!app.palette);
        assert_eq!(app.saved.workspaces.len(), 2);
        assert_eq!(app.panes.len(), 2);
        assert!(app.panes.contains_key(&original));
    }

    #[test]
    fn terminal_cursor_geometry_distinguishes_typing_and_selection() {
        let cell = Vec2::new(9.0, 20.0);
        let beam = cursor_rect(CursorShape::Beam, Pos2::ZERO, cell).unwrap();
        assert_eq!(beam.size(), Vec2::new(2.0, 20.0));
        let underline = cursor_rect(CursorShape::Underline, Pos2::ZERO, cell).unwrap();
        assert_eq!(underline.min.y, 18.0);
        assert_eq!(underline.height(), 2.0);
        assert_eq!(
            cursor_rect(CursorShape::HollowBlock, Pos2::ZERO, cell)
                .unwrap()
                .size(),
            cell
        );
        assert!(cursor_rect(CursorShape::Hidden, Pos2::ZERO, cell).is_none());
    }

    #[test]
    fn block_cursor_preserves_glyph_contrast_and_terminal_clip() {
        let ctx = egui::Context::default();
        let clip = Rect::from_min_size(Pos2::ZERO, Vec2::new(30.0, 40.0));
        for (color, foreground) in [(ACCENT, Color32::BLACK), (Color32::BLACK, Color32::WHITE)] {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                let painter = ctx
                    .layer_painter(egui::LayerId::background())
                    .with_clip_rect(clip);
                paint_cursor(
                    &painter,
                    CursorShape::Block,
                    Pos2::new(24.0, 2.0),
                    Vec2::new(18.0, 20.0),
                    color,
                    &FontId::monospace(15.0),
                    "界\u{301}",
                );
            });
            let block = output.shapes.iter().find(|shape| matches!(&shape.shape, egui::epaint::Shape::Rect(rect) if rect.fill == color)).unwrap();
            assert_eq!(block.clip_rect, clip);
            if let egui::epaint::Shape::Rect(rect) = &block.shape {
                assert_eq!(rect.rect.width(), 18.0);
            }
            let glyph = output.shapes.iter().find(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text() == "界\u{301}")).unwrap();
            assert_eq!(glyph.clip_rect, clip);
            if let egui::epaint::Shape::Text(text) = &glyph.shape {
                assert_eq!(text.fallback_color, foreground);
            }
        }
        let tiny = Vec2::new(1.0, 1.0);
        assert_eq!(
            cursor_rect(CursorShape::Beam, Pos2::ZERO, tiny)
                .unwrap()
                .size(),
            tiny
        );
        assert_eq!(
            cursor_rect(CursorShape::Underline, Pos2::ZERO, tiny)
                .unwrap()
                .min,
            Pos2::ZERO
        );
    }

    #[test]
    fn focused_terminal_receives_keyboard_input_once() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.overview = false;
        app.saved.sessions.clear();
        let pane = app.saved.workspaces[0].focus;
        let dir = std::env::current_dir().unwrap();
        let terminal = Terminal::spawn(
            pane,
            &dir,
            std::path::Path::new("/tmp/unused-test.sock"),
            ctx.clone(),
        )
        .unwrap();
        app.panes.insert(
            pane,
            Pane {
                terminal,
                search: TerminalSearch::default(),
            },
        );
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        }
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("keyboard-once");
        let command = format!("printf x >> '{}'\r", output_path.display());
        let grid = app.panes[&pane].terminal.term.clone();
        let held = grid.lock().unwrap();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                events: vec![egui::Event::Text(command)],
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        );
        drop(held);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if std::fs::read(&output_path).is_ok_and(|bytes| !bytes.is_empty()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Shells may repaint their echoed command multiple times. Check the command's
        // effect, rather than counting its appearances in terminal scrollback.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            std::fs::read(&output_path).expect("terminal widget swallowed keyboard input"),
            b"x",
            "keyboard input was duplicated"
        );
        let command = format!("printf y >> '{}'", output_path.display());
        app.panes.get_mut(&pane).unwrap().search.open();
        let frame = |app: &mut App, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                    events,
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Text(command.clone())]);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        frame(&mut app, vec![key(Key::Enter)]);
        frame(&mut app, vec![key(Key::Escape)]);
        assert!(!app.panes[&pane].search.is_open());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            b"x",
            "search input leaked into shell"
        );
        frame(&mut app, vec![egui::Event::Text(format!("{command}\r"))]);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if std::fs::read(&output_path).is_ok_and(|bytes| bytes == b"xy") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            b"xy",
            "terminal focus was not restored"
        );
        let command = format!("printf z >> '{}'\r", output_path.display());
        app.panes
            .get_mut(&pane)
            .unwrap()
            .terminal
            .selection_action(SelectionAction::Enter)
            .unwrap();
        let held = grid.lock().unwrap();
        frame(
            &mut app,
            vec![
                egui::Event::Text(command.clone()),
                egui::Event::Paste(command.clone()),
                egui::Event::Ime(egui::ImeEvent::Commit(command.clone())),
                key(Key::ArrowLeft),
                key(Key::Space),
                key(Key::Enter),
                egui::Event::Text(command.clone()),
            ],
        );
        assert!(!app.panes[&pane].terminal.copy_mode);
        drop(held);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            b"xy",
            "copy-mode input leaked into shell"
        );
        frame(&mut app, vec![egui::Event::Text(command)]);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if std::fs::read(&output_path).is_ok_and(|bytes| bytes == b"xyz") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            b"xyz",
            "copy mode did not restore terminal input"
        );
    }
    #[test]
    fn overview_hundred_session_workload() {
        let ctx = egui::Context::default();
        let mut app = fixture(100);
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                    ..Default::default()
                },
                |ctx| {
                    app.draw(ctx);
                },
            );
        }
        eprintln!(
            "100 cached Overview frames / 100 synthetic sessions: {:?} (headless debug build)",
            start.elapsed()
        );
    }
}
