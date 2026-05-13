use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::config::{store, AdvancedEvent, FireMode, LoopEvent, Macro, StepMode};
use super::{LearnTarget, ProfileApp, GREEN, GRAY, AMBER};

impl ProfileApp {
    // ── Macros section ────────────────────────────────────────────────────────

    pub(super) fn show_macros(&mut self, ui: &mut egui::Ui) {
        // Snapshot macro list for rendering
        let macro_list: Vec<(uuid::Uuid, String, FireMode, StepMode)> = self.profile()
            .map(|p| p.macros.iter().map(|m| (m.id, m.name.clone(), m.fire.clone(), m.mode.clone())).collect())
            .unwrap_or_default();

        let mut delete_macro: Option<usize> = None;
        let mut select_macro: Option<Option<usize>> = None;
        let mut new_macro    = false;
        let mut start_rec    = false;

        ui.horizontal_top(|ui| {
            // ── Left: macro list ────────────────────────────────────────────
            ui.vertical(|ui| {
                ui.set_min_width(180.0);
                ui.set_max_width(180.0);

                ui.heading("Macros");
                if ui.button("+ New Macro").clicked() {
                    new_macro = true;
                }
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_source("macro_list_scroll")
                    .show(ui, |ui| {
                        for (i, (_, name, fire, _)) in macro_list.iter().enumerate() {
                            let fire_label = match fire {
                                FireMode::Single => "1×",
                                FireMode::Loop   => "∞",
                            };
                            let selected = self.selected_macro == Some(i);
                            ui.horizontal(|ui| {
                                if ui.selectable_label(selected, format!("[{fire_label}] {name}")).clicked() {
                                    select_macro = Some(Some(i));
                                    self.adding_step = false;
                                }
                                if ui.small_button("x").clicked() {
                                    delete_macro = Some(i);
                                }
                            });
                        }
                    });
            });

            ui.separator();

            // ── Right: macro editor ─────────────────────────────────────────
            ui.vertical(|ui| {
                let Some(macro_idx) = self.selected_macro else {
                    ui.label(RichText::new("Select a macro to edit").color(GRAY));
                    return;
                };
                let Some(profile_idx) = self.selected else { return };

                let is_recording = self.recording;
                let mut changed  = false;

                // ── Name / Fire / Mode ── (borrow m, no self methods called)
                {
                    let m = &mut self.profiles[profile_idx].macros[macro_idx];

                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.add_enabled_ui(!is_recording, |ui| {
                            if ui.text_edit_singleline(&mut m.name).changed() { changed = true; }
                        });
                    });

                    ui.horizontal(|ui| {
                        ui.label("Fire:");
                        ui.add_enabled_ui(!is_recording, |ui| {
                            if ui.radio_value(&mut m.fire, FireMode::Single, "Single").changed() { changed = true; }
                            if ui.radio_value(&mut m.fire, FireMode::Loop,   "Loop").changed()   { changed = true; }
                        });
                    });

                    ui.horizontal(|ui| {
                        ui.label("Mode:");
                        ui.add_enabled_ui(!is_recording, |ui| {
                            let prev = m.mode.clone();
                            if ui.radio_value(&mut m.mode, StepMode::Simple, "Simple").changed() {
                                if prev == StepMode::Advanced { m.advanced_steps.clear(); }
                                changed = true;
                            }
                            if ui.radio_value(&mut m.mode, StepMode::Advanced, "Advanced")
                                .on_hover_text(
                                    "Advanced mode gives you fine-tuned control over individual \
                                     key press and release timings. Each event is placed at an \
                                     exact millisecond offset from the start of the macro."
                                )
                                .changed()
                            {
                                if prev == StepMode::Simple { m.simple_steps.clear(); }
                                changed = true;
                            }
                        });
                    });

                    ui.horizontal(|ui| {
                        ui.label("Start delay (ms):")
                            .on_hover_text("Delay before the first step fires.");
                        if ui.add(egui::DragValue::new(&mut m.start_delay_ms).speed(1)).changed() { changed = true; }
                        if m.fire == FireMode::Loop {
                            ui.separator();
                            ui.label("Loop gap (ms):")
                                .on_hover_text("Delay between the end of one loop iteration and the start of the next.");
                            if ui.add(egui::DragValue::new(&mut m.loop_delay_ms).speed(1)).changed() { changed = true; }
                        }
                    });
                } // m borrow ends — self methods are callable again

                ui.separator();

                // ── Record button / status ── (no m borrow needed)
                ui.horizontal(|ui| {
                    if is_recording {
                        ui.spinner();
                        ui.label(
                            RichText::new("Recording — press Escape to stop")
                                .color(AMBER)
                                .strong(),
                        );
                    } else if ui.button("⏺ Record")
                        .on_hover_text("Capture real keystrokes with actual timing. Press Escape when done.")
                        .clicked()
                    {
                        start_rec = true;
                    }
                });

                if is_recording {
                    egui::ScrollArea::vertical()
                        .id_source("record_feed")
                        .max_height(200.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            if self.record_buf.is_empty() {
                                ui.label(RichText::new("Waiting for input…").color(GRAY).italics());
                            } else {
                                egui::Grid::new("record_feed_grid")
                                    .num_columns(3)
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for entry in &self.record_buf {
                                            let (dir, color) = if entry.pressed {
                                                ("↓", GREEN)
                                            } else {
                                                ("↑", GRAY)
                                            };
                                            ui.label(RichText::new(format!("{}ms", entry.elapsed_ms)).color(GRAY).small());
                                            ui.label(RichText::new(dir).color(color));
                                            ui.label(RichText::new(&entry.name).monospace());
                                            ui.end_row();
                                        }
                                    });
                            }
                        });
                }

