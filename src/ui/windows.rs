use super::*;

impl App {
    pub(super) fn new_terminal_window(&mut self, ctx: &egui::Context) {
        let count = self.saved.workspaces.len();
        let active = self.ui.active;
        self.add_workspace_from_terminal(ctx);
        if self.saved.workspaces.len() > count {
            let workspace = self.saved.workspaces[self.ui.active].task.id;
            self.detach_workspace(ctx, workspace, None);
            self.ui.active = active;
            self.repair_active_workspace();
        }
    }

    pub(super) fn workspace_window(&self, workspace: Uuid) -> egui::ViewportId {
        self.workspace_windows
            .get(&workspace)
            .copied()
            .unwrap_or(egui::ViewportId::ROOT)
    }

    pub(super) fn window_workspaces(&self) -> Vec<usize> {
        self.saved
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, w)| self.workspace_window(w.task.id) == self.current_window)
            .map(|(index, _)| index)
            .collect()
    }

    pub(super) fn repair_active_workspace(&mut self) {
        if self
            .saved
            .workspaces
            .get(self.ui.active)
            .is_none_or(|w| self.workspace_window(w.task.id) != self.current_window)
        {
            self.ui.active = self
                .window_workspaces()
                .first()
                .copied()
                .unwrap_or(usize::MAX);
        }
    }

    pub(super) fn register_workspace_window(&mut self) {
        if let Some(workspace) = self.saved.workspaces.last() {
            self.workspace_windows
                .insert(workspace.task.id, self.current_window);
        }
    }

    fn record_workspace_folder(&mut self, index: usize) {
        let Some(workspace) = self.saved.workspaces.get(index) else {
            return;
        };
        let path = self
            .panes
            .get(&workspace.focus)
            .and_then(|pane| pane.terminal.current_directory().ok())
            .or_else(|| crate::directories::resolve(&workspace.task.directory).ok());
        if let Some(path) = path {
            self.saved.record_folder_open(path);
            self.dirty = true;
        }
    }

    pub(super) fn focus_workspace(&mut self, index: usize) {
        let workspace = &self.saved.workspaces[index];
        let id = workspace.task.id;
        let owner = self.workspace_window(id);
        if owner == self.current_window {
            self.record_workspace_folder(index);
            self.ui.active = index;
            self.ui.overview = false;
            self.ui.specifications_view = false;
            self.ui.maximized = false;
        } else if let Some(window) = self.windows.get_mut(&owner) {
            window.active = Some(id);
            window.ui.overview = false;
            window.ui.specifications_view = false;
            window.ui.maximized = false;
            self.pending_focus = Some(id);
            self.record_workspace_folder(index);
        }
    }

    pub(super) fn detach_workspace(
        &mut self,
        ctx: &egui::Context,
        workspace: Uuid,
        position: Option<Pos2>,
    ) {
        if !self.saved.workspaces.iter().any(|w| w.task.id == workspace)
            || self.workspace_window(workspace) != self.current_window
        {
            return;
        }
        let source = self.current_window;
        self.windows
            .entry(source)
            .or_insert_with(|| TerminalWindow {
                active: Some(workspace),
                ui: WindowUi::default(),
                position: None,
            });
        let target = egui::ViewportId::from_hash_of(Uuid::new_v4());
        self.workspace_windows.insert(workspace, target);
        self.windows.insert(
            target,
            TerminalWindow {
                active: Some(workspace),
                ui: WindowUi::default(),
                position,
            },
        );
        self.ui.palette = false;
        self.ui.rename = None;
        self.ui.closing = None;
        self.ui.maximized = false;
        self.repair_active_workspace();
        if self.window_workspaces().is_empty() {
            self.retire_window(ctx, source);
        }
        self.pending_focus = Some(workspace);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    fn retire_window(&mut self, ctx: &egui::Context, id: egui::ViewportId) {
        self.windows.remove(&id);
        if self
            .folder_launch
            .as_ref()
            .is_some_and(|launch| launch.window == id)
        {
            self.folder_launch = None;
        }
        if id == egui::ViewportId::ROOT && !self.windows.is_empty() {
            // eframe's root owns the event loop, so retain it as an invisible host.
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(false));
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub(super) fn close_terminal_window(&mut self, ctx: &egui::Context, id: egui::ViewportId) {
        let panes: Vec<_> = self
            .saved
            .workspaces
            .iter()
            .filter(|w| self.workspace_window(w.task.id) == id)
            .flat_map(|w| {
                let mut panes = Vec::new();
                w.layout.ids(&mut panes);
                panes
            })
            .collect();
        for pane in panes {
            self.stop_terminal(pane);
        }
        self.retire_window(ctx, id);
        if self.windows.is_empty() {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
    }

    fn draw_terminal_window(&mut self, ctx: &egui::Context, id: egui::ViewportId) {
        let Some(window) = self.windows.get_mut(&id) else {
            return;
        };
        std::mem::swap(&mut self.ui, &mut window.ui);
        let active = window.active;
        self.current_window = id;
        self.ui.active = active
            .and_then(|id| self.saved.workspaces.iter().position(|w| w.task.id == id))
            .unwrap_or(usize::MAX);
        self.repair_active_workspace();
        if ctx.input(|i| i.viewport().close_requested()) {
            self.close_terminal_window(ctx, id);
        } else {
            self.draw(ctx);
        }
        if let Some(window) = self.windows.get_mut(&id) {
            window.active = self.saved.workspaces.get(self.ui.active).map(|w| w.task.id);
            std::mem::swap(&mut self.ui, &mut window.ui);
        } else {
            self.ui = WindowUi::default();
        }
        self.current_window = egui::ViewportId::ROOT;
    }

    pub(super) fn show_windows(&mut self, ctx: &egui::Context) {
        if self.windows.is_empty() {
            if ctx.input(|i| i.viewport().close_requested()) {
                return;
            }
            self.windows.insert(
                egui::ViewportId::ROOT,
                TerminalWindow {
                    active: self.saved.workspaces.get(self.ui.active).map(|w| w.task.id),
                    ui: std::mem::take(&mut self.ui),
                    position: None,
                },
            );
        }
        if self.windows.contains_key(&egui::ViewportId::ROOT) {
            self.draw_terminal_window(ctx, egui::ViewportId::ROOT);
        }
        let windows: Vec<_> = self
            .windows
            .keys()
            .copied()
            .filter(|id| *id != egui::ViewportId::ROOT)
            .collect();
        let mut rendered = vec![egui::ViewportId::ROOT];
        for id in windows {
            let Some(window) = self.windows.get_mut(&id) else {
                continue;
            };
            let mut builder = crate::main_viewport()
                .with_title("Tessera")
                .with_inner_size([1180.0, 760.0])
                .with_min_inner_size([640.0, 400.0]);
            if let Some(position) = window.position.take() {
                builder = builder.with_position(position);
            }
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                self.draw_terminal_window(ctx, id);
            });
            rendered.push(id);
        }
        if let Some(workspace) = self.pending_focus {
            let owner = self.workspace_window(workspace);
            if rendered.contains(&owner) {
                ctx.send_viewport_cmd_to(owner, egui::ViewportCommand::Focus);
                self.pending_focus = None;
            } else {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        }
    }

    pub(super) fn finish_workspace_drag(&mut self, ctx: &egui::Context) {
        if self
            .dragged_workspace
            .is_some_and(|id| self.workspace_window(id) != self.current_window)
        {
            return;
        }
        if ctx.input(|i| {
            i.key_pressed(Key::Escape)
                || (!i.focused && !i.pointer.any_down() && !i.pointer.any_released())
        }) {
            self.dragged_workspace = None;
            return;
        }
        let Some(workspace) = self.dragged_workspace else {
            return;
        };
        if !ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
            return;
        }
        self.dragged_workspace = None;
        let Some(pointer) = ctx.input(|i| {
            i.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } => Some(*pos),
                _ => None,
            })
        }) else {
            return;
        };
        let content_rect = ctx.content_rect();
        let (outside, position) = ctx.input(|i| {
            let viewport = i.viewport();
            let position = viewport.inner_rect.map(|r| r.min + pointer.to_vec2());
            let outside = match (viewport.outer_rect, position) {
                (Some(outer), Some(position)) => !outer.contains(position),
                _ => !content_rect.contains(pointer),
            };
            (outside, position)
        });
        if outside {
            self.detach_workspace(ctx, workspace, position);
        }
    }
}
