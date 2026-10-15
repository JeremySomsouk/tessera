use super::*;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct SavedWindow {
    #[serde(default)]
    pub(super) root: bool,
    #[serde(default)]
    pub(super) workspaces: Vec<Uuid>,
    #[serde(default)]
    pub(super) active: Option<Uuid>,
    #[serde(default)]
    pub(super) bounds: Option<[f32; 4]>,
}

fn valid_bounds(bounds: [f32; 4]) -> bool {
    bounds.iter().all(|v| v.is_finite()) && bounds[2] >= 640.0 && bounds[3] >= 400.0
}

impl App {
    pub(super) fn capture_windows(&mut self) {
        let mut owners: Vec<_> = self
            .saved
            .workspaces
            .iter()
            .map(|w| self.workspace_window(w.task.id))
            .chain(self.windows.keys().copied())
            .collect();
        owners.sort_by_key(|id| (id != &egui::ViewportId::ROOT, id.0.value()));
        owners.dedup();
        self.saved.restoration.windows = owners
            .into_iter()
            .map(|id| {
                let workspaces: Vec<_> = self
                    .saved
                    .workspaces
                    .iter()
                    .filter(|w| self.workspace_window(w.task.id) == id)
                    .map(|w| w.task.id)
                    .collect();
                let window = self.windows.get(&id);
                let active = if self.drawing_window == Some(id) || self.windows.is_empty() {
                    self.saved.workspaces.get(self.ui.active).map(|w| w.task.id)
                } else {
                    window.and_then(|w| w.active)
                };
                let active = active
                    .filter(|id| workspaces.contains(id))
                    .or_else(|| workspaces.first().copied());
                SavedWindow {
                    root: id == egui::ViewportId::ROOT,
                    workspaces,
                    active,
                    bounds: window.and_then(|w| w.bounds),
                }
            })
            .collect();
    }