                ui.separator();

                // ── Step editor ── borrow m in its own scope, then call self methods after
                let current_mode = self.profiles[profile_idx].macros[macro_idx].mode.clone();

                match current_mode {
                    StepMode::Simple => {
                        if !is_recording {
                            let mut delete_step: Option<usize> = None;
                            let mut move_up:     Option<usize> = None;
                            let mut move_dn:     Option<usize> = None;

                            {
                                let m          = &mut self.profiles[profile_idx].macros[macro_idx];
                                let step_count = m.simple_steps.len();

                                TableBuilder::new(ui)
                                    .striped(true)
                                    .column(Column::initial(24.0))
                                    .column(Column::remainder().resizable(true))
                                    .column(Column::initial(70.0).resizable(true))
                                    .column(Column::initial(80.0).resizable(true))
                                    .column(Column::initial(42.0))
                                    .column(Column::initial(42.0))
                                    .column(Column::initial(48.0))
                                    .header(18.0, |mut h| {
                                        h.col(|ui| { ui.strong("#"); });
                                        h.col(|ui| { ui.strong("Key"); });
                                        h.col(|ui| { ui.strong("Hold (ms)"); });
                                        h.col(|ui| { ui.strong("Delay (ms)"); });
                                        h.col(|ui| { ui.strong("Up"); });
                                        h.col(|ui| { ui.strong("Down"); });
                                        h.col(|ui| { ui.strong("Delete"); });
                                    })
                                    .body(|mut body| {
                                        for i in 0..step_count {
                                            body.row(18.0, |mut row| {
                                                row.col(|ui| { ui.label(format!("{}", i + 1)); });
                                                row.col(|ui| {
                                                    if ui.add(egui::TextEdit::singleline(&mut m.simple_steps[i].action)
                                                        .desired_width(f32::INFINITY)).changed()
                                                    { changed = true; }
                                                });
                                                row.col(|ui| {
                                                    if ui.add(egui::DragValue::new(&mut m.simple_steps[i].hold_ms)
                                                        .speed(1)).changed()
                                                    { changed = true; }
                                                });
                                                row.col(|ui| {
                                                    if ui.add(egui::DragValue::new(&mut m.simple_steps[i].delay_after_ms)
                                                        .speed(1)).changed()
                                                    { changed = true; }
                                                });
                                                row.col(|ui| {
                                                    if i > 0 && ui.small_button("^").clicked() { move_up = Some(i); }
                                                });
                                                row.col(|ui| {
                                                    if i + 1 < step_count && ui.small_button("v").clicked() { move_dn = Some(i); }
                                                });
                                                row.col(|ui| {
                                                    if ui.small_button("x").clicked() { delete_step = Some(i); }
                                                });
                                            });
                                        }
                                    });
                            } // m borrow ends

                            if let Some(i) = delete_step {
                                self.profiles[profile_idx].macros[macro_idx].simple_steps.remove(i);
                                changed = true;
                            } else if let Some(i) = move_up {
                                self.profiles[profile_idx].macros[macro_idx].simple_steps.swap(i, i - 1);
                                changed = true;
                            } else if let Some(i) = move_dn {
                                self.profiles[profile_idx].macros[macro_idx].simple_steps.swap(i, i + 1);
                                changed = true;
                            }

                            ui.separator();
                            self.show_add_step(ui, profile_idx, macro_idx, &mut changed);
                        }
                    }
                    StepMode::Advanced => {
                        let is_empty = self.profiles[profile_idx].macros[macro_idx].advanced_steps.is_empty();
                        if is_empty {
                            ui.label(RichText::new(
                                "No steps yet — use ⏺ Record to capture key timings."
                            ).color(GRAY));
                        } else {
                            let mut delete_adv: Option<usize> = None;
                            let mut move_adv_up: Option<usize> = None;
                            let mut move_adv_dn: Option<usize> = None;

                            {
                                let m = &mut self.profiles[profile_idx].macros[macro_idx];
                                let step_count = m.advanced_steps.len();

                                TableBuilder::new(ui)
                                    .striped(true)
                                    .column(Column::initial(72.0).resizable(true))
                                    .column(Column::initial(100.0).resizable(true))
                                    .column(Column::remainder().resizable(true))
                                    .column(Column::initial(42.0))
                                    .column(Column::initial(42.0))
                                    .column(Column::initial(48.0))
                                    .header(18.0, |mut h| {
                                        h.col(|ui| { ui.strong("Time (ms)"); });
                                        h.col(|ui| { ui.strong("Event"); });
                                        h.col(|ui| { ui.strong("Key"); });
                                        h.col(|ui| { ui.strong("Up"); });
                                        h.col(|ui| { ui.strong("Down"); });
                                        h.col(|ui| { ui.strong("Delete"); });
                                    })
                                    .body(|mut body| {
                                        for i in 0..step_count {
                                            body.row(18.0, |mut row| {
                                                row.col(|ui| {
                                                    if ui.add(egui::DragValue::new(&mut m.advanced_steps[i].time_ms)
                                                        .speed(1)).changed()
                                                    { changed = true; }
                                                });
                                                row.col(|ui| {
                                                    egui::ComboBox::from_id_source(format!("adv_ev_{i}"))
                                                        .selected_text(format!("{:?}", m.advanced_steps[i].event))
                                                        .width(ui.available_width())
                                                        .show_ui(ui, |ui| {
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, AdvancedEvent::KeyDown,   "KeyDown").clicked()   { changed = true; }
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, AdvancedEvent::KeyUp,     "KeyUp").clicked()     { changed = true; }
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, AdvancedEvent::MouseDown, "MouseDown").clicked() { changed = true; }
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, AdvancedEvent::MouseUp,   "MouseUp").clicked()   { changed = true; }
                                                        });
                                                });
                                                row.col(|ui| {
                                                    if ui.add(egui::TextEdit::singleline(&mut m.advanced_steps[i].key)
                                                        .desired_width(f32::INFINITY)).changed()
                                                    { changed = true; }
                                                });
                                                row.col(|ui| {
                                                    if i > 0 && ui.small_button("^").clicked() { move_adv_up = Some(i); }
                                                });
                                                row.col(|ui| {
                                                    if i + 1 < step_count && ui.small_button("v").clicked() { move_adv_dn = Some(i); }
                                                });
                                                row.col(|ui| {
                                                    if ui.small_button("x").clicked() { delete_adv = Some(i); }
                                                });
                                            });
                                        }
                                    });
                            }

                            if let Some(i) = delete_adv {
                                self.profiles[profile_idx].macros[macro_idx].advanced_steps.remove(i);
                                changed = true;
                            } else if let Some(i) = move_adv_up {
                                self.profiles[profile_idx].macros[macro_idx].advanced_steps.swap(i - 1, i);
                                changed = true;
                            } else if let Some(i) = move_adv_dn {
                                self.profiles[profile_idx].macros[macro_idx].advanced_steps.swap(i, i + 1);
                                changed = true;
                            }
                        }
                    }
                }

                // ── Events section (loop macros only) ──────────────────────────
                let current_fire = self.profiles[profile_idx].macros[macro_idx].fire.clone();
                if current_fire == FireMode::Loop && !is_recording {
                    ui.separator();
                    self.show_loop_events(ui, profile_idx, macro_idx, &mut changed);
                }

                if changed {
                    store::save_profile(&self.profiles[profile_idx]);
                }
            });
        });

        // Apply list-level mutations after the closure
        if start_rec {
            self.start_record_session();
        }
        if new_macro {
            if let Some(p) = self.profile_mut() {
                let m = Macro::new("New Macro");
                p.macros.push(m);
                let idx = p.macros.len() - 1;
                store::save_profile(p);
                self.selected_macro = Some(idx);
            }
        }
        if let Some(i) = delete_macro {
            if let Some(profile_idx) = self.selected {
                let p = &mut self.profiles[profile_idx];
                let macro_id = p.macros[i].id;
                for a in &mut p.assignments {
                    if a.macro_id == Some(macro_id) {
                        a.macro_id = None;
                    }
                }
                p.macros.remove(i);
                store::save_profile(p);
                self.selected_macro = match self.selected_macro {
                    Some(sel) if sel == i => None,
                    Some(sel) if sel > i  => Some(sel - 1),
                    other => other,
                };
            }
        }
        if let Some(sel) = select_macro {
            self.selected_macro = sel;
        }
    }

    pub(super) fn show_add_step(&mut self, ui: &mut egui::Ui, _profile_idx: usize, _macro_idx: usize, _changed: &mut bool) {
        let is_learning = matches!(self.learn_target, Some(LearnTarget::NewMacroStep));

        if !self.adding_step {
            if ui.button("+ Add Step").clicked() {
                self.adding_step = true;
                self.new_step_action.clear();
            }
            return;
        }

        // Timing fields first so user can set them before learning the key.
        ui.horizontal(|ui| {
            ui.label("Hold (ms):");
            ui.add(egui::TextEdit::singleline(&mut self.new_step_hold).desired_width(50.0));
            ui.label("Delay after (ms):");
            ui.add(egui::TextEdit::singleline(&mut self.new_step_delay).desired_width(50.0));
        });

        // Learn key — pressing a key immediately adds the step.
        ui.horizontal(|ui| {
            if is_learning {
                ui.spinner();
                ui.label("Press a key or button…");
                if ui.small_button("Cancel").clicked() {
                    self.cancel_learn();
                }
            } else {
                if ui.button("Learn Key/Button").on_hover_text("Press a key; the step is added immediately.").clicked() {
                    self.start_learn(LearnTarget::NewMacroStep);
                }
                if ui.small_button("x Cancel").clicked() {
                    self.adding_step = false;
                    self.cancel_learn();
                }
            }
        });
    }

    pub(super) fn show_loop_events(&mut self, ui: &mut egui::Ui, profile_idx: usize, macro_idx: usize, changed: &mut bool) {
        ui.strong("Events");
        ui.label(
            RichText::new(
                "Events call a Single macro at specific loop lifecycle points.\n\
                 Order: 0 = before every iteration  |  -1 = on key release  |  N = every N cycles"
            )
            .color(GRAY),
        );

        // Snapshot the event list and the list of valid target macros (Single, no events, not self).
        let current_macro_id = self.profiles[profile_idx].macros[macro_idx].id;
        let valid_targets: Vec<(uuid::Uuid, String)> = self.profiles[profile_idx].macros
            .iter()
            .filter(|m| {
                m.id != current_macro_id
                    && m.fire == FireMode::Single
                    && m.events.is_empty()
            })
            .map(|m| (m.id, m.name.clone()))
            .collect();

        let event_count = self.profiles[profile_idx].macros[macro_idx].events.len();

        let mut delete_ev: Option<usize> = None;
        let mut set_ev_macro:     Option<(usize, uuid::Uuid)> = None;
        let mut set_ev_order:     Option<(usize, i32)> = None;
        let mut set_ev_min_loops: Option<(usize, u32)> = None;
        let mut set_ev_delay:     Option<(usize, u32)> = None;

        for i in 0..event_count {
            let ev = &self.profiles[profile_idx].macros[macro_idx].events[i];
            let ev_id = ev.id;
            let ev_macro_id  = ev.macro_id;
            let ev_order     = ev.order;
            let ev_min_loops = ev.min_loops;
            let ev_delay_ms  = ev.delay_ms;

            ui.horizontal(|ui| {
                // Order field
                ui.label("Order:");
                let mut order_val = ev_order;
                if ui.add(
                    egui::DragValue::new(&mut order_val)
                        .speed(1)
                        .range(-1..=i32::MAX)
                )
                .on_hover_text("0 = pre-iter  |  -1 = on release  |  N = every N cycles")
                .changed()
                {
                    set_ev_order = Some((i, order_val));
                }

                // Macro dropdown — filtered to valid Single macros
                ui.label("→");
                let macro_name = valid_targets.iter()
                    .find(|(id, _)| *id == ev_macro_id)
                    .map(|(_, n)| n.as_str())
                    .unwrap_or("(select)");
                egui::ComboBox::from_id_source(format!("ev_macro_{}", ev_id))
                    .selected_text(macro_name)
                    .show_ui(ui, |ui| {
                        for (mid, mname) in &valid_targets {
                            if ui.selectable_label(ev_macro_id == *mid, mname).clicked() {
                                set_ev_macro = Some((i, *mid));
                            }
                        }
                        if valid_targets.is_empty() {
                            ui.label(
                                RichText::new("No eligible Single macros")
                                    .small()
                                    .color(GRAY),
                            );
                        }
                    });

                // Min loops — only shown for end events
                if ev_order == -1 {
                    ui.label("Min loops:");
                    let mut ml = ev_min_loops;
                    if ui.add(egui::DragValue::new(&mut ml).speed(1))
                        .on_hover_text("Minimum complete cycles before this end event fires")
                        .changed()
                    {
                        set_ev_min_loops = Some((i, ml));
                    }
                }

                // Delay before invoking the event macro
                ui.label("Delay (ms):");
                let mut dms = ev_delay_ms;
                if ui.add(egui::DragValue::new(&mut dms).speed(1).range(0..=5000))
                    .on_hover_text("Wait this many milliseconds before firing the event macro")
                    .changed()
                {
                    set_ev_delay = Some((i, dms));
                }

                // Delete button
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("x").on_hover_text("Remove event").clicked() {
                        delete_ev = Some(i);
                    }
                });
            });
        }

        // Add event button
        let first_valid = valid_targets.first().map(|(id, _)| *id);
        if ui.button("+ Add Event").clicked() {
            if let Some(target_id) = first_valid.or_else(|| {
                // Use a nil UUID as placeholder if no valid targets exist yet
                Some(uuid::Uuid::nil())
            }) {
                let ev = LoopEvent {
                    id: uuid::Uuid::new_v4(),
                    macro_id: target_id,
                    order: 1,
                    min_loops: 0,
                    delay_ms: 100,
                };
                self.profiles[profile_idx].macros[macro_idx].events.push(ev);
                *changed = true;
            }
        }

        // Apply mutations
        if let Some(i) = delete_ev {
            self.profiles[profile_idx].macros[macro_idx].events.remove(i);
            *changed = true;
        }
        if let Some((i, mid)) = set_ev_macro {
            self.profiles[profile_idx].macros[macro_idx].events[i].macro_id = mid;
            *changed = true;
        }
        if let Some((i, order)) = set_ev_order {
            self.profiles[profile_idx].macros[macro_idx].events[i].order = order;
            *changed = true;
        }
        if let Some((i, ml)) = set_ev_min_loops {
            self.profiles[profile_idx].macros[macro_idx].events[i].min_loops = ml;
            *changed = true;
        }
        if let Some((i, d)) = set_ev_delay {
            self.profiles[profile_idx].macros[macro_idx].events[i].delay_ms = d;
            *changed = true;
        }
    }
}
