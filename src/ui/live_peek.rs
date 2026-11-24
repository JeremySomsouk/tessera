use super::*;
use alacritty_terminal::grid::Dimensions;

pub(super) fn show(ui: &mut egui::Ui, terminal: &Terminal) {
    let button = ui
        .small_button("Peek")
        .on_hover_text("Preview the latest output without moving your reading position");
    egui::Popup::from_toggle_button_response(&button)
        .align(egui::emath::RectAlign::TOP_END)
        .width((ui.ctx().content_rect().width() - 32.0).clamp(120.0, 380.0))
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.label(
                RichText::new("Live preview")
                    .small()
                    .strong()
                    .color(accent(ui)),
            );
            ui.separator();
            let lines = terminal.term.try_lock().ok().map(|grid| {
                let bottom = if grid.mode().contains(TermMode::ALT_SCREEN) {
                    grid.bottommost_line().0
                } else {
                    grid.grid().cursor.point.line.0
                };
                let mut lines = Vec::with_capacity(4);
                for row in (grid.topmost_line().0.max(bottom - 15)..=bottom).rev() {
                    let row = Line(row);
                    let text = grid.bounds_to_string(
                        Point::new(row, Column(0)),
                        Point::new(row, grid.last_column()),
                    );
                    if !lines.is_empty() || !text.trim().is_empty() {
                        lines.push(text);
                    }
                    if lines.len() == 4 {
                        break;
                    }
                }
                lines
            });
            if let Some(lines) = lines {
                for line in lines.into_iter().rev() {
                    ui.add(egui::Label::new(RichText::new(line).monospace().size(12.0)).truncate());
                }
            } else {
                ui.spinner();
                ui.ctx().request_repaint_after(Duration::from_millis(32));
            }
        });
}
