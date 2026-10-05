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
    // Use the split ratios directly: navigation also works before the first paint
    // and while a pane is maximized, without stale screen coordinates.
    fn navigation_rects(&self, rect: Rect, out: &mut Vec<(Uuid, Rect)>) {
        match self {
            Self::Pane(id) => out.push((*id, rect)),
            Self::Split {
                vertical,
                ratio,
                a,
                b,
            } => {
                let mut first = rect;
                let mut second = rect;
                if *vertical {
                    let cut = rect.left() + rect.width() * ratio;
                    first.max.x = cut;
                    second.min.x = cut;
                } else {
                    let cut = rect.top() + rect.height() * ratio;
                    first.max.y = cut;
                    second.min.y = cut;
                }
                a.navigation_rects(first, out);
                b.navigation_rects(second, out);
            }
        }
    }

    fn neighbor(&self, focus: Uuid, direction: Key) -> Option<Uuid> {
        let mut rects = Vec::new();
        self.navigation_rects(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0)),
            &mut rects,
        );
        let source = rects.iter().find(|(id, _)| *id == focus)?.1;
        let horizontal = matches!(direction, Key::ArrowLeft | Key::ArrowRight);
        let axis = |rect: Rect| {
            if horizontal {
                (rect.left(), rect.right(), rect.top(), rect.bottom())
            } else {
                (rect.top(), rect.bottom(), rect.left(), rect.right())
            }
        };
        let (start, end, cross_start, cross_end) = axis(source);
        let forward = matches!(direction, Key::ArrowRight | Key::ArrowDown);
        rects
            .into_iter()
            .filter_map(|(id, rect)| {
                if id == focus {
                    return None;
                }
                let (other_start, other_end, other_cross_start, other_cross_end) = axis(rect);
                let gap = if forward {
                    other_start - end
                } else {
                    start - other_end
                };
                let overlap = cross_end.min(other_cross_end) - cross_start.max(other_cross_start);
                if gap < -f32::EPSILON || overlap <= 0.0 {
                    return None;
                }
                let offset =
                    ((cross_start + cross_end) - (other_cross_start + other_cross_end)).abs();
                Some((id, gap.max(0.0), offset))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
            .map(|(id, _, _)| id)
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
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    created_directories: Vec<CreatedDirectory>,
    #[serde(default)]
    specifications: Vec<crate::specs::Specification>,
    #[serde(default = "default_sidebar_width")]
    sidebar_width: f32,
    workspaces: Vec<Workspace>,
    sessions: Vec<Session>,
    #[serde(default = "default_font")]
    font_size: f32,
    #[serde(default)]
    light: bool,
    #[serde(default)]
    skip_stop_confirmation: bool,
    #[serde(default = "default_clickable_choices")]
    clickable_codex_choices: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            created_directories: Vec::new(),
            specifications: Vec::new(),
            sidebar_width: default_sidebar_width(),
            workspaces: Vec::new(),
            sessions: Vec::new(),
            font_size: default_font(),
            light: false,
            skip_stop_confirmation: false,
            clickable_codex_choices: default_clickable_choices(),
        }
    }
}
fn default_sidebar_width() -> f32 {
    216.0
}

fn default_clickable_choices() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
struct CreatedDirectory {
    path: String,
    created_at: u64,
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

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum CommandPage {
    #[default]
    Commands,
    Create,
    Settings,
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
    specifications_view: bool,
    selected_spec: Option<Uuid>,
    spec_codex: bool,
    selected: Option<String>,
    endpoint: Option<Endpoint>,
    discovery: Option<crate::discovery::Discovery>,
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
    command_page: CommandPage,
    command_index: usize,
    command_focus: bool,
    rename: Option<WorkspaceRename>,
    closing: Option<TerminalClose>,
    filter: String,
    maximized: bool,
    category: u8,
    overview_inspector: bool,
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
            specifications_view: false,
            selected_spec: None,
            spec_codex: false,
            selected: None,
            error: load_error,
            endpoint: None,
            discovery: Some(crate::discovery::Discovery::start(cc.egui_ctx.clone())),
            writer,
            persistence: Some(persistence),
            save_error,
            dirty: false,
            last_save: 0,
            new_directory: crate::directories::default_directory()
                .to_string_lossy()
                .into(),
            new_workspace_name: String::new(),
            palette: false,
            command_page: CommandPage::Commands,
            command_index: 0,
            command_focus: false,
            rename: None,
            closing: None,
            filter: String::new(),
            maximized: false,
            category: 0,
            overview_inspector: false,
        };
        match endpoint {
            Ok(e) => app.endpoint = Some(e),
            Err(e) => app.error = format!("Event endpoint unavailable: {e}"),
        }
        match crate::updater::Updater::start() {
            Ok(updater) => app.updater = updater,
            Err(error) => app.updater_error = Some(format!("Updates unavailable: {error}")),
        }
        configure_terminal_fonts_with(&cc.egui_ctx, crate::fonts::automatic_terminal_fonts());
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
        #[cfg(not(test))]
        let result = Terminal::spawn(id, &PathBuf::from(directory), socket, ctx.clone());
        #[cfg(test)]
        let result = Terminal::spawn_test(id, &PathBuf::from(directory), socket, ctx.clone());
        match result {
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
    fn open_selection_tab(&mut self, ctx: &egui::Context, source: Uuid, text: &str) {
        let result = (|| {
            let text = selection_paste_text(text)?;
            let directory = self
                .panes
                .get(&source)
                .ok_or_else(|| anyhow::anyhow!("Source terminal is unavailable"))?
                .terminal
                .current_directory()?
                .into_os_string()
                .into_string()
                .map_err(|_| anyhow::anyhow!("working directory is not valid UTF-8"))?;
            if self.add_workspace_in(ctx, directory) {
                let id = self.saved.workspaces[self.active].focus;
                self.panes[&id].terminal.paste(&text)?;
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = result {
            self.error = format!("Cannot open selected text in a terminal: {error}");
        }
    }
    fn add_workspace(&mut self, ctx: &egui::Context) -> bool {
        if self.saved.workspaces.len() >= 32 {
            self.error = "Workspace limit (32) reached".into();
            return false;
        }
        let directory = match self.prepare_directory(&self.new_directory.clone()) {
            Ok(directory) => directory,
            Err(error) => {
                self.error = error.to_string();
                return false;
            }
        };
        if !self.add_workspace_in(ctx, directory.clone()) {
            return false;
        }
        self.new_directory = directory;
        let name = self.new_workspace_name.trim();
        if !name.is_empty() {
            self.saved.workspaces[self.active].task.title = name.to_owned();
        }
        self.new_workspace_name.clear();
        self.error.clear();
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
        self.specifications_view = false;
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
            self.specifications_view = false;
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
            ui.add_space(8.0);
            ui.label("The shell and its running processes will be stopped.");
            ui.add_space(12.0);
            ui.checkbox(
                &mut closing.skip_confirmation,
                "Don’t ask again for any terminal",
            );
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                confirm = ui
                    .add(egui::Button::new(
                        RichText::new("Stop terminal").color(ui.visuals().error_fg_color),
                    ))
                    .clicked();
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
        self.specifications_view = false;
        self.dirty = true;
        self.add_workspace_in(ctx, self.new_directory.clone());
    }
    fn drain(&mut self) {
        if let Some(discovery) = &mut self.discovery {
            let roots = self
                .panes
                .iter()
                .filter_map(|(&pane, state)| state.terminal.shell_pid().map(|pid| (pane, pid)))
                .collect();
            if let Some(mut found) = discovery.scan(roots) {
                found.retain(|&(pane, _, _)| self.panes.contains_key(&pane));
                self.dirty |= crate::discovery::reconcile(&mut self.saved.sessions, &found, now());
            }
        }
        if let Some(endpoint) = &self.endpoint {
            for e in endpoint.receiver.try_iter().take(256) {
                if !self.panes.contains_key(&e.pane) {
                    continue;
                }
                if let Some(existing) = self.saved.sessions.iter().find(|session| {
                    session.session_id == e.session_id
                        && session.pane == e.pane
                        && session.agent == e.agent
                        && session.observed_process.is_none()
                }) {
                    let key = existing.key();
                    self.saved.sessions.retain(|session| {
                        let placeholder = session.pane == e.pane
                            && session.agent == e.agent
                            && session.observed_process.is_some()
                            && session.state == SessionState::Unknown;
                        if placeholder && self.selected.as_deref() == Some(session.key().as_str()) {
                            self.selected = Some(key.clone());
                        }
                        !placeholder
                    });
                }
                let idx = self.saved.sessions.iter().position(|s| {
                    s.session_id == e.session_id && s.pane == e.pane && s.agent == e.agent
                });
                let idx = idx
                    .or_else(|| {
                        self.saved.sessions.iter().position(|session| {
                            session.pane == e.pane
                                && session.agent == e.agent
                                && session.observed_process.is_some()
                                && session.state == SessionState::Unknown
                        })
                    })
                    .unwrap_or_else(|| {
                        if self.saved.sessions.len() >= 256 {
                            self.saved.sessions.remove(0);
                        }
                        let mut session = Session::new(e.pane, e.session_id.clone());
                        session.agent = e.agent;
                        self.saved.sessions.push(session);
                        self.saved.sessions.len() - 1
                    });
                if self.saved.sessions[idx].observed_process.take().is_some() {
                    let old_key = self.saved.sessions[idx].key();
                    self.saved.sessions[idx].session_id = e.session_id.clone();
                    if self.selected.as_deref() == Some(old_key.as_str()) {
                        self.selected = Some(self.saved.sessions[idx].key());
                    }
                }
                self.dirty |= self.saved.sessions[idx].apply(e, now());
            }
        }
        let exited: Vec<_> = self
            .panes
            .iter()
            .filter_map(|(&id, pane)| pane.terminal.has_exited().then_some(id))
            .collect();
        for id in exited {
            let code = self.panes[&id].terminal.exit_code();
            let mut linked = false;
            for spec in &mut self.saved.specifications {
                if let Some(launch) = spec.launches.iter_mut().find(|launch| launch.pane == id) {
                    linked = true;
                    if launch.exit_code.is_none()
                        && let Some(code) = code
                    {
                        launch.exit_code = Some(code);
                        self.dirty = true;
                        if code != 0 {
                            self.error = format!(
                                "{} exited with code {code}. Output is retained in its terminal; check installation, PATH and authentication, then retry from Specs.",
                                launch.agent
                            );
                            if spec.status == 2
                                && !spec.launches.iter().any(|launch| {
                                    self.panes
                                        .get(&launch.pane)
                                        .is_some_and(|pane| !pane.terminal.has_exited())
                                })
                            {
                                spec.status = 1;
                            }
                        }
                    }
                }
            }
            // Spec launches retain their output until explicitly closed.
            if !linked {
                self.stop_terminal(id);
            }
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
                self.specifications_view = false;
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
            ui.add_space(12.0);
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
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                submit |= ui
                    .add_enabled_ui(valid, |ui| primary_button(ui, "Rename"))
                    .inner
                    .clicked();
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
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::S)) {
            self.specifications_view = !self.specifications_view;
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::O)) {
            self.overview = if self.specifications_view {
                true
            } else {
                !self.overview
            };
            self.specifications_view = false;
        }
        if self.specifications_view {
            if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::P)) {
                self.open_commands(CommandPage::Commands);
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::R)) {
            self.begin_rename();
            return;
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
            if self.palette {
                self.palette = false;
            } else {
                self.open_commands(CommandPage::Commands);
            }
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
            self.specifications_view = false;
            self.palette = false;
            self.maximized = false;
        }
        if !self.palette && !self.overview {
            for direction in [
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::ArrowUp,
                Key::ArrowDown,
            ] {
                // Exact modifiers keep the existing next-pane shortcut distinct.
                let pressed = ctx.input_mut(|input| {
                    input.modifiers.alt == command.alt
                        && input.modifiers.ctrl == command.ctrl
                        && input.modifiers.shift == command.shift
                        && input.modifiers.mac_cmd == command.mac_cmd
                        && input.consume_key(command, direction)
                });
                if pressed
                    && let Some(workspace) = self.saved.workspaces.get_mut(self.active)
                    && let Some(next) = workspace.layout.neighbor(workspace.focus, direction)
                {
                    workspace.focus = next;
                    self.dirty = true;
                }
            }
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
    fn cleanup_disconnected_sessions(&mut self) {
        let previous = self.saved.sessions.len();
        self.saved
            .sessions
            .retain(|session| session.state != SessionState::Disconnected);
        if self.saved.sessions.len() != previous {
            if !self
                .saved
                .sessions
                .iter()
                .any(|session| self.selected.as_deref() == Some(session.key().as_str()))
            {
                self.selected = None;
            }
            self.dirty = true;
        }
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        let narrow = ui.available_width() < 760.0;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Session overview");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if narrow && !self.saved.sessions.is_empty() {
                    if ui
                        .selectable_label(self.overview_inspector, "Activity")
                        .clicked()
                    {
                        self.overview_inspector = true;
                    }
                    if ui
                        .selectable_label(!self.overview_inspector, "Sessions")
                        .clicked()
                    {
                        self.overview_inspector = false;
                    }
                }
                ui.label(
                    RichText::new(format!("{} recorded", self.saved.sessions.len()))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
        });
        ui.add_space(8.0);
        let disconnected = self
            .saved
            .sessions
            .iter()
            .filter(|session| session.state == SessionState::Disconnected)
            .count();
        if disconnected > 0 && ui.button(format!("Clear disconnected sessions ({disconnected})"))
            .on_hover_text("Remove disconnected session history. Terminals and working directories are kept.")
            .clicked() {
            self.cleanup_disconnected_sessions();
        }
        ui.horizontal_wrapped(|ui| {
            let attention = self
                .saved
                .sessions
                .iter()
                .filter(|s| s.state.attention())
                .count();
            let running = self
                .saved
                .sessions
                .iter()
                .filter(|s| s.state == SessionState::Running)
                .count();
            ui.label(
                RichText::new(format!("{attention} need attention")).color(attention_color(ui)),
            );
            ui.separator();
            ui.label(RichText::new(format!("{running} running")).color(accent(ui)));
        });
        ui.add_space(16.0);
        if self.saved.sessions.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("overview-empty")
                .max_height(ui.available_height())
                .show(ui, |ui| {
                    ui.add_space(12.0);
                    ui.heading("Connect your first agent");
                    ui.label(
                        "Install lifecycle hooks, then start Claude Code or Codex in a terminal.",
                    );
                    ui.add_space(14.0);
                    section_label(ui, "CLAUDE CODE");
                    ui.monospace("tessera install-hooks");
                    ui.add_space(8.0);
                    section_label(ui, "CODEX");
                    ui.monospace("tessera install-codex-hooks");
                    ui.small("Review and trust the hooks with /hooks in Codex.");
                    ui.monospace("codex --no-daemon");
                    ui.small("Start in this pane without the shared background server.");
                    ui.add_space(16.0);
                    if primary_button(ui, "Open terminal").clicked() {
                        self.overview = false;
                        self.specifications_view = false;
                    }
                });
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for (category, label) in [
                (0, "All"),
                (1, "Needs you"),
                (2, "Running"),
                (3, "Review"),
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
                    .find(|w| layout_contains(&w.layout, s.pane))
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
        // A hidden selection must not display context from another filter.
        if !visible.iter().any(|index| {
            self.selected.as_deref() == Some(self.saved.sessions[*index].key().as_str())
        }) {
            self.selected = visible
                .first()
                .map(|index| self.saved.sessions[*index].key());
        }
        let list_width = if narrow {
            ui.available_width()
        } else {
            ui.available_width() * 0.52
        };
        let list_height = ui.available_height();
        let mut open = None;
        if narrow && self.overview_inspector && self.selected.is_some() {
            self.inspector(ui);
        } else {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(list_width, list_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if visible.is_empty() {
                            ui.add_space(18.0);
                            ui.label("No sessions in this view");
                            ui.small("New lifecycle events will appear here.");
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("sessions")
                            .max_height(list_height)
                            .auto_shrink([false, false])
                            .show_rows(ui, 100.0, visible.len(), |ui, range| {
                                for index in &visible[range] {
                                    let s = &self.saved.sessions[*index];
                                    let workspace = self
                                        .saved
                                        .workspaces
                                        .iter()
                                        .find(|w| layout_contains(&w.layout, s.pane));
                                    let title = workspace
                                        .map_or("Closed workspace", |w| w.task.title.as_str());
                                    let directory = workspace
                                        .map_or("Retained session history", |w| {
                                            w.task.directory.as_str()
                                        });
                                    let selected =
                                        self.selected.as_deref() == Some(s.key().as_str());
                                    let tint = if s.state.attention() {
                                        attention_color(ui)
                                    } else if s.state == SessionState::Running {
                                        accent(ui)
                                    } else {
                                        ui.visuals().weak_text_color()
                                    };
                                    let frame = egui::Frame::new()
                                        .fill(if selected {
                                            ui.visuals().selection.bg_fill
                                        } else {
                                            ui.visuals().panel_fill
                                        })
                                        .inner_margin(12.0)
                                        .corner_radius(4);
                                    let response = frame
                                        .show(ui, |ui| {
                                            ui.set_width((list_width - 30.0).max(80.0));
                                            ui.set_min_height(76.0);
                                            ui.spacing_mut().item_spacing.y = 3.0;
                                            ui.spacing_mut().interact_size.y = 20.0;
                                            ui.horizontal(|ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(title).strong().size(15.0),
                                                    )
                                                    .truncate(),
                                                );
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        ui.label(
                                                            RichText::new(s.agent.label())
                                                                .size(11.0)
                                                                .color(
                                                                    ui.visuals().weak_text_color(),
                                                                ),
                                                        );
                                                    },
                                                );
                                            });
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(directory)
                                                        .small()
                                                        .color(ui.visuals().weak_text_color()),
                                                )
                                                .truncate(),
                                            );
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(s.state.label())
                                                        .size(12.0)
                                                        .color(tint),
                                                );
                                                ui.label(
                                                    RichText::new(relative_time(
                                                        now().saturating_sub(s.updated),
                                                    ))
                                                    .small()
                                                    .color(ui.visuals().weak_text_color()),
                                                );
                                            });
                                            let detail = s
                                                .history
                                                .back()
                                                .map(|a| {
                                                    if a.detail.is_empty() {
                                                        a.kind.clone()
                                                    } else {
                                                        format!("{} · {}", a.kind, a.detail)
                                                    }
                                                })
                                                .unwrap_or_else(|| if s.observed_process.is_some() {
                                                    "Agent detected · waiting for lifecycle hooks".into()
                                                } else {
                                                    "Waiting for activity".into()
                                                });
                                            ui.add(
                                                egui::Label::new(RichText::new(detail).small())
                                                    .truncate(),
                                            );
                                        })
                                        .response;
                                    let response = ui.interact(
                                        response.rect,
                                        ui.id().with(s.key()),
                                        Sense::click(),
                                    );
                                    response.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::SelectableLabel,
                                            true,
                                            selected,
                                            title,
                                        )
                                    });
                                    if selected {
                                        ui.painter().line_segment(
                                            [
                                                response.rect.left_top() + Vec2::new(0.0, 6.0),
                                                response.rect.left_bottom() - Vec2::new(0.0, 6.0),
                                            ],
                                            Stroke::new(2.0_f32, accent(ui)),
                                        );
                                    }
                                    if response.clicked() {
                                        self.selected = Some(s.key());
                                        if narrow {
                                            self.overview_inspector = true;
                                        }
                                    }
                                    if response.double_clicked() {
                                        open = Some(s.key());
                                    }
                                }
                            });
                    },
                );
                if !narrow {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(10.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            self.inspector(ui);
                        },
                    );
                }
            });
        }
        if let Some(id) = open {
            self.focus_session(&id);
        }
        if !self.palette
            && self.rename.is_none()
            && self.closing.is_none()
            && !ui.ctx().wants_keyboard_input()
        {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.overview = false;
                self.specifications_view = false;
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
                    self.specifications_view = false;
                }
            }
        }
    }
    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected.clone() else {
            ui.label(
                RichText::new("Select a session to see its activity.")
                    .color(ui.visuals().weak_text_color()),
            );
            return;
        };
        let Some(index) = self.saved.sessions.iter().position(|s| s.key() == id) else {
            return;
        };
        let session = &self.saved.sessions[index];
        let pane = session.pane;
        let live = self.panes.contains_key(&pane);
        let wi = self
            .saved
            .workspaces
            .iter()
            .position(|w| layout_contains(&w.layout, pane));
        let mut open = false;
        let linked_spec = self.saved.specifications.iter().find_map(|spec| {
            spec.launches
                .iter()
                .find(|launch| launch.pane == pane)
                .map(|launch| (spec.id, spec.draft.title.clone(), launch.revision))
        });
        egui::ScrollArea::vertical()
            .id_salt("inspector")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                section_label(ui, "SESSION CONTEXT");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading(session.agent.label());
                    ui.label(RichText::new(session.state.label()).color(
                        if session.state.attention() {
                            attention_color(ui)
                        } else {
                            accent(ui)
                        },
                    ));
                });
                ui.add_space(8.0);
                open = ui
                    .add_enabled_ui(live, |ui| primary_button(ui, "Open live terminal"))
                    .inner
                    .clicked();
                if let Some((id, title, revision)) = &linked_spec
                    && ui
                        .button(format!("Spec: {title} · revision {revision}"))
                        .clicked()
                {
                    self.selected_spec = Some(*id);
                    self.specifications_view = true;
                }
                if !live {
                    ui.small("Terminal closed · history retained");
                }
                if let Some(wi) = wi {
                    ui.add_space(18.0);
                    section_label(ui, "TASK");
                    let w = &mut self.saved.workspaces[wi];
                    let label = ui.label("Title");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut w.task.title)
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(label.id)
                        .changed()
                    {
                        self.dirty = true;
                    }
                    ui.horizontal_wrapped(|ui| {
                        for (state, label) in [
                            (TaskState::Implementing, "Implementing"),
                            (TaskState::ReviewRequested, "Request review"),
                            (TaskState::Accepted, "Accept task"),
                        ] {
                            if ui.selectable_label(w.task.state == state, label).clicked() {
                                w.task.state = state;
                                self.dirty = true;
                            }
                        }
                    });
                }
                ui.add_space(18.0);
                let unread = session
                    .history
                    .iter()
                    .filter(|a| a.sequence > session.seen_sequence)
                    .count();
                section_label(ui, &format!("ACTIVITY · {unread} NEW"));
                ui.add_space(8.0);
                for a in session.history.iter().rev() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&a.kind).strong().color(
                            if a.sequence > session.seen_sequence {
                                accent(ui)
                            } else {
                                ui.visuals().text_color()
                            },
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(relative_time(now().saturating_sub(a.at)))
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                    });
                    if !a.detail.is_empty() {
                        ui.label(&a.detail);
                    }
                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(5.0);
                }
                ui.small("Review and acceptance are explicit task actions.");
            });
        if open {
            self.saved.sessions[index].seen = now();
            self.saved.sessions[index].seen_sequence = self.saved.sessions[index].last_sequence;
            self.dirty = true;
            self.focus_session(&id);
        }
    }
}
impl App {
    fn open_commands(&mut self, page: CommandPage) {
        self.palette = true;
        self.command_page = page;
        self.command_index = 0;
        self.command_focus = true;
        self.filter.clear();
    }

