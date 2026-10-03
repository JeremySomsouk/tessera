use crate::{
    integration::{Endpoint, now},
    model::{Session, SessionState, Task, TaskState},
    terminal::{Size, Terminal, color},
};
use alacritty_terminal::{
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{TermMode, cell::Flags},
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
}
pub struct App {
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
    palette: bool,
    filter: String,
    maximized: bool,
    close: Option<Uuid>,
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
        let (mut saved, load_error) = match std::fs::read(&path) {
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
        for session in &mut saved.sessions {
            if session.state != SessionState::Ended {
                session.state = SessionState::Disconnected;
            }
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
            palette: false,
            filter: String::new(),
            maximized: false,
            close: None,
            category: 0,
        };
        match endpoint {
            Ok(e) => app.endpoint = Some(e),
            Err(e) => app.error = format!("Event endpoint unavailable: {e}"),
        }
        cc.egui_ctx.set_visuals(if app.saved.light {
            egui::Visuals::light()
        } else {
            egui::Visuals::dark()
        });
        // Restore metadata only. Live processes never survive application shutdown in v0.1.
        if app.saved.workspaces.is_empty() {
            app.add_workspace(&cc.egui_ctx);
        } else {
            app.overview = true;
        }
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
                self.panes.insert(id, Pane { terminal });
                true
            }
            Err(e) => {
                self.error = format!("Cannot start shell: {e}");
                false
            }
        }
    }
    fn add_workspace(&mut self, ctx: &egui::Context) {
        if self.saved.workspaces.len() >= 32 {
            self.error = "Workspace limit (32) reached".into();
            return;
        }
        let directory = self.new_directory.clone();
        let id = Uuid::new_v4();
        if !self.spawn(id, ctx, &directory) {
            return;
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
        let directory = w.task.directory.clone();
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
    fn resume(&mut self, ctx: &egui::Context, index: usize) {
        let mut ids = Vec::new();
        self.saved.workspaces[index].layout.ids(&mut ids);
        let dir = self.saved.workspaces[index].task.directory.clone();
        for id in ids {
            if !self.panes.contains_key(&id) {
                self.spawn(id, ctx, &dir);
            }
        }
    }
    fn drain(&mut self) {
        if let Some(endpoint) = &self.endpoint {
            for e in endpoint.receiver.try_iter().take(256) {
                if !self.panes.contains_key(&e.pane) {
                    continue;
                }
                let idx = self
                    .saved
                    .sessions
                    .iter()
                    .position(|s| s.session_id == e.session_id && s.pane == e.pane);
                let idx = idx.unwrap_or_else(|| {
                    if self.saved.sessions.len() >= 256 {
                        self.saved.sessions.remove(0);
                    }
                    self.saved
                        .sessions
                        .push(Session::new(e.pane, e.session_id.clone()));
                    self.saved.sessions.len() - 1
                });
                self.dirty |= self.saved.sessions[idx].apply(e, now());
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
                self.maximized = false;
                return;
            }
        }
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let command = if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL | Modifiers::ALT
        };
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::O)) {
            self.overview = !self.overview;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::T)) {
            self.add_workspace(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::D)) {
            self.split(ctx, true);
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::D)) {
            self.split(ctx, false);
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::Enter)) {
            self.maximized = !self.maximized;
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::P)) {
            self.palette = !self.palette;
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::W)) {
            self.close = self.saved.workspaces.get(self.active).map(|w| w.focus);
        }
        if ctx.input_mut(|i| i.consume_key(command | Modifiers::SHIFT, Key::N))
            && let Some(s) = self.saved.sessions.iter().find(|s| s.state.attention())
        {
            let id = s.key();
            self.focus_session(&id);
        }
        for (n, key) in [
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
        .enumerate()
        {
            if n < self.saved.workspaces.len() && ctx.input_mut(|i| i.consume_key(command, *key)) {
                self.active = n;
                self.overview = false;
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
            egui::Frame::group(ui.style()).inner_margin(24.0).show(ui,|ui| {ui.heading("Your first session starts in a terminal");ui.label("Enable Tessera hooks, then run claude in any pane. Sessions will appear here as events arrive.");ui.add_space(12.0);ui.monospace("tessera hooks                 # preview\ntessera install-hooks         # merge with backup\nclaude");if ui.button("Return to terminal").clicked() {self.overview=false;}});
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
                                            "Session {}",
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
            if !live {ui.label("This session is disconnected. Resume its workspace to start a new shell.");}
            let wi=self.saved.workspaces.iter().position(|w|{let mut ids=Vec::new();w.layout.ids(&mut ids);ids.contains(&session.pane)});
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
                    .selectable_label(self.overview, "Overview  ⇧⌘O")
                    .clicked()
                {
                    self.overview = !self.overview;
                }
                if ui.button("+ Workspace").clicked() {
                    self.palette = true;
                }
                if ui.button("Split ↔").clicked() {
                    self.split(ctx, true);
                }
                if ui.button("Split ↕").clicked() {
                    self.split(ctx, false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(if self.saved.light { "Dark" } else { "Light" })
                        .clicked()
                    {
                        self.saved.light = !self.saved.light;
                        ctx.set_visuals(if self.saved.light {
                            egui::Visuals::light()
                        } else {
                            egui::Visuals::dark()
                        });
                        self.dirty = true;
                    }
                    if ui
                        .add(egui::Slider::new(&mut self.font_size, 11.0..=24.0).show_value(false))
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
                        RichText::new("⌘D split · ⇧⌘D stack · ⌥⌘→ next pane · ⇧⌘P commands")
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
        egui::CentralPanel::default().show(ctx,|ui| {
            if self.overview {self.overview(ui);return;}
            let Some(w)=self.saved.workspaces.get_mut(self.active) else {ui.label("Create a workspace to start a shell.");return;};
            let mut rects=Vec::new();let rect=ui.available_rect_before_wrap();
            if self.maximized {rects.push((w.focus,rect));}else{w.layout.rects(ui,rect,&mut rects);}
            let focused=w.focus;
            for (id,rect) in rects {
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect).id_salt(id),|ui| {
                    if let Some(pane)=self.panes.get_mut(&id) {
                        ui.horizontal(|ui| {ui.label(RichText::new(if focused==id {"● Terminal"}else{"Terminal"}).color(if focused==id{ACCENT}else{ui.visuals().weak_text_color()}));if ui.small_button("×").clicked(){self.close=Some(id);}
if !pane.terminal.alive.load(Ordering::Acquire){ui.label("Shell exited");}});
                        if let Some(err)=pane.terminal.error.try_lock().ok().and_then(|e|e.clone()) {ui.small(err);}
                        let (clicked,result)=terminal_view(ui,&mut pane.terminal,focused==id&&!self.palette&&self.close.is_none(),self.font_size);
                        if clicked {self.saved.workspaces[self.active].focus=id;self.dirty=true;}
                        if let Err(e)=result {self.error=e.to_string();}
                    }else{ui.heading("Workspace restored");ui.label("The previous processes have stopped. Start fresh login shells in this layout.");if ui.button("Resume workspace").clicked(){self.resume(ctx,self.active);}}
                });
            }
        });
        if self.palette {
            egui::Window::new("Workspace & commands")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Working directory");
                    ui.text_edit_singleline(&mut self.new_directory);
                    if ui.button("Create workspace").clicked() {
                        self.add_workspace(ctx);
                        self.palette = false;
                    }
                    ui.separator();
                    ui.label("Find workspace");
                    ui.text_edit_singleline(&mut self.filter);
                    for (i, w) in self.saved.workspaces.iter().enumerate() {
                        if w.task
                            .title
                            .to_lowercase()
                            .contains(&self.filter.to_lowercase())
                            && ui.button(&w.task.title).clicked()
                        {
                            self.active = i;
                            self.overview = false;
                            self.palette = false;
                        }
                    }
                    ui.separator();
                    if ui.button("Toggle Overview").clicked() {
                        self.overview = !self.overview;
                        self.palette = false;
                    }
                    if ui.button("Maximize / restore pane").clicked() {
                        self.maximized = !self.maximized;
                        self.palette = false;
                    }
                    if ui.button("Close").clicked() {
                        self.palette = false;
                    }
                });
        }
        if let Some(id) = self.close {
            egui::Window::new("Stop this terminal?").collapsible(false).resizable(false).show(ctx,|ui| {ui.label("The shell and its child processes will be stopped. Task history is retained.");if ui.button("Stop terminal").clicked(){self.panes.remove(&id);for w in &mut self.saved.workspaces {if w.layout.remove(id){let mut ids=Vec::new();w.layout.ids(&mut ids);w.focus=ids[0];}}self.close=None;self.dirty=true;}
if ui.button("Keep running").clicked(){self.close=None;}});
        }
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
fn terminal_view(
    ui: &mut egui::Ui,
    terminal: &mut Terminal,
    focused: bool,
    font_size: f32,
) -> (bool, anyhow::Result<()>) {
    let font = FontId::monospace(font_size);
    let cell = ui.fonts_mut(|f| Vec2::new(f.glyph_width(&font, 'M'), f.row_height(&font)));
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    if focused || response.clicked() {
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
    if let Ok(mut term) = terminal.term.try_lock() {
        mode = *term.mode();
        let reporting = mode.intersects(TermMode::MOUSE_MODE) && !ui.input(|i| i.modifiers.shift);
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
        if focused && mode.contains(TermMode::SHOW_CURSOR) && content.display_offset == 0 {
            let p = content.cursor.point;
            let pos =
                grid_rect.min + Vec2::new(p.column.0 as f32 * cell.x, p.line.0 as f32 * cell.y);
            painter.rect_stroke(
                Rect::from_min_size(pos, cell),
                0.0,
                Stroke::new(1.0_f32, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        selected = term.selection_to_string();
    } else {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let reporting = mode.intersects(TermMode::MOUSE_MODE) && !ui.input(|i| i.modifiers.shift);
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.0 {
            terminal.scroll((scroll / cell.y).round() as i32);
        }
    }
    let clicked = response.clicked() || response.drag_started();
    if (!focused && !clicked) || (!response.has_focus() && ui.ctx().wants_keyboard_input()) {
        return (clicked, Ok(()));
    }
    let mut result = Ok(());
    for event in ui.input(|i| i.events.clone()) {
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
        if let Some(bytes) = input
            && let Err(e) = terminal.input(bytes)
        {
            result = Err(e);
        }
    }
    (clicked, result)
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
            for seq in 1..4 {
                s.apply(
                    HookEvent {
                        id: Uuid::new_v4(),
                        pane,
                        sequence: seq,
                        session_id: format!("fixture-{n}"),
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
            palette: false,
            filter: String::new(),
            maximized: false,
            close: None,
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
            ctx.set_visuals(if light {
                egui::Visuals::light()
            } else {
                egui::Visuals::dark()
            });
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
        app.panes.insert(pane, Pane { terminal });
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
        }
        let grid = app.panes[&pane].terminal.term.clone();
        let held = grid.lock().unwrap();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
                events: vec![egui::Event::Text("printf '\\nACK_UI_ONCE\\n'\r".into())],
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        );
        drop(held);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut ack = false;
        while std::time::Instant::now() < deadline {
            let t = app.panes[&pane].terminal.term.lock().unwrap();
            let text: String = t.grid().display_iter().map(|c| c.cell.c).collect();
            let count = text.matches("ACK_UI_ONCE").count();
            if count >= 2 {
                assert_eq!(count, 2, "keyboard input was duplicated");
                ack = true;
                break;
            }
            drop(t);
            std::thread::sleep(Duration::from_millis(10));
        }
        if !ack {
            let t = app.panes[&pane].terminal.term.lock().unwrap();
            eprintln!(
                "SCREEN: {:?}",
                t.grid()
                    .display_iter()
                    .map(|c| c.cell.c)
                    .collect::<String>()
            );
        }
        assert!(ack, "terminal widget swallowed keyboard input");
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
