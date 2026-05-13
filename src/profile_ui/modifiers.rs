use eframe::egui::{self, RichText};
use uuid::Uuid;

use super::{LearnTarget, ProfileApp, GRAY};

impl ProfileApp {
    // ── Modifiers section ─────────────────────────────────────────────────────

    pub(super) fn show_modifiers(&mut self, ui: &mut egui::Ui) {
        ui.heading("Modifier Keys");
        ui.label(
            RichText::new("Holding a modifier changes which assignments fire.")
                .color(GRAY),
        );
        ui.separator();

        // Collect modifiers snapshot for rendering (avoids borrow conflict with profile_mut)
        let modifiers: Vec<(Uuid, String, String, Option<String>)> = self.profile()
            .map(|p| p.modifiers.iter()
                .map(|m| (m.id, m.key.clone(), m.name.clone(), m.source_device.clone()))
                .collect())
            .unwrap_or_default();

        let mut delete_idx: Option<usize> = None;

        egui::Grid::new("mod_grid")
            .num_columns(4)
            .striped(true)
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.strong("Key");
                ui.strong("Device");
                ui.strong("Name");
                ui.label("");
                ui.end_row();

                for (i, (_, key, name, device)) in modifiers.iter().enumerate() {
                    ui.label(key);
                    if let Some(dev) = device {
                        let short = if dev.len() > 16 { format!("{}…", &dev[..16]) } else { dev.clone() };
                        ui.label(RichText::new(&short).small().weak()).on_hover_text(dev.as_str());
                    } else {
                        ui.label(RichText::new("any").small().color(GRAY));
                    }
                    ui.label(name);
                    if ui.small_button("x").on_hover_text("Remove").clicked() {
                        delete_idx = Some(i);
                    }
                    ui.end_row();
                }
            });

        if let Some(i) = delete_idx {
            let removed_id = modifiers[i].0;
            if let Some(p) = self.profile_mut() {
                p.modifiers.remove(i);
                for a in &mut p.assignments {
                    a.modifiers.retain(|&id| id != removed_id);
                }
            }
            self.save_current();
        }

        ui.separator();
        self.show_add_modifier(ui);
    }

    pub(super) fn show_add_modifier(&mut self, ui: &mut egui::Ui) {
        if !self.adding_modifier {
            if ui.button("+ Add Modifier").clicked() {
                self.adding_modifier = true;
                self.start_learn(LearnTarget::NewModifier);
            }
            return;
        }

        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Press a key…");
            if ui.small_button("Cancel").clicked() {
                self.adding_modifier = false;
                self.cancel_learn();
            }
        });
    }
}