    fn chrome(
        &mut self,
        ctx: &egui::Context,
        wide: bool,
        pane_action: &mut Option<(Uuid, &'static str)>,
    ) {
        egui::TopBottomPanel::top("chrome")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 27, 31))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                *ui.visuals_mut() = egui::Visuals::dark();
                let native_controls = cfg!(target_os = "macos")
                    && !ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
                let mut drag_rect = ui.max_rect();
                drag_rect.max.y = drag_rect.min.y + 24.0;
                if native_controls {
                    drag_rect.min.x += 64.0;
                }
                if cfg!(target_os = "macos") {
                    let drag = ui.interact(
                        drag_rect,
                        ui.id().with("window-drag"),
                        Sense::click_and_drag(),
                    );
                    if drag.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if drag.double_clicked() {
                        let maximized =
                            ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.spacing_mut().interact_size.y = 28.0;
                    ui.set_min_height(28.0);
                    if native_controls {
                        ui.add_space(64.0);
                    }
                    if wide {
                        mosaic_mark(ui);
                        ui.label(RichText::new("TESSERA").size(14.0).strong());
                        ui.add_space(18.0);
                    }
                    if ui
                        .selectable_label(!self.overview && !self.specifications_view, "Terminal")
                        .clicked()
                    {
                        self.overview = false;
                        self.specifications_view = false;
                    }
                    if ui
                        .selectable_label(self.specifications_view, "Specs")
                        .clicked()
                    {
                        self.specifications_view = true;
                    }
                    let attention = self
                        .saved
                        .sessions
                        .iter()
                        .filter(|s| s.state.attention())
                        .count();
                    let overview = if attention > 0 {
                        format!("Overview · {attention}")
                    } else {
                        "Overview".into()
                    };
                    if ui
                        .selectable_label(self.overview && !self.specifications_view, overview)
                        .on_hover_text(shortcut("Overview", "Shift+O"))
                        .clicked()
                    {
                        self.overview = true;
                        self.specifications_view = false;
                    }
                    if !self.overview
                        && !self.specifications_view
                        && let Some(workspace) = self.saved.workspaces.get(self.active)
                    {
                        let title = workspace.task.title.clone();
                        let directory = workspace.task.directory.clone();
                        let controls_width = if wide { 208.0 } else { 0.0 };
                        let title_width = (ui.available_width() - 210.0 - controls_width).max(40.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(title_width, 24.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                if wide {
                                    ui.add_sized(
                                        [title_width, 24.0],
                                        egui::Label::new(RichText::new(&title).strong()).truncate(),
                                    )
                                    .on_hover_text(format!("{title}\n{directory}"));
                                } else {
                                    egui::containers::menu::MenuButton::from_button(
                                        egui::Button::new(RichText::new(&title).strong())
                                            .truncate()
                                            .min_size(Vec2::new(title_width, 24.0)),
                                    )
                                    .ui(ui, |ui| self.workspace_actions(ui, ctx))
                                    .0
                                    .on_hover_text(format!(
                                        "{title}\n{directory}\nWorkspace actions"
                                    ));
                                }
                            },
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([96.0, 28.0], egui::Button::new("+ Workspace"))
                            .on_hover_text("Create a workspace in a directory")
                            .clicked()
                        {
                            self.open_commands(CommandPage::Create);
                        }
                        if ui
                            .add_sized([96.0, 28.0], egui::Button::new("Commands"))
                            .on_hover_text(shortcut("Search workspaces and commands", "Shift+P"))
                            .clicked()
                        {
                            self.open_commands(CommandPage::Commands);
                        }
                        if wide
                            && !self.overview
                            && !self.specifications_view
                            && !self.saved.workspaces.is_empty()
                        {
                            egui::containers::menu::MenuButton::from_button(
                                egui::Button::new("Split").min_size(Vec2::new(96.0, 28.0)),
                            )
                            .ui(ui, |ui| self.split_actions(ui, ctx));
                            if ui
                                .add_sized(
                                    [96.0, 28.0],
                                    egui::Button::new(if self.maximized {
                                        "Restore"
                                    } else {
                                        "Maximize"
                                    }),
                                )
                                .on_hover_text(shortcut("Maximize / restore pane", "Shift+Enter"))
                                .clicked()
                            {
                                self.maximized = !self.maximized;
                            }
                        }
                    });
                });
                if !wide {
                    ui.add_space(8.0);
                    egui::ScrollArea::horizontal()
                        .id_salt("workspace-tabs")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                self.workspace_navigation(ui, false, pane_action);
                            });
                        });
                }
            });
    }

    fn workspace_sidebar(
        &mut self,
        ctx: &egui::Context,
        pane_action: &mut Option<(Uuid, &'static str)>,
    ) {
        let sidebar = egui::SidePanel::left("workspace-rail")
            .default_width(self.saved.sidebar_width)
            .width_range(160.0..=ctx.content_rect().width() * 0.45)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(12, 16)),
            )
            .show(ctx, |ui| {
                section_label(ui, "WORKSPACES");
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    if ui
                        .add_sized([ui.available_width(), 32.0], egui::Button::new("Settings"))
                        .clicked()
                    {
                        self.open_commands(CommandPage::Settings);
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("workspace-rail-list")
                            .show(ui, |ui| {
                                self.workspace_navigation(ui, true, pane_action);
                            });
                    });
                });
            });
        let width = sidebar.response.rect.width();
        if (self.saved.sidebar_width - width).abs() > 0.5 {
            self.saved.sidebar_width = width;
            self.dirty = true;
        }
    }

    fn workspace_actions(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if ui
            .button(if self.maximized {
                "Restore pane"
            } else {
                "Maximize pane"
            })
            .clicked()
        {
            self.maximized = !self.maximized;
            ui.close();
        }
        self.split_actions(ui, ctx);
    }

    fn split_actions(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if ui.button(shortcut("Side by side", "D")).clicked() {
            self.split(ctx, true);
            ui.close();
        }
        if ui.button(shortcut("Stacked", "Shift+D")).clicked() {
            self.split(ctx, false);
            ui.close();
        }
    }

    fn workspace_navigation(
        &mut self,
        ui: &mut egui::Ui,
        rail: bool,
        pane_action: &mut Option<(Uuid, &'static str)>,
    ) {
        for (index, w) in self.saved.workspaces.iter().enumerate() {
            let selected = index == self.active && !self.overview && !self.specifications_view;
            let title = format!("{}   {}", index + 1, w.task.title);
            let response = command_row(
                ui,
                &title,
                if rail {
                    directory_name(&w.task.directory)
                } else {
                    ""
                },
                "",
                selected,
                if rail { ui.available_width() } else { 190.0 },
            )
            .on_hover_text(if index < 9 {
                format!(
                    "{}\n{}\n{}",
                    w.task.title,
                    w.task.directory,
                    shortcut(
                        "Switch workspace · right-click for pane actions",
                        &(index + 1).to_string()
                    )
                )
            } else {
                format!(
                    "{}\n{}\nRight-click for pane actions",
                    w.task.title, w.task.directory
                )
            });
            if selected {
                ui.painter().line_segment(
                    [
                        Pos2::new(response.rect.left() + 1.0, response.rect.top() + 8.0),
                        Pos2::new(response.rect.left() + 1.0, response.rect.bottom() - 8.0),
                    ],
                    Stroke::new(2.0_f32, accent(ui)),
                );
            }
            response.context_menu(|ui| {
                for (action, keys) in [
                    ("Find", "F"),
                    ("Select", "Shift+Space"),
                    ("Stop terminal", "W"),
                ] {
                    if ui.button(shortcut(action, keys)).clicked() {
                        *pane_action = Some((w.focus, action));
                        self.active = index;
                        self.overview = false;
                        self.specifications_view = false;
                        self.maximized = false;
                        ui.close();
                    }
                }
            });
            if response.clicked() {
                self.active = index;
                self.overview = false;
                self.specifications_view = false;
                self.maximized = false;
            }
        }
    }

    fn command_center(&mut self, ctx: &egui::Context, submit: Option<egui::Id>, dismiss: bool) {
        let width = 560.0_f32.min((ctx.content_rect().width() - 80.0).max(160.0));
        let height = (if self.command_page == CommandPage::Create {
            450.0_f32
        } else {
            600.0_f32
        })
        .min(ctx.content_rect().height() - 48.0);
        let response = egui::Modal::new(egui::Id::new("workspace-commands"))
            .area(
                egui::Modal::default_area(egui::Id::new("workspace-commands"))
                    .default_size(Vec2::new(width + 44.0, height)),
            )
            .frame(egui::Frame::window(&ctx.style()).inner_margin(22.0))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.set_height(height - 44.0);
                ui.horizontal(|ui| {
                    ui.heading("Workspace & commands");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").on_hover_text("Escape").clicked() {
                            self.palette = false;
                        }
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    for (page, label) in [
                        (CommandPage::Commands, "Commands"),
                        (CommandPage::Create, "New workspace"),
                        (CommandPage::Settings, "Settings"),
                    ] {
                        if ui
                            .selectable_label(self.command_page == page, label)
                            .clicked()
                        {
                            self.command_page = page;
                            self.command_focus = true;
                        }
                    }
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);
                let page = self.command_page;
                if page == CommandPage::Commands {
                    self.command_results(ui, ctx, submit);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("command-center-body", page as u8))
                        .max_height(
                            (ui.available_height()
                                - if page == CommandPage::Create {
                                    52.0
                                } else {
                                    0.0
                                })
                            .max(60.0),
                        )
                        .auto_shrink([false, true])
                        .show(ui, |ui| match page {
                            CommandPage::Create => self.workspace_form(ui),
                            CommandPage::Settings => self.settings(ui, ctx),
                            CommandPage::Commands => {}
                        });
                }
                if page == CommandPage::Create {
                    ui.add_space(12.0);
                    let create = ui
                        .add_enabled_ui(!self.new_directory.trim().is_empty(), |ui| {
                            primary_button(ui, "Create workspace")
                        })
                        .inner;
                    if create.clicked()
                        || submit == Some(egui::Id::new("workspace-directory"))
                        || submit == Some(egui::Id::new("new-workspace-name"))
                    {
                        if self.add_workspace(ctx) {
                            self.palette = false;
                        } else {
                            ctx.memory_mut(|memory| {
                                memory.request_focus(egui::Id::new("workspace-directory"))
                            });
                        }
                    }
                }
            });
        if dismiss || response.should_close() {
            self.palette = false;
        }
    }

    fn workspace_form(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().content_rect().height() >= 500.0 {
            ui.label(RichText::new("Start a new workspace").size(18.0).strong());
            ui.label(
                RichText::new("A login shell in the directory you choose.")
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(16.0);
        }
        let label = ui.label("Working directory");
        let directory = ui
            .add(
                egui::TextEdit::singleline(&mut self.new_directory)
                    .id(egui::Id::new("workspace-directory"))
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("~/my-project or an absolute path"),
            )
            .labelled_by(label.id);
        if self.command_focus {
            directory.request_focus();
            self.command_focus = false;
        }
        if let Some(alternative) = crate::directories::root_alternative(&self.new_directory) {
            ui.small("A leading / points to the filesystem root, not your home directory.");
            if ui.button(format!("Use {alternative}")).clicked() {
                self.new_directory = alternative;
                self.error.clear();
            }
        }
        if let Ok(path) = crate::directories::resolve(&self.new_directory) {
            ui.small(format!("Workspace directory: {}", path.display()));
        }
        ui.add_space(10.0);
        let label = ui.label("Name (optional)");
        ui.add(
            egui::TextEdit::singleline(&mut self.new_workspace_name)
                .id(egui::Id::new("new-workspace-name"))
                .desired_width(f32::INFINITY)
                .hint_text("Use the directory name"),
        )
        .labelled_by(label.id);
        ui.label(
            RichText::new("Missing directories will be created. ~/ means your home directory. Name only changes the label.")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        if !self.error.is_empty() {
            ui.add_space(10.0);
            ui.colored_label(ui.visuals().error_fg_color, &self.error);
        }
    }

    fn command_results(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        submit: Option<egui::Id>,
    ) {
        let down = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown));
        let up = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp));
        let label = ui.label("Search workspaces and commands");
        let filter = ui
            .add(
                egui::TextEdit::singleline(&mut self.filter)
                    .id(egui::Id::new("workspace-filter"))
                    .desired_width(f32::INFINITY)
                    .hint_text("Workspace name, split, settings…"),
            )
            .labelled_by(label.id);
        if self.command_focus {
            filter.request_focus();
            self.command_focus = false;
        }
        if filter.changed() {
            self.command_index = 0;
        }
        let query = self.filter.trim().to_lowercase();
        let mut results = Vec::new();
        for (index, w) in self.saved.workspaces.iter().enumerate() {
            if w.task.title.to_lowercase().contains(&query)
                || w.task.directory.to_lowercase().contains(&query)
            {
                results.push(CommandAction::Workspace(index));
            }
        }
        for action in [
            CommandAction::Create,
            CommandAction::Overview,
            CommandAction::Split,
            CommandAction::Stack,
            CommandAction::Maximize,
            CommandAction::Rename,
            CommandAction::Settings,
        ] {
            if action.label().to_lowercase().contains(&query) {
                results.push(action);
            }
        }
        self.command_index = self.command_index.min(results.len().saturating_sub(1));
        let mut moved = false;
        if !results.is_empty() {
            if down {
                self.command_index = (self.command_index + 1) % results.len();
                moved = true;
            }
            if up {
                self.command_index = (self.command_index + results.len() - 1) % results.len();
                moved = true;
            }
        }
        ui.add_space(12.0);
        let mut chosen = None;
        let mut was_workspace = None;
        egui::ScrollArea::vertical()
            .id_salt("command-results")
            .max_height((ui.available_height() - 30.0).max(40.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (row, action) in results.iter().enumerate() {
                    let workspace = matches!(action, CommandAction::Workspace(_));
                    if was_workspace != Some(workspace) {
                        section_label(ui, if workspace { "WORKSPACES" } else { "ACTIONS" });
                        ui.add_space(6.0);
                        was_workspace = Some(workspace);
                    }
                    let (title, detail, keys, enabled) = match *action {
                        CommandAction::Workspace(index) => {
                            let w = &self.saved.workspaces[index];
                            (
                                w.task.title.as_str(),
                                w.task.directory.as_str(),
                                if index < 9 {
                                    (index + 1).to_string()
                                } else {
                                    String::new()
                                },
                                true,
                            )
                        }
                        _ => (
                            action.label(),
                            action.detail(),
                            action.keys().to_owned(),
                            !matches!(
                                action,
                                CommandAction::Split
                                    | CommandAction::Stack
                                    | CommandAction::Maximize
                                    | CommandAction::Rename
                            ) || !self.saved.workspaces.is_empty(),
                        ),
                    };
                    let response = ui
                        .add_enabled_ui(enabled, |ui| {
                            command_row(
                                ui,
                                title,
                                detail,
                                &keys,
                                row == self.command_index,
                                ui.available_width(),
                            )
                        })
                        .inner;
                    if moved && row == self.command_index {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                    if enabled
                        && (response.clicked()
                            || (submit == Some(egui::Id::new("workspace-filter"))
                                && row == self.command_index))
                    {
                        chosen = Some(*action);
                    }
                }
                if results.is_empty() {
                    ui.add_space(12.0);
                    ui.label("No matching workspace or command");
                    if submit.is_some() {
                        filter.request_focus();
                    }
                }
            });
        ui.add_space(12.0);
        ui.small("Up / Down navigate     Enter open     Esc close");
        if let Some(action) = chosen {
            match action {
                CommandAction::Workspace(index) => {
                    self.active = index;
                    self.overview = false;
                    self.specifications_view = false;
                    self.maximized = false;
                    self.palette = false;
                }
                CommandAction::Create => {
                    self.command_page = CommandPage::Create;
                    self.command_focus = true;
                }
                CommandAction::Settings => {
                    self.command_page = CommandPage::Settings;
                }
                CommandAction::Overview => {
                    self.overview = if self.specifications_view {
                        true
                    } else {
                        !self.overview
                    };
                    self.specifications_view = false;
                    self.palette = false;
                }
                CommandAction::Split | CommandAction::Stack => {
                    self.split(ctx, action == CommandAction::Split);
                    self.palette = false;
                }
                CommandAction::Maximize => {
                    self.maximized = !self.maximized;
                    self.overview = false;
                    self.specifications_view = false;
                    self.palette = false;
                }
                CommandAction::Rename => self.begin_rename(),
            }
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section_label(ui, "APPEARANCE");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Theme");
            for (light, label) in [(false, "Dark"), (true, "Light")] {
                if ui
                    .selectable_label(self.saved.light == light, label)
                    .clicked()
                {
                    self.saved.light = light;
                    configure_appearance(ctx, light);
                    self.dirty = true;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Terminal text");
            if ui
                .add(egui::Slider::new(&mut self.font_size, 11.0..=24.0).suffix(" px"))
                .changed()
            {
                self.saved.font_size = self.font_size;
                self.dirty = true;
            }
        });
        if let Some(label) =
            ctx.data(|data| data.get_temp::<String>(egui::Id::new("terminal-font-label")))
        {
            ui.label(format!("Font: {label}"));
        }
        ui.add_space(16.0);
        section_label(ui, "TERMINAL BEHAVIOR");
        ui.add_space(8.0);
        let mut confirm = !self.saved.skip_stop_confirmation;
        if ui
            .checkbox(&mut confirm, "Confirm before stopping terminals")
            .changed()
        {
            self.saved.skip_stop_confirmation = !confirm;
            self.dirty = true;
        }
        if ui
            .checkbox(
                &mut self.saved.clickable_codex_choices,
                "Clickable Claude Code and Codex choices",
            )
            .on_hover_text("Click an option to select it. Press Enter to submit.")
            .changed()
        {
            self.dirty = true;
        }
        ui.add_space(16.0);
        section_label(ui, "UPDATES");
        ui.add_space(8.0);
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
                    egui::Checkbox::new(&mut download, "Download updates automatically"),
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
        ui.add_space(16.0);
        egui::CollapsingHeader::new("Keyboard shortcuts").show(ui, |ui| {
            for (label, keys) in [
                ("New terminal", "T"),
                ("Side-by-side split", "D"),
                ("Stacked split", "Shift+D"),
                ("Focus pane in direction", "← / → / ↑ / ↓"),
                ("Overview", "Shift+O"),
                ("Commands", "Shift+P"),
                ("Rename workspace", "Shift+R"),
                ("Find", "F"),
                ("Keyboard selection", "Shift+Space"),
                ("Maximize pane", "Shift+Enter"),
                ("Stop terminal", "W"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    ui.label(
                        RichText::new(shortcut_keys(keys))
                            .monospace()
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            }
        });
    }
    fn prepare_directory(&mut self, input: &str) -> anyhow::Result<String> {
        let (directory, created) = crate::directories::prepare(input)?;
        if created {
            let path = std::fs::canonicalize(&directory)?
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Working directory is not valid UTF-8"))?
                .to_owned();
            if !self
                .saved
                .created_directories
                .iter()
                .any(|entry| entry.path == path)
            {
                self.saved.created_directories.push(CreatedDirectory {
                    path,
                    created_at: now(),
                });
            }
            self.dirty = true;
        }
        Ok(directory)
    }

    fn launch_specification(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        command: &str,
    ) -> anyhow::Result<()> {
        let spec = &self.saved.specifications[index];
        anyhow::ensure!(
            self.saved.workspaces.len() < 32 && self.panes.len() < 32,
            "Workspace/pane limit (32) reached"
        );
        let mut draft = spec.draft.clone();
        anyhow::ensure!(
            !draft.title.trim().is_empty() && !draft.markdown.trim().is_empty(),
            "A title and specification are required"
        );
        draft.directory = crate::directories::resolve(&draft.directory)?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Working directory is not valid UTF-8"))?
            .to_owned();
        anyhow::ensure!(
            spec.launches.len() < 256
                && (spec.revisions.last() == Some(&draft) || spec.revisions.len() < 256),
            "Specification revision/launch limit (256) reached"
        );
        let directory = self.prepare_directory(&draft.directory)?;
        let pane = Uuid::new_v4();
        let socket = self
            .endpoint
            .as_ref()
            .map(|e| e.path.as_path())
            .unwrap_or_else(|| std::path::Path::new(""));
        let terminal = Terminal::spawn_agent(
            pane,
            std::path::Path::new(&directory),
            socket,
            ctx.clone(),
            command,
        )
        .map_err(|error| anyhow::anyhow!("Cannot launch agent: {error}"))?;
        self.panes.insert(
            pane,
            Pane {
                terminal,
                search: TerminalSearch::default(),
            },
        );
        self.saved.workspaces.push(Workspace {
            task: Task {
                id: Uuid::new_v4(),
                title: draft.title.clone(),
                directory,
                state: TaskState::Implementing,
            },
            layout: Layout::Pane(pane),
            focus: pane,
        });
        self.active = self.saved.workspaces.len() - 1;
        let spec = &mut self.saved.specifications[index];
        spec.draft = draft;
        let revision = spec
            .save_revision()
            .expect("revision capacity checked before launch");
        spec.launches.push(crate::specs::Launch {
            pane,
            revision,
            agent: if self.spec_codex { "Codex" } else { "Claude" }.into(),
            exit_code: None,
        });
        spec.status = 2;
        self.overview = false;
        self.specifications_view = false;
        self.dirty = true;
        self.error.clear();
        Ok(())
    }

    fn specifications(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Specifications");
            if ui.button("+ Specification").clicked() {
                let directory = self
                    .saved
                    .workspaces
                    .get(self.active)
                    .map(|w| w.task.directory.clone())
                    .unwrap_or_else(|| self.new_directory.clone());
                let spec = crate::specs::Specification::new(directory);
                self.selected_spec = Some(spec.id);
                self.saved.specifications.push(spec);
                self.dirty = true;
            }
        });
        egui::ScrollArea::vertical().id_salt("spec-board").show(ui, |ui| {
            ui.columns(3, |columns| {
                for (status, label) in ["Draft", "Ready", "In progress"].iter().enumerate() {
                    columns[status].strong(*label);
                    for spec in self.saved.specifications.iter().filter(|s| s.status == status) {
                        if columns[status].selectable_label(self.selected_spec == Some(spec.id), &spec.draft.title).clicked() { self.selected_spec = Some(spec.id); }
                    }
                }
            });
            ui.separator();
            let Some(index) = self.saved.specifications.iter().position(|s| Some(s.id) == self.selected_spec) else { ui.label("Create or select a specification."); return; };
            let mut delete = false;
            ui.menu_button("Delete specification", |ui| {
                ui.label("Delete this specification and its revision history?");
                ui.small("Working directory and agent terminals will be kept.");
                ui.horizontal(|ui| {
                    if ui.button("Confirm deletion").clicked() {
                        delete = true;
                        ui.close();
                    }
                    if ui.button("Cancel").clicked() {
                        ui.close();
                    }
                });
            });
            if delete {
                self.saved.specifications.remove(index);
                self.selected_spec = None;
                self.dirty = true;
                return;
            }
            let spec = &mut self.saved.specifications[index];
            ui.label("Title");
            self.dirty |= ui.text_edit_singleline(&mut spec.draft.title).changed();
            ui.label("Working directory");
            self.dirty |= ui.text_edit_singleline(&mut spec.draft.directory).changed();
            ui.small("Missing directories are created when you launch. ~/ means your home directory.");
            if let Some(alternative) = crate::directories::root_alternative(&spec.draft.directory) {
                ui.small("A leading / points to the filesystem root, not your home directory.");
                if ui.button(format!("Use {alternative}")).clicked() { spec.draft.directory = alternative; self.dirty = true; self.error.clear(); }
            }
            ui.horizontal(|ui| {
                for (status, label) in ["Draft", "Ready", "In progress"].iter().enumerate() {
                    if ui.selectable_label(spec.status == status, *label).clicked() { spec.status = status; self.dirty = true; }
                }
            });
            ui.label("Specification · Markdown");
            self.dirty |= ui.add(egui::TextEdit::multiline(&mut spec.draft.markdown).desired_width(f32::INFINITY).desired_rows(10).code_editor()).changed();
            if ui.button("Save revision").clicked() {
                match spec.save_revision() { Ok(_) => self.dirty = true, Err(e) => self.error = e.to_string() }
            }
            egui::CollapsingHeader::new(format!("Revision history · {}", spec.revisions.len())).show(ui, |ui| {
                let mut restore = None;
                for (i, revision) in spec.revisions.iter().enumerate().rev() {
                    egui::CollapsingHeader::new(format!("Revision {} · {}", i + 1, revision.title)).id_salt((spec.id, i)).show(ui, |ui| {
                        ui.label("Compare this snapshot with the current draft");
                        ui.label(format!("Changed fields: title {}, directory {}, scope {}", revision.title != spec.draft.title, revision.directory != spec.draft.directory, revision.markdown != spec.draft.markdown));
                        ui.columns(2, |columns| {
                            columns[0].strong("Saved revision");
                            columns[0].label(&revision.title);
                            columns[0].label(&revision.directory);
                            columns[0].label(&revision.markdown);
                            columns[1].strong("Current draft");
                            columns[1].label(&spec.draft.title);
                            columns[1].label(&spec.draft.directory);
                            columns[1].label(&spec.draft.markdown);
                        });
                        if ui.add_enabled(spec.proposal.is_none() && revision != &spec.draft, egui::Button::new("Restore this version")).clicked() { restore = Some(i + 1); }
                    });
                }
                if let Some(number) = restore {
                    match spec.restore_revision(number) { Ok(()) => self.dirty = true, Err(e) => self.error = e.to_string() }
                }
            });
            ui.separator();
            if spec.proposal.is_none() && ui.button("Propose a change").clicked() {
                match spec.begin_proposal() { Ok(()) => self.dirty = true, Err(e) => self.error = e.to_string() }
            }
            let current = spec.proposal_is_current();
            if let Some(proposal) = &mut spec.proposal {
                ui.strong(format!("Review proposal against revision {}", proposal.base));
                ui.label("Edit or paste the proposed scope, compare it with the original, then accept or cancel. Running agents keep their original scope.");
                ui.label(format!("Changed fields: title {}, directory {}, scope {}", proposal.replacement.title != proposal.expected_draft.title, proposal.replacement.directory != proposal.expected_draft.directory, proposal.replacement.markdown != proposal.expected_draft.markdown));
                if !current { ui.colored_label(ui.visuals().error_fg_color, "Conflict: the draft or saved revision changed. Discard and prepare a new proposal."); }
                ui.columns(2, |columns| {
                    columns[0].strong("Base revision");
                    columns[0].label(&proposal.expected_draft.title);
                    columns[0].label(&proposal.expected_draft.directory);
                    columns[0].label(&proposal.expected_draft.markdown);
                    columns[1].strong("Proposed revision");
                    self.dirty |= columns[1].text_edit_singleline(&mut proposal.replacement.title).changed();
                    self.dirty |= columns[1].text_edit_singleline(&mut proposal.replacement.directory).changed();
                    self.dirty |= columns[1].add(egui::TextEdit::multiline(&mut proposal.replacement.markdown).desired_width(f32::INFINITY).desired_rows(8).code_editor()).changed();
                });
                let mut accept = None;
                let mut discard = false;
                ui.horizontal(|ui| {
                    if ui.add_enabled(current, egui::Button::new("Accept changes")).clicked() { accept = Some((true, true, true)); }
                    discard = ui.button("Cancel proposal").clicked();
                });
                egui::CollapsingHeader::new("Advanced: accept selected fields").show(ui, |ui| {
                    ui.label("Accepting one field closes the proposal and discards the other proposed fields.");
                    ui.horizontal_wrapped(|ui| {
                        for (label, fields) in [("Title only", (true, false, false)), ("Scope only", (false, true, false)), ("Directory only", (false, false, true))] {
                            if ui.add_enabled(current, egui::Button::new(label)).clicked() { accept = Some(fields); }
                        }
                    });
                });
                if discard { spec.proposal = None; self.dirty = true; }
                else if let Some((title, markdown, directory)) = accept {
                    match spec.accept_proposal(title, markdown, directory) { Ok(()) => self.dirty = true, Err(e) => self.error = e.to_string() }
                }
            }
            ui.separator();
            ui.horizontal(|ui| { ui.strong("Launch context preview"); ui.selectable_value(&mut self.spec_codex, false, "Claude"); ui.selectable_value(&mut self.spec_codex, true, "Codex"); });
            ui.label("The current draft is saved as an immutable revision. The agent starts in a new terminal after you confirm Launch agent.");
            let directory = crate::directories::resolve(&spec.draft.directory);
            let mut launch_draft = spec.draft.clone();
            if let Ok(path) = &directory { launch_draft.directory = path.to_string_lossy().into_owned(); }
            let next = spec.revisions.last().filter(|r| **r == launch_draft).map_or(spec.revisions.len() + 1, |_| spec.revisions.len());
            let command = crate::specs::command(&launch_draft, next, self.spec_codex);
            match &command { Ok(command) => { egui::ScrollArea::vertical().id_salt("launch-preview").max_height(100.0).show(ui, |ui| { ui.label(egui::RichText::new(command).monospace()); }); }, Err(e) => { ui.label(e.to_string()); } }
            if spec.draft.title.trim().is_empty() { ui.colored_label(ui.visuals().error_fg_color, "Enter a specification title."); }
            if spec.draft.markdown.trim().is_empty() { ui.colored_label(ui.visuals().error_fg_color, "Write the specification before launching."); }
            match &directory {
                Ok(path) => { ui.small(format!("Agent working directory: {}", path.display())); },
                Err(error) => { ui.colored_label(ui.visuals().error_fg_color, error.to_string()); }
            }
            let launch = ui.add_enabled(command.is_ok() && !spec.draft.markdown.trim().is_empty() && !spec.draft.title.trim().is_empty() && directory.is_ok(), egui::Button::new("Launch agent")).clicked();
            let mut open_pane = None;
            for launch in spec.launches.iter().rev() {
                ui.horizontal(|ui| {
                    ui.label(format!("{} · revision {} · {}", launch.agent, launch.revision, launch.exit_code.map_or_else(|| "Started".into(), |code| if code == 0 { "Finished".into() } else { format!("Failed (exit {code})") })));
                    if ui.add_enabled(self.panes.contains_key(&launch.pane), egui::Button::new("Open terminal")).clicked() { open_pane = Some(launch.pane); }
                });
            }
            if launch && let Err(error) = self.launch_specification(ui.ctx(), index, command.as_ref().unwrap()) { self.error = error.to_string(); }
            if let Some(pane) = open_pane && let Some(index) = self.saved.workspaces.iter().position(|w| layout_contains(&w.layout, pane)) {
                self.active = index; self.saved.workspaces[index].focus = pane; self.overview = false;
                self.specifications_view = false;
            }
        });
    }
    fn draw(&mut self, ctx: &egui::Context) {
        self.drain();
        if self.discovery.is_some() && !self.panes.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
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
        let mut pane_action = None;
        let wide = ctx.content_rect().width() >= 900.0;
        self.chrome(ctx, wide, &mut pane_action);
        if wide {
            self.workspace_sidebar(ctx, &mut pane_action);
        }
        if let Some((id, action)) = pane_action {
            if action == "Stop terminal" {
                self.request_stop_terminal(id);
            } else if let Some(pane) = self.panes.get_mut(&id) {
                if action == "Select" {
                    pane.search.close();
                    if let Err(error) = pane.terminal.selection_action(SelectionAction::Enter) {
                        self.error = error.to_string();
                    }
                } else {
                    if pane.terminal.copy_mode
                        && let Err(error) = pane.terminal.selection_action(SelectionAction::Exit)
                    {
                        self.error = error.to_string();
                    }
                    pane.search.open();
                }
            }
        }
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(16, 7)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if self.error.is_empty() {
                        let label = if self.overview {
                            format!("{} recorded sessions", self.saved.sessions.len())
                        } else if self.maximized {
                            "Focused pane · maximized".into()
                        } else {
                            "Terminal workspace".into()
                        };
                        ui.label(
                            RichText::new(label)
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.add_space(16.0);
                        for (action, keys) in
                            [("New tab", "T"), ("Find", "F"), ("Commands", "Shift+P")]
                        {
                            status_shortcut(ui, action, keys);
                        }
                    } else {
                        ui.colored_label(ui.visuals().error_fg_color, &self.error);
                        if ui.small_button("Dismiss").clicked() {
                            self.error.clear();
                        }
                    }
                });
            });
        let mut selection_tab = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.extreme_bg_color)
                    .inner_margin(12.0),
            )
            .show(ctx, |ui| {
                if self.specifications_view {
                    self.specifications(ui);
                    return;
                }
                if self.overview {
                    self.overview(ui);
                    return;
                }
                let Some(w) = self.saved.workspaces.get_mut(self.active) else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.heading("A place for your next task");
                        ui.label("Open a terminal workspace to get started.");
                        ui.add_space(16.0);
                        if primary_button(ui, "Create workspace").clicked() {
                            self.open_commands(CommandPage::Create);
                        }
                    });
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
                            .ok_or_else(|| {
                                anyhow::anyhow!("Drop files onto a running terminal pane")
                            })
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
                            if let Some(err) =
                                pane.terminal.error.try_lock().ok().and_then(|e| e.clone())
                            {
                                ui.small(err);
                            }
                            let (clicked, result, new_tab) = terminal_view(
                                ui,
                                &mut pane.terminal,
                                &mut pane.search,
                                focused == id
                                    && !self.palette
                                    && self.rename.is_none()
                                    && self.closing.is_none(),
                                self.font_size,
                                self.saved.clickable_codex_choices
                                    && !self.palette
                                    && self.rename.is_none()
                                    && self.closing.is_none(),
                            );
                            if let Some(text) = new_tab {
                                selection_tab = Some((id, text));
                            }
                            if clicked {
                                self.saved.workspaces[self.active].focus = id;
                                self.dirty = true;
                            }
                            if let Err(e) = result {
                                self.error = e.to_string();
                            }
                        } else {
                            ui.label(
                                "Terminal unavailable. Create a new workspace to start a shell.",
                            );
                        }
                    });
                }
            });
        if let Some((source, text)) = selection_tab {
            self.open_selection_tab(ctx, source, &text);
        }
        if self.palette {
            self.command_center(ctx, workspace_submit, dismiss_palette);
        }
        self.rename_dialog(ctx);
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandAction {
    Workspace(usize),
    Create,
    Overview,
    Split,
    Stack,
    Maximize,
    Rename,
    Settings,
}
impl CommandAction {
    fn label(self) -> &'static str {
        match self {
            Self::Workspace(_) => "Workspace",
            Self::Create => "New workspace",
            Self::Overview => "Toggle Overview",
            Self::Split => "Split side by side",
            Self::Stack => "Split stacked",
            Self::Maximize => "Maximize / restore pane",
            Self::Rename => "Rename workspace",
            Self::Settings => "Settings",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Self::Workspace(_) => "",
            Self::Create => "Choose a directory and an optional name",
            Self::Overview => "Inspect agent sessions and activity",
            Self::Split | Self::Stack => "Open another shell in the current directory",
            Self::Maximize => "Focus on one terminal",
            Self::Rename => "Change the current workspace name",
            Self::Settings => "Appearance, terminal behavior, and updates",
        }
    }
    fn keys(self) -> &'static str {
        match self {
            Self::Overview => "Shift+O",
            Self::Split => "D",
            Self::Stack => "Shift+D",
            Self::Maximize => "Shift+Enter",
            Self::Rename => "Shift+R",
            _ => "",
        }
    }
}
fn layout_contains(layout: &Layout, pane: Uuid) -> bool {
    match layout {
        Layout::Pane(id) => *id == pane,
        Layout::Split { a, b, .. } => layout_contains(a, pane) || layout_contains(b, pane),
    }
}
fn relative_time(seconds: u64) -> String {
    if seconds < 5 {
        "just now".into()
    } else if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    }
}
fn attention_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(239, 192, 127)
    } else {
        Color32::from_rgb(134, 77, 14)
    }
}
fn directory_name(directory: &str) -> &str {
    std::path::Path::new(directory)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(directory)
}
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(10.0)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
}
fn mosaic_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
    for (offset, size, color) in [
        (Vec2::ZERO, Vec2::new(14.0, 7.0), accent(ui)),
        (Vec2::new(16.0, 0.0), Vec2::splat(7.0), attention_color(ui)),
        (Vec2::new(8.0, 9.0), Vec2::new(7.0, 14.0), accent(ui)),
    ] {
        ui.painter()
            .rect_filled(Rect::from_min_size(rect.min + offset, size), 1.0, color);
    }
}

fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .strong()
                .color(Color32::from_rgb(15, 32, 29)),
        )
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(140.0, 36.0)),
    )
}
fn command_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    keys: &str,
    selected: bool,
    width: f32,
) -> egui::Response {
    let response = ui.add(
        egui::Button::new(())
            .min_size(Vec2::new(
                width,
                if detail.is_empty() { 36.0 } else { 54.0 },
            ))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            format!("{title}, {detail}"),
        )
    });
    if !ui.is_rect_visible(response.rect) {
        return response;
    }
    let visuals = ui.style().interact_selectable(&response, selected);
    if selected || response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(response.rect, 4.0, visuals.weak_bg_fill);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            4.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    let painter = ui.painter().with_clip_rect(response.rect.shrink(8.0));
    let shortcut = if keys.is_empty() {
        String::new()
    } else {
        shortcut_keys(keys)
    };
    let key_galley = painter.layout_no_wrap(
        shortcut,
        FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
    let key_width = if keys.is_empty() {
        0.0
    } else {
        key_galley.size().x + 24.0
    };
    let width = (response.rect.width() - 24.0 - key_width).max(20.0);
    let line = |text: &str, size: f32, color: Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(
            text.to_owned(),
            FontId::proportional(size),
            color,
        );
        job.wrap.max_width = width;
        job.wrap.max_rows = 1;
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let title_galley = line(title, 14.0, ui.visuals().text_color());
    let y = if detail.is_empty() {
        response.rect.center().y - title_galley.size().y / 2.0
    } else {
        response.rect.top() + 8.0
    };
    painter.galley(
        Pos2::new(response.rect.left() + 12.0, y),
        title_galley,
        ui.visuals().text_color(),
    );
    if !detail.is_empty() {
        painter.galley(
            Pos2::new(response.rect.left() + 12.0, response.rect.top() + 31.0),
            line(detail, 11.0, ui.visuals().weak_text_color()),
            ui.visuals().weak_text_color(),
        );
    }
    painter.galley(
        Pos2::new(
            response.rect.right() - 12.0 - key_galley.size().x,
            response.rect.center().y - key_galley.size().y / 2.0,
        ),
        key_galley,
        ui.visuals().weak_text_color(),
    );
    response
}

