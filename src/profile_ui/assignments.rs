use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use uuid::Uuid;

use crate::config::{store, FireMode, TriggerMode};
use super::{LearnTarget, ProfileApp, GRAY};

impl ProfileApp {
    // ── Assignments section ───────────────────────────────────────────────────

    pub(super) fn show_assignments(&mut self, ui: &mut egui::Ui) {
        ui.heading("Key Assignments");
        ui.label(
            RichText::new("Map a physical key (+ optional modifiers) to a remap or macro.")
                .color(GRAY),
        );
        ui.separator();

        // Snapshot data needed for rendering
        let modifiers: Vec<(Uuid, String)> = self.profile()
            .map(|p| p.modifiers.iter().map(|m| (m.id, m.name.clone())).collect())
            .unwrap_or_default();
        let macro_list: Vec<(Uuid, String, FireMode)> = self.profile()
            .map(|p| p.macros.iter().map(|m| (m.id, m.name.clone(), m.fire.clone())).collect())
            .unwrap_or_default();
        let assignment_count = self.profile().map(|p| p.assignments.len()).unwrap_or(0);

        let mut delete_assignment: Option<usize> = None;
        let mut clear_assignment_target: Option<usize> = None;
        let mut start_learn_source: Option<usize> = None;
        let mut start_learn_remap:  Option<usize> = None;
        let mut set_target_macro: Option<(usize, Option<Uuid>)> = None;
        let mut set_trigger_mode: Option<(usize, TriggerMode)> = None;
        let mut clear_device_filter: Option<usize> = None;
        let mut cancel_learn = false;
        let mut needs_save = false;

        TableBuilder::new(ui)
            .striped(true)
            .vscroll(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(160.0).resizable(true))  // Source Key (with modifiers)
            .column(Column::initial(120.0).resizable(true))  // Source Device
            .column(Column::initial(200.0).resizable(true))  // Target
            .column(Column::exact(100.0))                    // Trigger
            .column(Column::exact(55.0))                     // Actions
            .header(20.0, |mut header| {
                header.col(|ui| { ui.strong("Source Key"); });
                header.col(|ui| { ui.strong("Source Device"); });
                header.col(|ui| { ui.strong("Target"); });
                header.col(|ui| { ui.strong("Trigger"); });
                header.col(|ui| {});
            })
            .body(|mut body| {
                for i in 0..assignment_count {
                    let Some(profile_idx) = self.selected else { return };
                    let a = &self.profiles[profile_idx].assignments[i];

                    let source_key    = a.source_key.clone();
                    let source_device = a.source_device.clone();
                    let remap_key     = a.remap_key.clone();
                    let macro_id      = a.macro_id;
                    let held_mods     = a.modifiers.clone();
                    let trigger_mode  = a.trigger_mode.clone();

                    // Build "ModName + ModName + KEY" display for the source key.
                    let source_label: String = if held_mods.is_empty() || source_key.is_empty() {
                        if source_key.is_empty() { "— not set —".into() } else { source_key.clone() }
                    } else {
                        let mod_labels: Vec<&str> = modifiers.iter()
                            .filter(|(id, _)| held_mods.contains(id))
                            .map(|(_, name)| name.as_str())
                            .collect();
                        format!("{} + {}", mod_labels.join(" + "), source_key)
                    };

                    body.row(24.0, |mut row| {
                        // Source Key
                        row.col(|ui| {
                            let is_learning = matches!(
                                self.learn_target,
                                Some(LearnTarget::AssignmentSource(j)) if j == i
                            );
                            if is_learning {
                                ui.spinner();
                                ui.label("Press a key…");
                                if ui.small_button("Cancel").clicked() { cancel_learn = true; }
                            } else {
                                if ui.button(&source_label)
                                    .on_hover_text("Click to re-learn source key")
                                    .clicked()
                                {
                                    start_learn_source = Some(i);
                                }
                            }
                        });

                        // Source Device
                        row.col(|ui| {
                            if let Some(ref dev) = source_device {
                                let short = if dev.len() > 12 { format!("{}…", &dev[..12]) } else { dev.clone() };
                                ui.label(RichText::new(&short).small().weak())
                                    .on_hover_text(dev.as_str());
                                if ui.small_button("x").on_hover_text("Clear device filter").clicked() {
                                    clear_device_filter = Some(i);
                                }
                            }
                        });

                        // Target
                        row.col(|ui| {
                            let is_learning_remap = matches!(
                                self.learn_target,
                                Some(LearnTarget::AssignmentRemap(j)) if j == i
                            );
                            if is_learning_remap {
                                ui.spinner();
                                ui.label("Press target key…");
                                if ui.small_button("Cancel").clicked() { cancel_learn = true; }
                            } else if remap_key.is_some() {
                                let label = remap_key.as_deref().unwrap_or("Key…")
                                    .split('+').collect::<Vec<_>>().join(" + ");
                                if ui.button(&label)
                                    .on_hover_text("Click to re-learn remap target key")
                                    .clicked()
                                {
                                    start_learn_remap = Some(i);
                                }
                            } else if macro_id.is_some() {
                                let macro_name = macro_id
                                    .and_then(|mid| macro_list.iter().find(|(id, _, _)| *id == mid))
                                    .map(|(_, n, _)| n.as_str())
                                    .unwrap_or("(missing)");
                                let mut chosen = macro_id;
                                egui::ComboBox::from_id_source(format!("assign_macro_{i}"))
                                    .selected_text(macro_name)
                                    .width(ui.available_width())
                                    .show_ui(ui, |ui| {
                                        for (mid, mname, _) in &macro_list {
                                            if ui.selectable_label(chosen == Some(*mid), mname).clicked() {
                                                set_target_macro = Some((i, Some(*mid)));
                                                chosen = Some(*mid);
                                            }
                                        }
                                    });
                            } else {
                                if ui.button("Key…").on_hover_text("Click to learn a remap target key").clicked() {
                                    start_learn_remap = Some(i);
                                }
                                ui.label("or");
                                egui::ComboBox::from_id_source(format!("assign_macro_{i}"))
                                    .selected_text("Macro…")
                                    .show_ui(ui, |ui| {
                                        for (mid, mname, _) in &macro_list {
                                            if ui.selectable_label(false, mname).clicked() {
                                                set_target_macro = Some((i, Some(*mid)));
                                            }
                                        }
                                    });
                            }
                        });

                        // Trigger
                        row.col(|ui| {
                            let is_single = macro_id
                                .and_then(|mid| macro_list.iter().find(|(id, _, _)| *id == mid))
                                .map(|(_, _, fire)| *fire == FireMode::Single)
                                .unwrap_or(false);
                            if is_single {
                                let cb = egui::ComboBox::from_id_source(format!("assign_trigger_{i}"))
                                    .selected_text(trigger_mode_label(&trigger_mode))
                                    .width(ui.available_width())
                                    .show_ui(ui, |ui| {
                                        for mode in [TriggerMode::Any, TriggerMode::QuickPress, TriggerMode::ShortHold] {
                                            let lbl = trigger_mode_label(&mode);
                                            if ui.selectable_label(trigger_mode == mode, lbl)
                                                .on_hover_text(trigger_mode_tooltip(&mode))
                                                .clicked()
                                            {
                                                set_trigger_mode = Some((i, mode));
                                            }
                                        }
                                    });
                                cb.response.on_hover_text(trigger_mode_tooltip(&trigger_mode));
                            }
                        });

                        // Actions
                        row.col(|ui| {
                            if ui.small_button("Clr").on_hover_text("Clear macro or remap target").clicked() {
                                clear_assignment_target = Some(i);
                            }
                            if ui.small_button("Del").on_hover_text("Delete assignment").clicked() {
                                delete_assignment = Some(i);
                            }
                        });
                    });
                }
            });

        if cancel_learn { self.cancel_learn(); }

        // Apply mutations
        let Some(profile_idx) = self.selected else { return };

        if let Some(i) = delete_assignment {
            self.profiles[profile_idx].assignments.remove(i);
            needs_save = true;
        }
        if let Some(i) = clear_assignment_target {
            let a = &mut self.profiles[profile_idx].assignments[i];
            a.macro_id = None;
            a.remap_key = None;
            a.trigger_mode = TriggerMode::Any;
            needs_save = true;
        }
        if let Some(i) = start_learn_source {
            self.start_learn(LearnTarget::AssignmentSource(i));
        }
        if let Some(i) = start_learn_remap {
            self.start_learn(LearnTarget::AssignmentRemap(i));
        }
        if let Some((i, mid)) = set_target_macro {
            let a = &mut self.profiles[profile_idx].assignments[i];
            a.macro_id  = mid;
            a.remap_key = if mid.is_some() { None } else { a.remap_key.clone() };
            if mid.is_none() {
                a.trigger_mode = TriggerMode::Any;
            }
            needs_save = true;
        }
        if let Some((i, mode)) = set_trigger_mode {
            self.profiles[profile_idx].assignments[i].trigger_mode = mode;
            needs_save = true;
        }
        if let Some(i) = clear_device_filter {
            self.profiles[profile_idx].assignments[i].source_device = None;
            needs_save = true;
        }

        ui.separator();
        self.show_add_assignment(ui, profile_idx, &mut needs_save);

        if needs_save {
            store::save_profile(&self.profiles[profile_idx]);
        }
    }

