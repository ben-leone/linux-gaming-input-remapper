use eframe::egui::{self, RichText};

use super::{ProfileApp, GRAY};

impl ProfileApp {
    // ── Engine log section ────────────────────────────────────────────────────

    pub(super) fn show_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let was = self.log_enabled;
            if ui.checkbox(&mut self.log_enabled, "Enable engine logging").changed() {
                crate::engine::set_engine_logging(self.log_enabled);
                if !was {
                    crate::engine::clear_engine_log();
                    self.log_entries.clear();
                }
            }
            if ui.button("Clear").clicked() {
                self.log_entries.clear();
                crate::engine::clear_engine_log();
            }
            ui.label(
                RichText::new(format!("{} entries", self.log_entries.len()))
                    .small()
                    .color(GRAY),
            );
        });
        ui.separator();
        if !self.log_enabled && self.log_entries.is_empty() {
            ui.colored_label(GRAY, "Enable logging above, then trigger a macro.");
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in &self.log_entries {
                    ui.monospace(entry);
                }
            });
    }
}