#[cfg(test)]
fn configure_terminal_fonts(ctx: &egui::Context) {
    configure_terminal_fonts_with(ctx, crate::fonts::AutomaticFonts::default());
}

fn configure_terminal_fonts_with(ctx: &egui::Context, detected: crate::fonts::AutomaticFonts) {
    let mut fonts = egui::FontDefinitions::default();
    let mut label = "Automatic (bundled monospace)".to_owned();
    if let Some(primary) = detected.primary {
        label = format!("Automatic ({})", primary.name);
        let mut data = egui::FontData::from_owned(primary.data);
        data.index = primary.index;
        fonts
            .font_data
            .insert("DetectedTerminal".into(), std::sync::Arc::new(data));
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "DetectedTerminal".into());
    }
    if let Some(icons) = detected.icons {
        let mut data = egui::FontData::from_owned(icons.data);
        data.index = icons.index;
        fonts
            .font_data
            .insert("DetectedIcons".into(), std::sync::Arc::new(data));
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("DetectedIcons".into());
    }
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("terminal-font-label"), label));
    const SYMBOLS: &str = "NotoSansSymbols2";
    fonts.font_data.insert(
        SYMBOLS.into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols2-Regular.ttf"
        ))),
    );
    const EMOJI: &str = "NotoEmoji";
    fonts.font_data.insert(
        EMOJI.into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoEmoji.ttf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .extend([SYMBOLS.into(), EMOJI.into()]);
    ctx.set_fonts(fonts);
}

