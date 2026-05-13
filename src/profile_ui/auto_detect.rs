use eframe::egui::{self, RichText};

use crate::config::AutoDetectTarget;
use super::{ProfileApp, GRAY, AMBER};

impl ProfileApp {
    // ── Auto detect section ───────────────────────────────────────────────────

    pub(super) fn show_auto_detect(&mut self, ui: &mut egui::Ui) {
        ui.heading("Auto Detect");
        ui.label(
            RichText::new(
                "Add targets with one or more process name fragments. \
                 All fragments in a target must match (case-insensitive) for it to fire. \
                 Any matching target activates this profile. Only one target is registered at a time."
            )
            .color(GRAY),
        );
        ui.separator();

        let targets: Vec<(uuid::Uuid, Vec<String>)> = self.profile()
            .map(|p| p.auto_detect.iter().map(|t| (t.id, t.fragments.clone())).collect())
            .unwrap_or_default();

        let mut delete_target:    Option<usize>          = None;
        let mut delete_fragment:  Option<(usize, usize)> = None;
        let mut confirm_fragment: Option<usize>          = None;
        let mut needs_save = false;

        if targets.is_empty() {
            ui.label(
                RichText::new("No targets yet — click \"+ New Target\" to add one.")
                    .color(GRAY)
                    .italics(),
            );
            ui.add_space(4.0);
        }

        for (i, (_, fragments)) in targets.iter().enumerate() {
            ui.group(|ui| {
                // Fragment chips with AND separators
                ui.horizontal_wrapped(|ui| {
                    if fragments.is_empty() {
                        ui.label(
                            RichText::new("(empty — add a fragment below)")
                                .color(GRAY)
                                .italics()
                                .small(),
                        );
                    }
                    for (j, frag) in fragments.iter().enumerate() {
                        if j > 0 {
                            ui.label(RichText::new("AND").small().color(AMBER));
                        }
                        ui.label(RichText::new(frag).monospace());
                        if ui.small_button("×").on_hover_text("Remove this fragment").clicked() {
                            delete_fragment = Some((i, j));
                        }
                    }
                });

                // Controls row
                ui.horizontal(|ui| {
                    if self.adding_fragment_to == Some(i) {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.new_fragment_text)
                                .hint_text("e.g. darktide.exe")
                                .desired_width(160.0),
                        );
                        let enter = resp.lost_focus()
                            && ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                        let can_add = !self.new_fragment_text.trim().is_empty();
                        let add_clicked = ui.add_enabled(can_add, egui::Button::new("Add")).clicked();
                        if (enter || add_clicked) && can_add {
                            confirm_fragment = Some(i);
                        }
                        if ui.small_button("Cancel").clicked() {
                            self.adding_fragment_to = None;
                            self.new_fragment_text.clear();
                        }
                    } else {
                        if ui.small_button("+ fragment").clicked() {
                            self.adding_fragment_to = Some(i);
                            self.new_fragment_text.clear();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove target").on_hover_text("Delete this target").clicked() {
                                delete_target = Some(i);
                            }
                        });
                    }
                });
            });
            ui.add_space(2.0);
        }

        if ui.button("+ New Target").clicked() {
            if let Some(p) = self.profile_mut() {
                p.auto_detect.push(AutoDetectTarget::new());
            }
            needs_save = true;
        }

        // ── Apply mutations ──────────────────────────────────────────────────
        if let Some((ti, fi)) = delete_fragment {
            if let Some(p) = self.profile_mut() {
                if let Some(t) = p.auto_detect.get_mut(ti) {
                    t.fragments.remove(fi);
                }
            }
            needs_save = true;
        }
        if let Some(ti) = confirm_fragment {
            let frag = self.new_fragment_text.trim().to_string();
            if !frag.is_empty() {
                if let Some(p) = self.profile_mut() {
                    if let Some(t) = p.auto_detect.get_mut(ti) {
                        t.fragments.push(frag);
                        needs_save = true;
                    }
                }
            }
            self.adding_fragment_to = None;
            self.new_fragment_text.clear();
        }
        if let Some(i) = delete_target {
            if let Some(p) = self.profile_mut() {
                p.auto_detect.remove(i);
            }
            self.adding_fragment_to = None;
            self.new_fragment_text.clear();
            needs_save = true;
        }

        if needs_save {
            self.save_current();
        }
    }
}