    pub(super) fn restore_windows(&mut self, ctx: &egui::Context) {
        self.windows.clear();
        self.workspace_windows.clear();
        let mut assigned = std::collections::HashSet::new();
        let mut root_used = false;
        for saved in self.saved.restoration.windows.clone() {
            let workspaces: Vec<_> = saved
                .workspaces
                .iter()
                .copied()
                .filter(|id| {
                    self.saved.workspaces.iter().any(|w| w.task.id == *id) && assigned.insert(*id)
                })
                .collect();
            if workspaces.is_empty() && !saved.workspaces.is_empty() {
                continue;
            }
            let root = saved.root && !root_used;
            root_used |= root;
            let id = if root {
                egui::ViewportId::ROOT
            } else {
                egui::ViewportId::from_hash_of(
                    workspaces.first().copied().unwrap_or_else(Uuid::new_v4),
                )
            };
            let active = saved
                .active
                .filter(|id| workspaces.contains(id))
                .or_else(|| workspaces.first().copied());
            for workspace in workspaces {
                self.workspace_windows.insert(workspace, id);
            }
            let bounds = saved.bounds.filter(|bounds| valid_bounds(*bounds));
            if root && let Some([x, y, width, height]) = bounds {
                ctx.send_viewport_cmd_to(
                    id,
                    egui::ViewportCommand::OuterPosition(egui::pos2(x, y)),
                );
                ctx.send_viewport_cmd_to(
                    id,
                    egui::ViewportCommand::InnerSize(egui::vec2(width, height)),
                );
            }
            self.windows.insert(
                id,
                TerminalWindow {
                    active,
                    ui: WindowUi::default(),
                    position: None,
                    bounds,
                    restore_geometry: !root,
                },
            );
        }
        let missing: Vec<_> = self
            .saved
            .workspaces
            .iter()
            .filter(|w| !assigned.contains(&w.task.id))
            .map(|w| w.task.id)
            .collect();
        if !missing.is_empty() {
            let active = self
                .saved
                .restoration
                .active
                .filter(|id| missing.contains(id))
                .or(Some(missing[0]));
            self.windows
                .entry(egui::ViewportId::ROOT)
                .or_insert_with(|| TerminalWindow {
                    active,
                    ui: WindowUi::default(),
                    position: None,
                    bounds: None,
                    restore_geometry: false,
                });
            for workspace in missing {
                self.workspace_windows
                    .insert(workspace, egui::ViewportId::ROOT);
            }
        }
        if !self.windows.is_empty() && !self.windows.contains_key(&egui::ViewportId::ROOT) {
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Visible(false),
            );
        }
    }

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
                bounds: None,
                restore_geometry: false,
            });
        let target = egui::ViewportId::from_hash_of(Uuid::new_v4());
        self.workspace_windows.insert(workspace, target);
        self.windows.insert(
            target,
            TerminalWindow {
                active: Some(workspace),
                ui: WindowUi::default(),
                position,
                bounds: None,
                restore_geometry: false,
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
        if self.quitting {
            return;
        }
        if self.windows.len() == 1 {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            return;
        }
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
        self.handle_app_quit(ctx);
        if self.quitting {
            return;
        }
        let Some(window) = self.windows.get_mut(&id) else {
            return;
        };
        ctx.input(|i| {
            if let (Some(outer), Some(inner)) = (i.viewport().outer_rect, i.viewport().inner_rect) {
                let bounds = [outer.min.x, outer.min.y, inner.width(), inner.height()];
                if valid_bounds(bounds) {
                    window.bounds = Some(bounds);
                }
            }
        });
        self.drawing_window = Some(id);
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
        self.drawing_window = None;
    }

    fn handle_app_quit(&mut self, ctx: &egui::Context) {
        let quit_modifiers = if cfg!(target_os = "macos") {
            egui::Modifiers::COMMAND
        } else {
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT
        };
        if ctx.input_mut(|i| i.consume_key(quit_modifiers, Key::Q)) {
            self.quitting = true;
            self.capture_restoration();
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
    }

    pub(super) fn show_windows(&mut self, ctx: &egui::Context) {
        self.handle_app_quit(ctx);
        if self.quitting {
            return;
        }
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
                    bounds: None,
                    restore_geometry: false,
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
            let builder = window.viewport_builder();
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

impl TerminalWindow {
    fn viewport_builder(&mut self) -> egui::ViewportBuilder {
        let mut builder = crate::main_viewport()
            .with_title("Tessera")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([640.0, 400.0]);
        if let Some([x, y, width, height]) = self.bounds {
            builder = builder.with_inner_size([width, height]);
            if self.restore_geometry {
                builder = builder.with_position(egui::pos2(x, y));
            }
        }
        self.restore_geometry = false;
        if let Some(position) = self.position.take() {
            builder = builder.with_position(position);
        }
        builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_geometry_does_not_reset_on_next_frame() {
        let mut window = TerminalWindow {
            active: None,
            ui: WindowUi::default(),
            position: None,
            bounds: Some([40.0, 50.0, 900.0, 600.0]),
            restore_geometry: true,
        };
        let mut initial = window.viewport_builder();
        let (commands, _) = initial.patch(window.viewport_builder());
        assert!(
            !commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::InnerSize(_)))
        );
    }

    #[test]
    fn invalid_geometry_is_rejected() {
        assert!(!valid_bounds([f32::NAN, 0.0, 900.0, 600.0]));
        assert!(!valid_bounds([0.0, 0.0, -900.0, 600.0]));
        assert!(valid_bounds([-1000.0, 0.0, 900.0, 600.0]));
    }

    #[test]
    fn app_quit_keeps_all_windows() {
        let ctx = egui::Context::default();
        let mut app = super::super::render_tests::fixture(2);
        app.restore_windows(&ctx);
        let ids: Vec<_> = app.saved.workspaces.iter().map(|w| w.task.id).collect();
        app.detach_workspace(&ctx, ids[1], None);
        let modifiers = if cfg!(target_os = "macos") {
            egui::Modifiers::COMMAND
        } else {
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT
        };
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Q,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
            ..Default::default()
        };
        let child = app.workspace_window(ids[1]);
        let output = ctx.run(input, |ctx| app.draw_terminal_window(ctx, child));
        app.close_terminal_window(&ctx, egui::ViewportId::ROOT);
        assert!(app.quitting);
        assert_eq!(app.saved.workspaces.len(), 2);
        assert_eq!(app.windows.len(), 2);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Close))
        );
    }
}