fn configure_appearance(ctx: &egui::Context, light: bool) {
    ctx.set_theme(if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    });
    let mut visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    let (base, surface, raised, border, text, muted, selection) = if light {
        (
            Color32::from_rgb(246, 248, 249),
            Color32::from_rgb(235, 240, 242),
            Color32::WHITE,
            Color32::from_rgb(200, 212, 216),
            Color32::from_rgb(29, 43, 49),
            Color32::from_rgb(85, 103, 113),
            Color32::from_rgb(207, 230, 222),
        )
    } else {
        (
            Color32::from_rgb(18, 23, 30),
            Color32::from_rgb(23, 30, 38),
            Color32::from_rgb(31, 40, 50),
            Color32::from_rgb(53, 66, 79),
            Color32::from_rgb(225, 233, 240),
            Color32::from_rgb(153, 171, 184),
            Color32::from_rgb(35, 62, 59),
        )
    };
    visuals.panel_fill = surface;
    visuals.window_fill = surface;
    visuals.extreme_bg_color = base;
    visuals.faint_bg_color = raised;
    visuals.override_text_color = Some(text);
    visuals.weak_text_color = Some(muted);
    visuals.selection.bg_fill = selection;
    visuals.selection.stroke = Stroke::new(
        1.0_f32,
        if light {
            Color32::from_rgb(27, 108, 87)
        } else {
            ACCENT
        },
    );
    visuals.window_stroke = Stroke::new(1.0_f32, border);
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    visuals.error_fg_color = if light {
        Color32::from_rgb(166, 43, 48)
    } else {
        Color32::from_rgb(245, 151, 150)
    };
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(4);
        widget.fg_stroke = Stroke::new(1.0_f32, text);
        widget.bg_stroke = Stroke::new(1.0_f32, border);
    }
    visuals.widgets.noninteractive.bg_fill = surface;
    visuals.widgets.inactive.bg_fill = raised;
    visuals.widgets.inactive.weak_bg_fill = raised;
    visuals.widgets.hovered.bg_fill = selection;
    visuals.widgets.hovered.weak_bg_fill = selection;
    visuals.widgets.active.bg_fill = selection;
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        style.spacing.item_spacing = Vec2::new(8.0, 7.0);
        style.spacing.interact_size = Vec2::new(36.0, 30.0);
        style.spacing.text_edit_width = 280.0;
        style.spacing.slider_width = 160.0;
        style.animation_time = 0.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(11.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(22.0));
    });
}

fn status_shortcut(ui: &mut egui::Ui, action: &str, keys: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(action).size(12.0));
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(3)
            .inner_margin(egui::Margin::symmetric(5, 2))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(shortcut_keys(keys))
                        .monospace()
                        .size(12.0)
                        .color(ui.visuals().text_color()),
                );
            });
    });
}

fn shortcut(action: &str, keys: &str) -> String {
    format!("{action} — {}", shortcut_keys(keys))
}

fn shortcut_keys(keys: &str) -> String {
    let command = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl+Alt"
    };
    format!("{command}+{keys}")
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

#[derive(Clone)]
struct TerminalPaintCache {
    origin: Pos2,
    font_size: f32,
    shapes: Vec<egui::epaint::Shape>,
    cursor: Vec<egui::epaint::Shape>,
    copying: bool,
    selected: Option<String>,
}