    pub(super) fn show_add_assignment(&mut self, ui: &mut egui::Ui, _profile_idx: usize, _needs_save: &mut bool) {
        let is_learning = matches!(self.learn_target, Some(LearnTarget::AssignmentSource(usize::MAX)));
        // We reuse AssignmentSource(usize::MAX) as "learning for a new assignment"

        if !self.adding_assignment {
            if ui.button("+ New Assignment").clicked() {
                self.adding_assignment = true;
                self.start_learn(LearnTarget::AssignmentSource(usize::MAX));
            }
            return;
        }

        if is_learning {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Press the key to assign…");
                if ui.small_button("Cancel").clicked() {
                    self.adding_assignment = false;
                    self.cancel_learn();
                }
            });
        }
    }

    // ── Blocked keys section ──────────────────────────────────────────────────

    pub(super) fn show_blocked(&mut self, ui: &mut egui::Ui) {
        ui.heading("Blocked Keys");
        ui.label(
            RichText::new("Keys in this list cannot be used as assignment sources.")
                .color(GRAY),
        );
        ui.separator();

        let blocked: Vec<String> = self.profile()
            .map(|p| p.blocked_keys.clone())
            .unwrap_or_default();

        let mut delete_idx: Option<usize> = None;

        egui::Grid::new("blocked_grid")
            .num_columns(2)
            .striped(true)
            .min_col_width(160.0)
            .show(ui, |ui| {
                ui.strong("Key");
                ui.label("");
                ui.end_row();

                for (i, key) in blocked.iter().enumerate() {
                    ui.label(key);
                    if ui.small_button("x").on_hover_text("Remove").clicked() {
                        delete_idx = Some(i);
                    }
                    ui.end_row();
                }
            });

        if let Some(i) = delete_idx {
            if let Some(p) = self.profile_mut() {
                p.blocked_keys.remove(i);
            }
            self.save_current();
        }

        ui.separator();

        let is_learning = matches!(self.learn_target, Some(LearnTarget::NewBlockedKey));
        if is_learning {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Press a key to block…");
                if ui.small_button("Cancel").clicked() {
                    self.cancel_learn();
                }
            });
        } else if ui.button("+ Block Key").clicked() {
            self.start_learn(LearnTarget::NewBlockedKey);
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Defaults: KEY_ESC, BTN_LEFT, BTN_RIGHT.").small().color(GRAY));
            if ui.small_button("Restore defaults").clicked() {
                if let Some(p) = self.profile_mut() {
                    for key in ["KEY_ESC", "BTN_LEFT", "BTN_RIGHT"] {
                        if !p.blocked_keys.iter().any(|k| k == key) {
                            p.blocked_keys.push(key.into());
                        }
                    }
                }
                self.save_current();
            }
        });
    }
}

// ── Trigger mode helpers ──────────────────────────────────────────────────────

fn trigger_mode_label(mode: &TriggerMode) -> &'static str {
    match mode {
        TriggerMode::Any        => "Any",
        TriggerMode::QuickPress => "Short",
        TriggerMode::ShortHold  => "Long",
    }
}

fn trigger_mode_tooltip(mode: &TriggerMode) -> &'static str {
    match mode {
        TriggerMode::Any        => "Fire on any press duration",
        TriggerMode::QuickPress => "Short press: < 250 ms",
        TriggerMode::ShortHold  => "Long press: 250–500 ms",
    }
}