fn terminal_view(
    ui: &mut egui::Ui,
    terminal: &mut Terminal,
    search: &mut TerminalSearch,
    focused: bool,
    font_size: f32,
    clickable_choices: bool,
) -> (bool, anyhow::Result<()>, Option<String>) {
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
    if !ui.is_sizing_pass() && !ui.ctx().will_discard() {
        terminal.resize(Size { cols, rows });
    }
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
    // Reserve primary drags for local selection. Latch the explicit mouse-input
    // override at press time so changing modifiers cannot split a gesture.
    let (primary_pressed, primary_down, primary_released, modifiers, press_origin) =
        ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.modifiers,
                i.pointer.press_origin(),
            )
        });
    let primary_reporting = ui.ctx().data_mut(|data| {
        let reporting = data.get_temp_mut_or_default::<bool>(response.id.with("primary_reporting"));
        if primary_pressed {
            *reporting = !copying
                && !searching
                && mode.intersects(TermMode::MOUSE_MODE)
                && modifiers.alt
                && !modifiers.shift
                && response.contains_pointer()
                && press_origin.is_some_and(|pos| grid_rect.contains(pos));
        } else if !primary_down && !primary_released {
            *reporting = false;
        }
        *reporting
    });
    let mut selected = None;
    let mut choice_prompt = None;
    let mut choice_revision = 0;
    if let Ok(mut term) = terminal.term.try_lock() {
        search.invalidate(terminal.revision());
        mode = *term.mode();
        if !primary_reporting && let Some(pos) = response.interact_pointer_pos() {
            let display_offset = term.grid().display_offset() as i32;
            let point_at = |pos: Pos2| {
                Point::new(
                    Line(
                        ((pos.y - grid_rect.top()) / cell.y)
                            .floor()
                            .clamp(0.0, rows as f32 - 1.0) as i32
                            - display_offset,
                    ),
                    Column(
                        ((pos.x - grid_rect.left()) / cell.x)
                            .floor()
                            .clamp(0.0, cols as f32 - 1.0) as usize,
                    ),
                )
            };
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            {
                term.selection = Some(Selection::new(
                    SelectionType::Simple,
                    point_at(origin),
                    Side::Left,
                ));
            }
            let point = point_at(pos);
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(s) = &mut term.selection
            {
                s.update(point, Side::Right);
            }
            if response.double_clicked_by(egui::PointerButton::Primary) {
                crate::selection::select_word(&mut *term, point);
            } else if response.clicked_by(egui::PointerButton::Primary) {
                term.selection = None;
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
        let paint_start = ui.ctx().graphics(|graphics| {
            graphics
                .get(painter.layer_id())
                .map_or(0, |list| list.all_entries().len())
        });
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
        let cursor_start = ui.ctx().graphics(|graphics| {
            graphics
                .get(painter.layer_id())
                .map_or(paint_start, |list| list.all_entries().len())
        });
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
        let (shapes, cursor) = ui.ctx().graphics(|graphics| {
            let entries = graphics.get(painter.layer_id()).unwrap().all_entries();
            let shapes = entries
                .skip(paint_start)
                .map(|entry| entry.shape.clone())
                .collect::<Vec<_>>();
            let mut shapes = shapes;
            let cursor = shapes.split_off(cursor_start - paint_start);
            (shapes, cursor)
        });
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                response.id.with("paint_cache"),
                TerminalPaintCache {
                    origin: grid_rect.min,
                    font_size,
                    shapes,
                    cursor,
                    copying,
                    selected: selected.clone(),
                },
            )
        });
    } else {
        if let Some(mut cache) = ui
            .ctx()
            .data(|data| data.get_temp::<TerminalPaintCache>(response.id.with("paint_cache")))
            && cache.font_size == font_size
        {
            if focused && !searching && cache.copying == copying {
                cache.shapes.extend(cache.cursor);
            }
            let translation = grid_rect.min - cache.origin;
            for shape in &mut cache.shapes {
                shape.translate(translation);
            }
            ui.painter().with_clip_rect(grid_rect).extend(cache.shapes);
            selected = cache.selected;
        }
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
    } else if response.hovered() && !primary_reporting {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
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
    let mut new_tab = None;
    let menu_id = response.id.with("selection_menu_text");
    if response.secondary_clicked() && !modifiers.alt {
        ui.ctx().data_mut(|data| {
            data.insert_temp(menu_id, selected.clone().unwrap_or_default());
        });
    }
    let mut menu_action = None;
    let menu_was_open = response.context_menu_opened();
    if !modifiers.alt || response.context_menu_opened() {
        response.context_menu(|ui| {
            let text = ui
                .ctx()
                .data(|data| data.get_temp::<String>(menu_id))
                .unwrap_or_default();
            if text.is_empty() {
                ui.add_enabled(false, egui::Label::new("Select text for actions"));
                return;
            }
            for (action, label) in [
                (SelectionMenuAction::Copy, "Copy"),
                (SelectionMenuAction::Paste, "Paste into this terminal"),
                (SelectionMenuAction::NewTab, "Open in new terminal tab"),
                (SelectionMenuAction::Search, "Search in browser"),
            ] {
                let enabled = action != SelectionMenuAction::Paste
                    || (!copying && !searching && terminal.alive.load(Ordering::Acquire));
                let button = ui.add_enabled(enabled, egui::Button::new(label));
                let button = if matches!(
                    action,
                    SelectionMenuAction::Paste | SelectionMenuAction::NewTab
                ) {
                    button.on_hover_text("Paste as one line without pressing Enter")
                } else {
                    button
                };
                if button.clicked() {
                    menu_action = Some((action, text));
                    ui.close();
                    break;
                }
            }
        });
    }
    if let Some((action, text)) = menu_action {
        ui.ctx().data_mut(|data| data.remove::<String>(menu_id));
        match action {
            SelectionMenuAction::Copy => ui.ctx().copy_text(text),
            SelectionMenuAction::Paste => {
                result = selection_paste_text(&text).and_then(|text| terminal.paste(&text));
                response.request_focus();
            }
            SelectionMenuAction::NewTab => new_tab = Some(text),
            SelectionMenuAction::Search => ui
                .ctx()
                .open_url(egui::OpenUrl::new_tab(selection_search_url(&text))),
        }
        return (true, result, new_tab);
    }
    let menu_open = response.context_menu_opened();
    if !menu_open && menu_was_open {
        ui.ctx().data_mut(|data| data.remove::<String>(menu_id));
    }
    let clicked = response.clicked() || response.drag_started() || response.secondary_clicked();
    if searching
        || menu_open
        || menu_was_open
        || (!focused && !clicked)
        || (!response.has_focus() && ui.ctx().wants_keyboard_input())
    {
        return (clicked, result, new_tab);
    }
    if copying {
        let copy_result = copy_input(terminal, ui.input(|i| i.events.clone()));
        return (clicked, result.and(copy_result), new_tab);
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
            } if (if button == egui::PointerButton::Primary {
                primary_reporting
            } else if button == egui::PointerButton::Secondary {
                reporting && modifiers.alt
            } else {
                reporting
            }) && grid_rect.contains(pos) =>
            {
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
    (clicked, result, new_tab)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectionMenuAction {
    Copy,
    Paste,
    NewTab,
    Search,
}

fn selection_paste_text(text: &str) -> anyhow::Result<String> {
    // A selection action stages text, never submits shell commands. Flatten line
    // breaks even when a new shell has not enabled bracketed paste yet.
    let text: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    if text.chars().any(char::is_control) {
        anyhow::bail!("Selected text contains control characters; copy it instead");
    }
    if text.len() > 64 * 1024 - 12 {
        anyhow::bail!("Selected text exceeds the terminal paste limit; select less text");
    }
    Ok(text)
}

fn selection_search_url(text: &str) -> String {
    use std::fmt::Write;
    let mut url = String::from("https://www.google.com/search?q=");
    for byte in text.trim().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            url.push(byte as char);
        } else {
            write!(url, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    url
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
    fn legacy_saved_state_loads_without_specifications() {
        let saved: Saved = serde_json::from_str(r#"{"workspaces":[],"sessions":[]}"#).unwrap();
        assert!(saved.specifications.is_empty());
        assert!(saved.created_directories.is_empty());
    }
    #[test]
    fn saved_appearance_survives_first_frame_and_system_theme_changes() {
        for light in [false, true] {
            let ctx = egui::Context::default();
            configure_appearance(&ctx, light);
            let expected = ctx.style().clone();
            for system_theme in [egui::Theme::Light, egui::Theme::Dark] {
                let _ = ctx.run(
                    egui::RawInput {
                        system_theme: Some(system_theme),
                        ..Default::default()
                    },
                    |ctx| {
                        assert_eq!(ctx.style().visuals.dark_mode, !light);
                        assert_eq!(ctx.style().visuals.panel_fill, expected.visuals.panel_fill);
                        assert_eq!(
                            ctx.style().spacing.button_padding,
                            expected.spacing.button_padding
                        );
                    },
                );
            }
        }
    }

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
    fn directional_navigation_respects_nested_splits_and_edges() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let mut layout = Layout::Pane(a);
        layout.split(a, b, true);
        layout.split(b, c, false);
        assert_eq!(layout.neighbor(a, Key::ArrowRight), Some(b));
        assert_eq!(layout.neighbor(b, Key::ArrowDown), Some(c));
        assert_eq!(layout.neighbor(c, Key::ArrowUp), Some(b));
        assert_eq!(layout.neighbor(c, Key::ArrowLeft), Some(a));
        assert_eq!(layout.neighbor(a, Key::ArrowLeft), None);
        assert_eq!(layout.neighbor(b, Key::ArrowRight), None);
        assert_eq!(layout.neighbor(c, Key::ArrowDown), None);
        if let Layout::Split { b: right, .. } = &mut layout
            && let Layout::Split { ratio, .. } = right.as_mut()
        {
            *ratio = 0.2;
        }
        assert_eq!(layout.neighbor(a, Key::ArrowRight), Some(c));
        layout.remove(b);
        assert_eq!(layout.neighbor(a, Key::ArrowRight), Some(c));
        assert_eq!(Layout::Pane(a).neighbor(a, Key::ArrowRight), None);
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
                for symbol in [
                    '➜',
                    '✗',
                    '✘',
                    '✓',
                    '✔',
                    '\u{1f916}',
                    '\u{1f4b0}',
                    '\u{1f4ca}',
                    '\u{1f4c1}',
                ] {
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
    fn automatic_font_precedes_bundled_fallbacks() {
        let ctx = egui::Context::default();
        let defaults = egui::FontDefinitions::default();
        let primary =
            defaults.font_data[&defaults.families[&egui::FontFamily::Monospace][0]].clone();
        configure_terminal_fonts_with(
            &ctx,
            crate::fonts::AutomaticFonts {
                primary: Some(crate::fonts::LoadedFont {
                    name: "Profile font".into(),
                    data: primary.font.to_vec(),
                    index: primary.index,
                }),
                icons: Some(crate::fonts::LoadedFont {
                    name: "Icon fallback".into(),
                    data: include_bytes!("../assets/fonts/NotoSansSymbols2-Regular.ttf").to_vec(),
                    index: 0,
                }),
            },
        );
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            ctx.fonts_mut(|fonts| {
                let chain = &fonts.definitions().families[&egui::FontFamily::Monospace];
                assert_eq!(chain[0], "DetectedTerminal");
                let icons = chain
                    .iter()
                    .position(|name| name == "DetectedIcons")
                    .unwrap();
                let bundled = chain
                    .iter()
                    .position(|name| name == "NotoSansSymbols2")
                    .unwrap();
                assert!(icons < bundled);
                assert!(fonts.has_glyphs(&FontId::monospace(15.0), "Text \u{1f916}"));
            });
        });
        assert_eq!(
            ctx.data(|data| data.get_temp::<String>(egui::Id::new("terminal-font-label"))),
            Some("Automatic (Profile font)".into())
        );
    }

    #[test]
    fn clickable_choices_default_on_and_preserve_saved_opt_out() {
        assert!(Saved::default().clickable_codex_choices);
        let mut value = serde_json::to_value(Saved::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("clickable_codex_choices");
        let legacy: Saved = serde_json::from_value(value.clone()).unwrap();
        assert!(legacy.clickable_codex_choices);
        value["clickable_codex_choices"] = serde_json::json!(false);
        let opted_out: Saved = serde_json::from_value(value).unwrap();
        assert!(!opted_out.clickable_codex_choices);
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
            specifications_view: false,
            selected_spec: None,
            spec_codex: false,
            selected,
            endpoint: None,
            discovery: None,
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
            command_page: CommandPage::Commands,
            command_index: 0,
            command_focus: false,
            rename: None,
            closing: None,
            filter: String::new(),
            maximized: false,
            category: 0,
            overview_inspector: false,
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
    fn first_hook_upgrades_discovered_session_and_preserves_selection() {
        let ctx = egui::Context::default();
        let mut app = fixture(0);
        let directory = tempfile::tempdir().unwrap();
        app.new_directory = directory.path().to_string_lossy().into_owned();
        assert!(app.add_workspace(&ctx));
        let pane = app.saved.workspaces[0].focus;
        assert!(crate::discovery::reconcile(
            &mut app.saved.sessions,
            &[(pane, 123, crate::model::Agent::Codex)],
            1,
        ));
        app.selected = Some(app.saved.sessions[0].key());
        let (sender, receiver) = sync_channel(1);
        app.endpoint = Some(Endpoint {
            path: directory.path().join("unused.sock"),
            receiver,
        });
        sender
            .send(HookEvent {
                id: Uuid::new_v4(),
                pane,
                sequence: 1,
                session_id: "actual-conversation".into(),
                agent: crate::model::Agent::Codex,
                hook_event_name: "PermissionRequest".into(),
                detail: "Bash".into(),
            })
            .unwrap();
        app.drain();
        assert_eq!(app.saved.sessions.len(), 1);
        let session = &app.saved.sessions[0];
        assert_eq!(session.session_id, "actual-conversation");
        assert_eq!(session.state, SessionState::Permission);
        assert_eq!(session.observed_process, None);
        assert_eq!(app.selected, Some(session.key()));
        assert!(!crate::discovery::reconcile(
            &mut app.saved.sessions,
            &[(pane, 123, crate::model::Agent::Codex)],
            2,
        ));
        for (sequence, kind) in [(2, "SessionEnd"), (3, "PermissionRequest")] {
            if sequence == 3 {
                assert!(crate::discovery::reconcile(
                    &mut app.saved.sessions,
                    &[(pane, 123, crate::model::Agent::Codex)],
                    3,
                ));
                app.selected = Some(app.saved.sessions[1].key());
            }
            sender
                .send(HookEvent {
                    id: Uuid::new_v4(),
                    pane,
                    sequence,
                    session_id: "actual-conversation".into(),
                    agent: crate::model::Agent::Codex,
                    hook_event_name: kind.into(),
                    detail: String::new(),
                })
                .unwrap();
            app.drain();
        }
        assert_eq!(app.saved.sessions.len(), 1);
        assert_eq!(app.saved.sessions[0].history.len(), 3);
        assert_eq!(app.selected, Some(app.saved.sessions[0].key()));
    }

    #[test]
    fn workspace_sidebar_drag_resizes_and_persists_width() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        let render = |app: &mut App, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                    events,
                    ..Default::default()
                },
                |ctx| app.workspace_sidebar(ctx, &mut None),
            )
        };
        render(&mut app, vec![]);
        let initial = app.saved.sidebar_width;
        let edge = Pos2::new(initial, 200.0);
        render(
            &mut app,
            vec![
                egui::Event::PointerMoved(edge),
                egui::Event::PointerButton {
                    pos: edge,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
        let target = edge + Vec2::new(90.0, 0.0);
        render(&mut app, vec![egui::Event::PointerMoved(target)]);
        render(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(app.saved.sidebar_width > initial + 60.0);
        assert!(app.dirty);
        let saved: Saved =
            serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
        assert_eq!(saved.sidebar_width, app.saved.sidebar_width);
        let fresh = egui::Context::default();
        let _ = fresh.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                ..Default::default()
            },
            |ctx| app.workspace_sidebar(ctx, &mut None),
        );
        assert_eq!(saved.sidebar_width, app.saved.sidebar_width);
    }

    #[test]
    fn chrome_navigation_remains_clickable_and_fits_narrow_windows() {
        for width in [640.0, 900.0, 1180.0] {
            let ctx = egui::Context::default();
            configure_appearance(&ctx, true);
            let mut app = fixture(1);
            app.saved.workspaces[0].task.title =
                "A long workspace name that must be truncated in the compact navigation bar".into();
            app.overview = false;
            app.saved.sessions[0].state = SessionState::Input;
            let frame = |app: &mut App, events| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 400.0))),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.chrome(ctx, width >= 900.0, &mut None),
                )
            };
            let out = frame(&mut app, vec![]);
            let pos = out
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == "Specs" => {
                        Some(text.pos + Vec2::new(5.0, 5.0))
                    }
                    _ => None,
                })
                .unwrap();
            for shape in &out.shapes {
                if let egui::epaint::Shape::Text(text) = &shape.shape {
                    assert!(
                        text.pos.x + text.galley.size().x <= width,
                        "{} clipped",
                        text.galley.text()
                    );
                    if cfg!(target_os = "macos") {
                        assert!(
                            text.pos.x >= 80.0,
                            "native controls overlap {}",
                            text.galley.text()
                        );
                    }
                }
            }
            for pressed in [true, false] {
                let out = frame(
                    &mut app,
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
                assert!(!out.viewport_output.values().any(|viewport| {
                    viewport
                        .commands
                        .contains(&egui::ViewportCommand::StartDrag)
                }));
            }
            assert!(app.specifications_view);
            assert!(
                !ctx.style().visuals.dark_mode,
                "chrome theme leaked into content"
            );
        }
    }

    #[test]
    fn chrome_controls_have_matching_text_size_and_spacing() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.overview = false;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                ..Default::default()
            },
            |ctx| app.chrome(ctx, true, &mut None),
        );
        let controls: Vec<_> = ["Maximize", "Split", "Commands", "+ Workspace"]
            .iter()
            .map(|label| {
                out.shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == *label => {
                            Some((text.pos + text.galley.size() / 2.0, text.galley.size().y))
                        }
                        _ => None,
                    })
                    .unwrap()
            })
            .collect();
        for (center, height) in &controls {
            assert!((center.y - controls[0].0.y).abs() < 1.0);
            assert!((height - controls[0].1).abs() < 1.0);
        }
        for pair in controls.windows(2) {
            assert!(((pair[1].0.x - pair[0].0.x).abs() - 104.0).abs() < 1.0);
        }
    }

    #[test]
    fn chrome_workspace_name_keeps_maximize_accessible() {
        for width in [640.0, 1180.0] {
            let ctx = egui::Context::default();
            let mut app = fixture(1);
            app.overview = false;
            app.saved.workspaces[0].task.title = "My task".into();
            let frame = |app: &mut App, events| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 400.0))),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.chrome(ctx, width >= 900.0, &mut None),
                )
            };
            let text_pos = |out: &egui::FullOutput, label: &str| {
                out.shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                            Some(text.pos + Vec2::new(5.0, 5.0))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "missing {label}: {:?}",
                            out.shapes
                                .iter()
                                .filter_map(|shape| match &shape.shape {
                                    egui::epaint::Shape::Text(text) => Some(text.galley.text()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                        )
                    })
            };
            let click = |app: &mut App, pos| {
                let _ = frame(
                    app,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                );
                frame(
                    app,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    }],
                )
            };
            let mut out = frame(&mut app, vec![]);
            let name = text_pos(&out, "My task");
            assert!(
                name.y < 40.0,
                "workspace name did not move into the top bar"
            );
            if width < 900.0 {
                click(&mut app, name);
                out = frame(&mut app, vec![]);
            }
            let action = text_pos(
                &out,
                if width < 900.0 {
                    "Maximize pane"
                } else {
                    "Maximize"
                },
            );
            click(&mut app, action);
            assert!(app.maximized);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chrome_background_starts_native_window_drag() {
        let ctx = egui::Context::default();
        let mut app = fixture(0);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 400.0))),
                ..Default::default()
            },
            |ctx| app.chrome(ctx, true, &mut None),
        );
        let mut drag_sent = false;
        for (pos, pressed) in [
            (Pos2::new(700.0, 20.0), Some(true)),
            (Pos2::new(730.0, 20.0), None),
        ] {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 400.0))),
                    events,
                    ..Default::default()
                },
                |ctx| app.chrome(ctx, true, &mut None),
            );
            drag_sent |= out.viewport_output.values().any(|viewport| {
                viewport
                    .commands
                    .contains(&egui::ViewportCommand::StartDrag)
            });
        }
        assert!(drag_sent);
    }

    #[test]
    fn specification_workspace_click_returns_to_terminal() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.specifications_view = true;
        let frame = |app: &mut App, events| {
            ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| app.workspace_navigation(ui, true, &mut None));
                },
            )
        };
        let out = frame(&mut app, vec![]);
        let pos = out
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text().starts_with("1   ") => {
                    Some(text.pos + Vec2::new(10.0, 5.0))
                }
                _ => None,
            })
            .unwrap();
        frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
        frame(
            &mut app,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(!app.specifications_view);
        assert!(!app.overview);
    }

    #[test]
    fn specification_failed_launch_retains_output_and_reports_failure() {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = fixture(0);
        let pane = Uuid::new_v4();
        let terminal = Terminal::spawn_agent(
            pane,
            directory.path(),
            std::path::Path::new(""),
            ctx,
            "tessera_deliberately_missing_agent",
        )
        .unwrap();
        app.panes.insert(
            pane,
            Pane {
                terminal,
                search: TerminalSearch::default(),
            },
        );
        let mut spec =
            crate::specs::Specification::new(directory.path().to_string_lossy().into_owned());
        spec.status = 2;
        spec.launches.push(crate::specs::Launch {
            pane,
            revision: 1,
            agent: "Claude".into(),
            exit_code: None,
        });
        app.saved.specifications.push(spec);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !app.panes[&pane].terminal.has_exited() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        app.drain();
        assert!(app.panes.contains_key(&pane));
        assert_eq!(app.saved.specifications[0].launches[0].exit_code, Some(127));
        assert_eq!(app.saved.specifications[0].status, 1);
        assert!(app.error.contains("exited with code 127"));
        app.error.clear();
        app.drain();
        assert!(app.error.is_empty(), "dismissed error must not reappear");
    }

    #[test]
    fn cleanup_disconnected_sessions_preserves_other_states_and_workspaces() {
        let mut app = fixture(1);
        app.saved.sessions.clear();
        for state in [
            SessionState::Disconnected,
            SessionState::Running,
            SessionState::Idle,
            SessionState::Permission,
            SessionState::Input,
            SessionState::Unknown,
            SessionState::Ended,
        ] {
            for agent in [crate::model::Agent::Claude, crate::model::Agent::Codex] {
                let mut session = Session::new(Uuid::new_v4(), "cleanup".into());
                session.state = state;
                session.agent = agent;
                app.saved.sessions.push(session);
            }
        }
        app.selected = Some(app.saved.sessions[0].key());
        app.dirty = false;
        app.cleanup_disconnected_sessions();
        assert_eq!(app.saved.sessions.len(), 12);
        assert!(
            app.saved
                .sessions
                .iter()
                .all(|session| session.state != SessionState::Disconnected)
        );
        assert!(app.selected.is_none());
        assert!(app.dirty);
        assert_eq!(app.saved.workspaces.len(), 1);
        app.selected = Some(app.saved.sessions[0].key());
        app.dirty = false;
        app.cleanup_disconnected_sessions();
        assert!(app.selected.is_some());
        assert!(!app.dirty);
    }

    #[test]
    fn specification_deletion_requires_confirmation_and_preserves_workspaces() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        let directory = tempfile::tempdir().unwrap();
        let spec = crate::specs::Specification::new(directory.path().display().to_string());
        app.selected_spec = Some(spec.id);
        app.saved.specifications.push(spec);
        app.dirty = false;
        let render = |app: &mut App, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 900.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.specifications(ui));
                },
            )
        };
        let position = |out: &egui::FullOutput, label: &str| {
            out.shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.pos + text.galley.size() / 2.0)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("Missing control: {label}"))
        };
        let click = |app: &mut App, pos| {
            render(
                app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            render(
                app,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            )
        };
        let out = render(&mut app, vec![]);
        click(&mut app, position(&out, "Delete specification"));
        let out = render(&mut app, vec![]);
        assert_eq!(app.saved.specifications.len(), 1);
        assert!(!app.dirty);
        click(&mut app, position(&out, "Cancel"));
        assert_eq!(app.saved.specifications.len(), 1);
        let out = render(&mut app, vec![]);
        click(&mut app, position(&out, "Delete specification"));
        let out = render(&mut app, vec![]);
        click(&mut app, position(&out, "Confirm deletion"));
        assert!(app.saved.specifications.is_empty());
        assert!(app.selected_spec.is_none());
        assert!(app.dirty);
        assert_eq!(app.saved.workspaces.len(), 1);
        assert!(directory.path().is_dir());
        let restored: Saved =
            serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
        assert!(restored.specifications.is_empty());
    }

    #[test]
    fn proposal_review_keeps_primary_actions_simple_and_reports_conflicts() {
        for width in [640.0, 1180.0] {
            let ctx = egui::Context::default();
            let mut app = fixture(0);
            let mut spec = crate::specs::Specification::new("/tmp".into());
            spec.draft.markdown = "Original scope".into();
            spec.begin_proposal().unwrap();
            app.selected_spec = Some(spec.id);
            app.saved.specifications.push(spec);
            for conflicted in [false, true] {
                if conflicted {
                    app.saved.specifications[0].draft.markdown = "New edit".into();
                }
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(width, 1800.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| app.specifications(ui));
                    },
                );
                let text: Vec<_> = out
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect();
                assert!(text.iter().any(|t| t.contains("Accept changes")));
                assert!(text.iter().any(|t| t.contains("Cancel proposal")));
                assert!(!text.contains(&"Scope only"));
                assert_eq!(text.iter().any(|t| t.contains("Conflict:")), conflicted);
            }
        }
    }
    #[test]
    fn created_directory_registry_persists_without_claiming_existing_folders() {
        let ctx = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        let folder = root.path().join("new project");
        let mut app = fixture(0);
        app.prepare_directory(existing.to_str().unwrap()).unwrap();
        assert!(app.saved.created_directories.is_empty());
        app.prepare_directory(folder.to_str().unwrap()).unwrap();
        app.prepare_directory(folder.to_str().unwrap()).unwrap();
        assert_eq!(app.saved.created_directories.len(), 1);
        assert_eq!(
            PathBuf::from(&app.saved.created_directories[0].path),
            folder.canonicalize().unwrap()
        );
        assert!(app.saved.created_directories[0].created_at > 0);
        app.saved = serde_json::from_slice(&serde_json::to_vec(&app.saved).unwrap()).unwrap();
        app.new_directory = existing.to_string_lossy().into_owned();
        app.start_fresh_workspace(&ctx);
        assert_eq!(app.saved.created_directories.len(), 1);
        assert!(folder.is_dir());
    }

    #[test]
    fn specification_launch_creates_directory_and_starts_in_it() {
        let ctx = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("new project/nested");
        let output = root.path().join("agent-directory");
        let mut app = fixture(0);
        let mut spec = crate::specs::Specification::new(format!(" {} ", folder.display()));
        spec.draft.markdown = "Create a project".into();
        app.saved.specifications.push(spec);
        app.launch_specification(
            &ctx,
            0,
            &format!("printf '%s' \"$PWD\" > '{}'", output.display()),
        )
        .unwrap();
        assert!(folder.is_dir());
        assert_eq!(app.saved.created_directories.len(), 1);
        assert_eq!(
            app.saved.workspaces[0].task.directory,
            folder.to_str().unwrap()
        );
        let spec = &app.saved.specifications[0];
        assert_eq!(spec.status, 2);
        assert_eq!(spec.revisions.len(), 1);
        assert_eq!(spec.revisions[0].directory, folder.to_str().unwrap());
        assert_eq!(spec.launches[0].revision, 1);
        let pane = spec.launches[0].pane;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !app.panes[&pane].terminal.has_exited() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture agent did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let actual = PathBuf::from(std::fs::read_to_string(output).unwrap());
        assert_eq!(
            actual.canonicalize().unwrap(),
            folder.canonicalize().unwrap()
        );
    }

    #[test]
    fn specification_directory_failure_preserves_draft_and_limits_prevent_creation() {
        let ctx = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "keep").unwrap();
        let mut app = fixture(0);
        let mut spec =
            crate::specs::Specification::new(file.join("child").to_string_lossy().into_owned());
        spec.draft.markdown = "Scope".into();
        let original = spec.draft.clone();
        app.saved.specifications.push(spec);
        let error = app.launch_specification(&ctx, 0, "true").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Cannot create working directory")
        );
        assert!(app.panes.is_empty());
        assert!(app.saved.workspaces.is_empty());
        assert_eq!(
            app.saved.specifications[0].draft.directory,
            original.directory
        );
        assert!(app.saved.specifications[0].revisions.is_empty());
        assert!(app.saved.specifications[0].launches.is_empty());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
        let folder = root.path().join("over limit");
        app.saved.specifications[0].draft.directory = folder.to_string_lossy().into_owned();
        app.saved.workspaces = fixture(32).saved.workspaces;
        assert!(
            app.launch_specification(&ctx, 0, "true")
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert!(!folder.exists());
    }

    #[test]
    fn specification_empty_directory_explains_disabled_launch() {
        let ctx = egui::Context::default();
        let mut app = fixture(0);
        let mut spec = crate::specs::Specification::new(" ".into());
        spec.draft.markdown = "Scope".into();
        app.selected_spec = Some(spec.id);
        app.saved.specifications.push(spec);
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 1400.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.specifications(ui));
            },
        );
        assert!(out.shapes.iter().any(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text().contains("Enter a working directory"))));
    }

    #[test]
    fn specification_shortcuts_keep_editor_input_out_of_terminals() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.specifications_view = true;
        let original = app.saved.workspaces[0].task.title.clone();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::R, None, command() | Modifiers::SHIFT)],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert!(app.rename.is_none());
        assert_eq!(app.saved.workspaces[0].task.title, original);
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::O, None, command() | Modifiers::SHIFT)],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert!(!app.specifications_view);
        assert!(app.overview);
    }
    #[test]
    fn overview_renders_both_themes_and_narrow_windows() {
        for (name, width, height, light) in [
            ("specs-dark", 1180.0, 760.0, false),
            ("specs-light", 640.0, 700.0, true),
            ("specs-short", 640.0, 400.0, false),
            ("dark", 1180.0, 760.0, false),
            ("light", 1180.0, 760.0, true),
            ("narrow", 640.0, 700.0, false),
            ("terminal-light", 1180.0, 760.0, true),
            ("terminal-dark", 640.0, 700.0, false),
            ("terminal-wide", 1180.0, 760.0, false),
            ("commands-dark", 1180.0, 760.0, false),
            ("commands-light", 640.0, 700.0, true),
            ("create-dark", 640.0, 400.0, false),
            ("settings-light", 1180.0, 760.0, true),
            ("overview-short", 640.0, 400.0, false),
        ] {
            let ctx = egui::Context::default();
            configure_appearance(&ctx, light);
            let mut app = fixture(6);
            let directory = tempfile::tempdir().unwrap();
            if name.starts_with("specs-") {
                let mut spec = crate::specs::Specification::new(
                    directory.path().to_string_lossy().into_owned(),
                );
                spec.draft.title = "Add project navigation".into();
                spec.draft.markdown = "## Goal\nMake project navigation accessible.\n\n## Acceptance\n- Keyboard navigation works\n- Existing routes remain available".into();
                spec.save_revision().unwrap();
                spec.begin_proposal().unwrap();
                spec.proposal.as_mut().unwrap().replacement.markdown = "## Goal\nMake navigation accessible and intuitive.\n\n## Acceptance\n- Keyboard navigation works\n- Existing routes remain available".into();
                app.selected_spec = Some(spec.id);
                app.saved.specifications.push(spec);
                app.specifications_view = true;
            }
            if name.starts_with("terminal-") {
                app.overview = false;
                app.saved.workspaces[0].task.directory =
                    directory.path().to_string_lossy().into_owned();
                start_fixture_terminal(&mut app, &ctx);
            }
            if name.starts_with("commands-") {
                app.open_commands(CommandPage::Commands);
            }
            if name.starts_with("create-") {
                app.open_commands(CommandPage::Create);
                app.new_directory = "/Users/developer/projects/tessera".into();
            }
            if name.starts_with("settings-") {
                app.open_commands(CommandPage::Settings);
            }
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height))),
                ..Default::default()
            };
            app.saved.light = light;
            let first = ctx.run(input.clone(), |ctx| app.draw(ctx));
            let second = ctx.run(input.clone(), |ctx| app.draw(ctx));
            let third = ctx.run(input.clone(), |ctx| app.draw(ctx));
            let mut out = ctx.run(input, |ctx| app.draw(ctx));
            let mut textures = first.textures_delta;
            textures.append(second.textures_delta);
            textures.append(third.textures_delta);
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
    fn directional_shortcut_consumes_arrow_and_preserves_maximized_view() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.overview = false;
        let a = app.saved.workspaces[0].focus;
        let b = Uuid::new_v4();
        app.saved.workspaces[0].layout.split(a, b, true);
        app.maximized = true;
        let _ = ctx.run(
            egui::RawInput {
                modifiers: command(),
                events: vec![key_event(Key::ArrowRight, None, command())],
                ..Default::default()
            },
            |ctx| {
                app.shortcuts(ctx);
                assert!(!ctx.input(|input| input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: Key::ArrowRight,
                        pressed: true,
                        ..
                    }
                ))));
            },
        );
        assert_eq!(app.saved.workspaces[0].focus, b);
        assert!(app.maximized);
        app.palette = true;
        let _ = ctx.run(
            egui::RawInput {
                modifiers: command(),
                events: vec![key_event(Key::ArrowLeft, None, command())],
                ..Default::default()
            },
            |ctx| app.shortcuts(ctx),
        );
        assert_eq!(app.saved.workspaces[0].focus, b);
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
        app.command_page = CommandPage::Settings;
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
        choice_click_regression(false);
    }

    #[test]
    fn clicking_claude_choice_sends_arrow_without_submitting() {
        choice_click_regression(true);
    }

    fn choice_click_regression(claude: bool) {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choice-input");
        let mut terminal = Terminal::spawn_test(
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
        let prompt = if claude {
            "Which option?\\r\\nChoose an option.\\r\\n ❯ 1. First\\r\\n   2. Second\\r\\n\\r\\nEnter to select · ↑/↓ to navigate · Esc to cancel"
        } else {
            "Question 1/1 (1 unanswered)\\r\\nChoose an option.\\r\\n › 1. First\\r\\n   2. Second\\r\\n\\r\\ntab to add notes | enter to submit answer | esc to interrupt"
        };
        terminal.input(format!("stty raw -echo; printf '\\033[?1049h\\033[2J\\033[H{prompt}'; dd bs=1 count=3 of='{}' 2>/dev/null; stty sane\r", path.display()).into_bytes()).unwrap();
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
    fn primary_drag_selects_mouse_reporting_text_without_copy_mode() {
        pointer_selection_regression(false);
    }

    #[test]
    fn alt_click_preserves_application_mouse_input() {
        pointer_selection_regression(true);
    }

    fn pointer_selection_regression(reporting: bool) {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pointer-input");
        let mut terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            dir.path(),
            std::path::Path::new(""),
            ctx.clone(),
        )
        .unwrap();
        let mut search = TerminalSearch::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0));
        let mut start = Pos2::ZERO;
        let mut end = Pos2::ZERO;
        let mut frame = |terminal: &mut Terminal, events, modifiers| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    modifiers,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let font = FontId::monospace(15.0);
                        let cell = ui.fonts_mut(|f| {
                            Vec2::new(f.glyph_width(&font, 'M'), f.row_height(&font))
                        });
                        let grid = ui.available_rect_before_wrap().shrink(8.0);
                        start = grid.min + Vec2::new(0.25 * cell.x, 0.5 * cell.y);
                        end = grid.min + Vec2::new(6.25 * cell.x, 0.5 * cell.y);
                        terminal_view(ui, terminal, &mut search, true, 15.0, true)
                            .1
                            .unwrap();
                    });
                },
            )
        };
        frame(&mut terminal, vec![], Modifiers::NONE);
        let count = if reporting { 18 } else { 1 };
        terminal.input(format!("stty raw -echo; printf '\\033[?1049h\\033[?1000h\\033[?1006h\\033[2J\\033[Hselectable text'; dd bs=1 count={count} of='{}' 2>/dev/null; stty sane\r", path.display()).into_bytes()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !terminal
            .mode()
            .contains(TermMode::ALT_SCREEN | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE)
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        // Resolve the cell coordinates before releasing the frame closure's borrows.
        frame(&mut terminal, vec![], Modifiers::NONE);
        drop(frame);
        let frame = |terminal: &mut Terminal, search: &mut TerminalSearch, events, modifiers| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    modifiers,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        terminal_view(ui, terminal, search, true, 15.0, true)
                            .1
                            .unwrap();
                    });
                },
            )
        };
        let modifiers = if reporting {
            Modifiers::ALT
        } else {
            Modifiers::NONE
        };
        frame(
            &mut terminal,
            &mut search,
            vec![egui::Event::PointerMoved(start)],
            modifiers,
        );
        frame(
            &mut terminal,
            &mut search,
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            }],
            modifiers,
        );
        if !reporting {
            // A fast drag must anchor at the press, even if Alt changes mid-gesture.
            frame(
                &mut terminal,
                &mut search,
                vec![egui::Event::PointerMoved(end)],
                Modifiers::ALT,
            );
        }
        let release = if reporting { start } else { end };
        frame(
            &mut terminal,
            &mut search,
            vec![egui::Event::PointerButton {
                pos: release,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
        );
        assert!(!terminal.copy_mode);
        if reporting {
            assert!(terminal.term.lock().unwrap().selection.is_none());
        } else {
            assert_eq!(
                terminal
                    .term
                    .lock()
                    .unwrap()
                    .selection_to_string()
                    .as_deref(),
                Some("selecta")
            );
            let output = frame(
                &mut terminal,
                &mut search,
                vec![egui::Event::Copy],
                Modifiers::NONE,
            );
            assert!(output.platform_output.commands.iter().any(|command| {
                matches!(command, egui::OutputCommand::CopyText(text) if text == "selecta")
            }));
            for pressed in [true, false] {
                frame(
                    &mut terminal,
                    &mut search,
                    vec![egui::Event::PointerButton {
                        pos: end,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    }],
                    Modifiers::NONE,
                );
            }
            let output = frame(&mut terminal, &mut search, vec![], Modifiers::NONE);
            let menu_button = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                            Some(text.pos + text.galley.rect.center().to_vec2())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("Missing selection menu action: {label}"))
            };
            for label in [
                "Copy",
                "Paste into this terminal",
                "Open in new terminal tab",
                "Search in browser",
            ] {
                assert!(screen.contains(menu_button(label)));
            }
            let pos = menu_button("Search in browser");
            // The menu keeps the original text even if output clears selection.
            terminal.term.lock().unwrap().selection = None;
            frame(
                &mut terminal,
                &mut search,
                vec![egui::Event::PointerMoved(pos)],
                Modifiers::NONE,
            );
            frame(
                &mut terminal,
                &mut search,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                }],
                Modifiers::NONE,
            );
            let output = frame(
                &mut terminal,
                &mut search,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                }],
                Modifiers::NONE,
            );
            assert!(output.platform_output.commands.iter().any(|command| {
                matches!(command, egui::OutputCommand::OpenUrl(url) if url.url == "https://www.google.com/search?q=selecta")
            }));
            terminal.input(b"x".to_vec()).unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !std::fs::read(&path).is_ok_and(|bytes| bytes.len() == count) {
            assert!(
                std::time::Instant::now() < deadline,
                "pointer input was lost or leaked into PTY"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read(path).unwrap(),
            if reporting {
                b"\x1b[<0;1;1M\x1b[<0;1;1m".as_slice()
            } else {
                b"x".as_slice()
            }
        );
    }

    #[test]
    fn selection_actions_encode_search_and_stage_safe_terminal_text() {
        assert_eq!(
            selection_search_url("  Rust & café\n#?  "),
            "https://www.google.com/search?q=Rust%20%26%20caf%C3%A9%0A%23%3F"
        );
        assert_eq!(
            selection_paste_text("echo first\necho second\r\t").unwrap(),
            "echo first echo second  "
        );
        assert!(selection_paste_text("echo\u{1b}[31m").is_err());
        assert!(selection_paste_text("echo\u{0}").is_err());
        assert!(selection_paste_text(&"x".repeat(64 * 1024)).is_err());
    }

    #[test]
    fn selection_tab_uses_source_directory_without_executing_text() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(1);
        let source = app.saved.workspaces[0].focus;
        assert!(app.spawn(source, &ctx, dir.path().to_str().unwrap()));
        let marker = dir.path().join("must-not-run");
        let text = format!("printf unsafe > '{}'\necho second", marker.display());
        app.open_selection_tab(&ctx, source, &text);
        assert!(app.error.is_empty(), "{}", app.error);
        assert_eq!(app.saved.workspaces.len(), 2);
        assert_eq!(
            PathBuf::from(&app.saved.workspaces[1].task.directory)
                .canonicalize()
                .unwrap(),
            dir.path().canonicalize().unwrap()
        );
        let target = app.saved.workspaces[1].focus;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let terminal = &app.panes[&target].terminal;
            let term = terminal.term.lock().unwrap();
            let output: String = term.grid().display_iter().map(|cell| cell.cell.c).collect();
            if output.contains("echo second") {
                break;
            }
            drop(term);
            assert!(
                std::time::Instant::now() < deadline,
                "Selected text did not reach new tab"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !marker.exists(),
            "Selection action submitted a shell command"
        );
        app.open_selection_tab(&ctx, source, "bad\u{1b}text");
        assert_eq!(app.saved.workspaces.len(), 2);
        assert!(app.error.contains("control characters"));
    }

    #[test]
    fn trackpad_gesture_reaches_mouse_application_through_pty() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wheel-input");
        let mut terminal = Terminal::spawn_test(
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
    fn workspace_creation_makes_missing_directories_and_preserves_state_on_failure() {
        let ctx = egui::Context::default();
        let mut app = fixture(0);
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("new project").join("nested");
        app.new_directory = format!("  {}  ", directory.display());
        app.new_workspace_name = "  My workspace  ".into();
        assert!(app.add_workspace(&ctx));
        assert!(directory.is_dir());
        assert_eq!(app.saved.created_directories.len(), 1);
        assert_eq!(app.saved.workspaces[0].task.title, "My workspace");
        assert_eq!(
            app.saved.workspaces[0].task.directory,
            directory.to_str().unwrap()
        );
        assert_eq!(
            app.panes[&app.saved.workspaces[0].focus]
                .terminal
                .current_directory()
                .unwrap(),
            directory.canonicalize().unwrap()
        );

        let file = root.path().join("file");
        std::fs::write(&file, "occupied").unwrap();
        app.new_directory = file.join("child").to_string_lossy().into_owned();
        app.new_workspace_name = "Keep this name".into();
        assert!(!app.add_workspace(&ctx));
        assert_eq!(app.saved.workspaces.len(), 1);
        assert_eq!(app.panes.len(), 1);
        assert_eq!(app.active, 0);
        assert_eq!(app.new_workspace_name, "Keep this name");
        assert!(app.error.contains("Cannot create working directory"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "occupied");

        app.new_directory = "   ".into();
        assert!(!app.add_workspace(&ctx));
        assert_eq!(app.error, "Enter a working directory");
        app.new_directory = directory.to_string_lossy().into_owned();
        assert!(app.add_workspace(&ctx));
        assert!(app.error.is_empty());

        app.saved
            .workspaces
            .resize(32, app.saved.workspaces[0].clone());
        let blocked = root.path().join("over limit");
        app.new_directory = blocked.to_string_lossy().into_owned();
        assert!(!app.add_workspace(&ctx));
        assert!(!blocked.exists());
    }

    #[test]
    fn named_workspace_creation_retries_invalid_directory_and_submits_from_name() {
        let ctx = egui::Context::default();
        let mut app = fixture(0);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "occupied").unwrap();
        app.new_directory = file.to_string_lossy().into_owned();
        app.new_workspace_name = "  Review changes  ".into();
        app.palette = true;
        app.command_page = CommandPage::Create;
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
        let project = dir.path().join("new project");
        app.new_directory = project.to_string_lossy().into_owned();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("new-workspace-name")));
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert!(project.is_dir());
        assert!(app.error.is_empty());
        assert_eq!(app.saved.workspaces.len(), 1);
        assert_eq!(app.saved.workspaces[0].task.title, "Review changes");
        assert_eq!(app.saved.workspaces[0].task.directory, app.new_directory);
        assert_eq!(app.panes.len(), 1);
        assert!(!app.palette);
        assert!(app.new_workspace_name.is_empty());
        assert!(app.add_workspace(&ctx));
        assert_eq!(app.saved.workspaces[1].task.title, "new project");
    }

    #[test]
    fn enter_in_workspace_fields_creates_or_opens_and_preserves_invalid_input() {
        let ctx = egui::Context::default();
        let mut app = fixture(2);
        let dir = tempfile::tempdir().unwrap();
        app.new_directory = dir.path().to_string_lossy().into_owned();
        app.palette = true;
        app.command_page = CommandPage::Create;
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
        app.command_page = CommandPage::Create;
        let file = dir.path().join("file");
        std::fs::write(&file, "occupied").unwrap();
        app.new_directory = file.to_string_lossy().into_owned();
        frame(&mut app, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("workspace-directory")));
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert!(app.palette);
        assert_eq!(app.saved.workspaces.len(), 3);
        assert_eq!(app.panes.len(), 1);
        assert!(app.error.contains("Cannot create working directory"));

        app.saved.workspaces[1].task.title = "Unique match".into();
        app.command_page = CommandPage::Commands;
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
            app.command_page = CommandPage::Create;
            let file = dir.path().join("file");
            std::fs::write(&file, "occupied").unwrap();
            app.new_directory = file.to_string_lossy().into_owned();
            frame(&mut app, vec![]);
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("workspace-directory")));
            frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
            assert!(app.palette);
            assert_eq!(app.overview, overview);
            assert_eq!(app.saved.workspaces.len(), 1);
            assert!(app.error.contains("Cannot create working directory"));
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
    fn split_terminal_sizes_settle_after_creation_and_removal() {
        let ctx = egui::Context::default();
        let mut app = fixture(1);
        app.overview = false;
        app.saved.workspaces[0].task.directory = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        start_fixture_terminal(&mut app, &ctx);
        let original = app.saved.workspaces[0].focus;
        let frame = |app: &mut App| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        };
        let dimensions = |app: &App, pane| {
            let size = app.panes[&pane].terminal.size;
            (size.cols, size.rows)
        };
        frame(&mut app);
        frame(&mut app);
        let full = dimensions(&app, original);
        for vertical in [true, false] {
            app.saved.workspaces[0].focus = original;
            app.split(&ctx, vertical);
            let split = app.saved.workspaces[0].focus;
            frame(&mut app);
            frame(&mut app);
            let first = dimensions(&app, original);
            let second = dimensions(&app, split);
            assert!(if vertical {
                first.0 < full.0
            } else {
                first.1 < full.1
            });
            assert!(first.0 > 2 && first.1 > 1 && second.0 > 2 && second.1 > 1);
            for _ in 0..10 {
                frame(&mut app);
                assert_eq!(dimensions(&app, original), first);
                assert_eq!(dimensions(&app, split), second);
            }
            app.stop_terminal(split);
            frame(&mut app);
            frame(&mut app);
            for _ in 0..10 {
                frame(&mut app);
                assert_eq!(dimensions(&app, original), full);
            }
        }
    }

    #[test]
    fn discarded_layout_pass_does_not_resize_the_shell() {
        use alacritty_terminal::grid::Dimensions;
        let ctx = egui::Context::default();
        let mut terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            std::path::Path::new(""),
            ctx.clone(),
        )
        .unwrap();
        let mut search = TerminalSearch::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
                ..Default::default()
            },
            |ctx| {
                if ctx.current_pass_index() == 0 {
                    ctx.request_discard("workspace layout changed after closing a tab");
                    assert!(ctx.will_discard());
                    egui::CentralPanel::default().show(ctx, |ui| {
                        terminal_view(ui, &mut terminal, &mut search, true, 15.0, false)
                    });
                } else {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.scope_builder(egui::UiBuilder::new().sizing_pass(), |ui| {
                            assert!(ui.is_sizing_pass());
                            terminal_view(ui, &mut terminal, &mut search, true, 15.0, false)
                        });
                    });
                }
            },
        );
        terminal
            .input(b"printf 'LAYOUT_BARRIER\\n'\n".to_vec())
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let term = terminal.term.lock().unwrap();
            let text: String = term.grid().display_iter().map(|cell| cell.cell.c).collect();
            if text.contains("LAYOUT_BARRIER") {
                assert_eq!(term.columns(), 100);
                assert_eq!(term.screen_lines(), 30);
                break;
            }
            drop(term);
            assert!(
                std::time::Instant::now() < deadline,
                "shell did not process barrier input"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn terminal_content_stays_visible_while_parser_holds_grid() {
        let ctx = egui::Context::default();
        let mut terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            std::path::Path::new(""),
            ctx.clone(),
        )
        .unwrap();
        let mut search = TerminalSearch::default();
        let mut frame = |terminal: &mut Terminal| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 480.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        terminal_view(ui, terminal, &mut search, true, 15.0, false)
                            .1
                            .unwrap();
                    });
                },
            )
        };
        frame(&mut terminal);
        std::thread::sleep(Duration::from_millis(30));
        {
            let mut term = terminal.term.lock().unwrap();
            let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
            parser.advance(&mut *term, b"\x1b[2J\x1b[Hstable-output");
        }
        let rendered = frame(&mut terminal);
        let grid = terminal.term.clone();
        let held = grid.lock().unwrap();
        let busy = frame(&mut terminal);
        let text = |out: &egui::FullOutput| {
            out.shapes
                .iter()
                .filter_map(|s| {
                    if let egui::epaint::Shape::Text(t) = &s.shape {
                        Some(t.galley.text().to_owned())
                    } else {
                        None
                    }
                })
                .collect::<String>()
        };
        assert!(text(&rendered).contains("stable-output"));
        assert!(
            text(&busy).contains("stable-output"),
            "busy parser erased terminal content"
        );
        drop(held);
    }

    #[test]
    fn double_click_selects_complete_space_delimited_terminal_token() {
        let ctx = egui::Context::default();
        let mut terminal = Terminal::spawn_test(
            Uuid::new_v4(),
            &std::env::current_dir().unwrap(),
            std::path::Path::new(""),
            ctx.clone(),
        )
        .unwrap();
        let mut search = TerminalSearch::default();
        let mut pos = Pos2::ZERO;
        let mut frame = |terminal: &mut Terminal, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 480.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let font = FontId::monospace(15.0);
                        let cell = ui.fonts_mut(|f| {
                            Vec2::new(f.glyph_width(&font, 'M'), f.row_height(&font))
                        });
                        pos = ui.available_rect_before_wrap().shrink(8.0).min
                            + Vec2::new(12.5 * cell.x, 0.5 * cell.y);
                        terminal_view(ui, terminal, &mut search, true, 15.0, false)
                            .1
                            .unwrap();
                    });
                },
            )
        };
        frame(&mut terminal, vec![]);
        std::thread::sleep(Duration::from_millis(30));
        {
            let mut term = terminal.term.lock().unwrap();
            let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
            parser.advance(
                &mut *term,
                b"\x1b[2J\x1b[Hprefix https://host/a(b):42?q=x suffix",
            );
        }
        frame(&mut terminal, vec![]);
        for pressed in [true, false, true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 480.0))),
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
                        terminal_view(ui, &mut terminal, &mut search, true, 15.0, false)
                            .1
                            .unwrap();
                    });
                },
            );
        }
        assert_eq!(
            terminal
                .term
                .lock()
                .unwrap()
                .selection_to_string()
                .as_deref(),
            Some("https://host/a(b):42?q=x")
        );
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
        let terminal = Terminal::spawn_test(
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
        app.open_commands(CommandPage::Commands);
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| app.draw(ctx));
        }
        let forbidden = format!("printf BAD >> '{}'\r", output_path.display());
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Text(forbidden.clone()),
                    egui::Event::Paste(forbidden.clone()),
                    egui::Event::Ime(egui::ImeEvent::Commit(forbidden)),
                    key_event(Key::Enter, None, Modifiers::NONE),
                ],
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(Key::Escape, None, Modifiers::NONE)],
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        );
        assert!(!app.palette);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            b"x",
            "command center input leaked into the shell"
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
    fn command_center_search_navigation_and_actions_work_from_the_keyboard() {
        let ctx = egui::Context::default();
        let mut app = fixture(2);
        let frame = |app: &mut App, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 400.0))),
                    events,
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            )
        };
        app.open_commands(CommandPage::Commands);
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new("workspace-filter"))));
        frame(
            &mut app,
            vec![key_event(Key::ArrowDown, None, Modifiers::NONE)],
        );
        assert_eq!(app.command_index, 1);
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert_eq!(app.active, 1);
        assert!(!app.palette);
        assert!(!app.overview);

        app.open_commands(CommandPage::Commands);
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        frame(
            &mut app,
            vec![key_event(Key::ArrowUp, None, Modifiers::NONE)],
        );
        assert_eq!(app.command_index, 8, "Up should wrap to Settings");
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert!(app.palette);
        assert_eq!(app.command_page as u8, CommandPage::Settings as u8);

        app.open_commands(CommandPage::Commands);
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Text("New workspace".into())]);
        assert_eq!(app.command_index, 0, "search resets the selection");
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert_eq!(app.command_page as u8, CommandPage::Create as u8);
        frame(&mut app, vec![]);
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new("workspace-directory"))));
        assert_eq!(app.saved.workspaces.len(), 2);

        app.open_commands(CommandPage::Commands);
        app.filter = "~/Projects/melimo".into();
        frame(&mut app, vec![]);
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert_eq!(app.active, 1, "directory search should open its workspace");
        assert!(!app.palette);

        app.saved.workspaces.clear();
        app.open_commands(CommandPage::Commands);
        app.filter = "Split".into();
        frame(&mut app, vec![]);
        frame(&mut app, vec![key_event(Key::Enter, None, Modifiers::NONE)]);
        assert!(app.palette, "disabled command should stay open");
        assert!(app.panes.is_empty());
        assert!(app.error.is_empty());
    }

    #[test]
    fn command_center_fits_short_windows_and_keeps_create_action_visible() {
        for light in [false, true] {
            for (width, height) in [(640.0, 400.0), (1180.0, 760.0)] {
                for page in [
                    CommandPage::Commands,
                    CommandPage::Create,
                    CommandPage::Settings,
                ] {
                    let ctx = egui::Context::default();
                    configure_appearance(&ctx, light);
                    let mut app = fixture(32);
                    app.saved.light = light;
                    app.saved.workspaces[0].task.title = "Very long workspace title ".repeat(20);
                    app.new_directory = "/a/long/project/directory/".repeat(20);
                    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height));
                    app.open_commands(page);
                    let mut out = None;
                    for _ in 0..4 {
                        out = Some(ctx.run(
                            egui::RawInput {
                                screen_rect: Some(screen),
                                ..Default::default()
                            },
                            |ctx| app.draw(ctx),
                        ));
                    }
                    let modal = ctx
                        .memory(|m| m.area_rect(egui::Id::new("workspace-commands")))
                        .unwrap();
                    assert!(
                        screen.contains_rect(modal),
                        "Command center overflows: {modal:?}"
                    );
                    if page == CommandPage::Create {
                        let button = out
                            .unwrap()
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::epaint::Shape::Text(text)
                                    if text.galley.text() == "Create workspace" =>
                                {
                                    Some(text.galley.rect.translate(text.pos.to_vec2()))
                                }
                                _ => None,
                            })
                            .expect("create button must remain outside the scrolling form");
                        assert!(screen.contains_rect(button));
                        assert!(modal.contains_rect(button));
                    }
                }
            }
        }
    }

    #[test]
    fn compact_overview_activity_and_filtered_selection_remain_accessible() {
        let ctx = egui::Context::default();
        let mut app = fixture(3);
        let frame = |app: &mut App| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 400.0))),
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            )
        };
        for _ in 0..3 {
            frame(&mut app);
        }
        app.overview_inspector = true;
        let out = frame(&mut app);
        assert!(out.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::epaint::Shape::Text(text) if text.galley.text() == "SESSION CONTEXT")));
        app.category = 2;
        let out = frame(&mut app);
        assert!(
            app.selected.is_none(),
            "empty filter must not retain unrelated session context"
        );
        assert!(out.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::epaint::Shape::Text(text) if text.galley.text() == "No sessions in this view")));
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
