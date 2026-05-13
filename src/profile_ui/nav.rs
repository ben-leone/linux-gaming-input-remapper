use eframe::egui::{self, RichText};

use crate::config::{store, Profile};
use super::{ProfileApp, Section, GREEN, RED, GRAY, AMBER};

impl ProfileApp {
    // ── Top bar ───────────────────────────────────────────────────────────────

    pub(super) fn show_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.show_auto_switch(ui);
                ui.separator();
                self.show_run_stop(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.show_game_link_summary(ui);
                });
            });
        });
    }

    fn show_run_stop(&mut self, ui: &mut egui::Ui) {
        if self.engine.is_active() {
            if ui.add(egui::Button::new(RichText::new("Stop").color(RED))).clicked() {
                self.engine_err.clear();
                self.engine.stop();
                self.auto_detected_uuid = None;
            }
            let can_reapply = self.selected.is_some();
            ui.add_enabled_ui(can_reapply, |ui| {
                if ui.button("Re-apply").on_hover_text("Save profile and restart remapping").clicked() {
                    if let Some(i) = self.selected {
                        let uuid = self.profiles[i].id;
                        self.engine_err.clear();
                        self.save_current();
                        if let Err(e) = self.engine.load(uuid) {
                            self.engine_err = e;
                        }
                        self.auto_detected_uuid = None;
                    }
                }
            });
        } else {
            let can_run = self.selected.is_some();
            ui.add_enabled_ui(can_run, |ui| {
                if ui.add(egui::Button::new(RichText::new("Run").color(GREEN))).clicked() {
                    if let Some(i) = self.selected {
                        let uuid = self.profiles[i].id;
                        self.engine_err.clear();
                        self.save_current();
                        if let Err(e) = self.engine.load(uuid) {
                            self.engine_err = e;
                        }
                        self.auto_detected_uuid = None;
                    }
                }
            });
        }

        if !self.engine_err.is_empty() {
            ui.label(RichText::new(&self.engine_err).color(RED).small());
        }
    }

    fn show_auto_switch(&mut self, ui: &mut egui::Ui) {
        let (color, label) = if self.settings.auto_switch_profiles {
            match self.engine.active_profile_name() {
                Some(name) => (GREEN, format!("Running [{}]", name)),
                None       => (AMBER, "Auto-detect: ON".to_string()),
            }
        } else {
            (GRAY, "Auto-detect: OFF".to_string())
        };
        if ui.add(egui::Button::new(RichText::new(&label).color(color))).clicked() {
            self.settings.auto_switch_profiles = !self.settings.auto_switch_profiles;
            store::save_settings(&self.settings);
            if self.settings.auto_switch_profiles {
                self.scanner_dirty = true;
            } else {
                self.engine.stop_auto_detect();
            }
        }
    }

    fn show_game_link_summary(&mut self, ui: &mut egui::Ui) {
        let Some(_) = self.selected else { return };

        let link_text = self.profile()
            .and_then(|p| p.game_link.as_ref())
            .map(|g| format!("🎮 {}", g.display_name))
            .unwrap_or_else(|| "No game linked".into());

        let color = if self.profile().and_then(|p| p.game_link.as_ref()).is_some() {
            GREEN
        } else {
            GRAY
        };

        ui.label(RichText::new(&link_text).color(color).small());
        if ui.small_button("Edit").clicked() {
            self.game_link_draft = self.profile()
                .and_then(|p| p.game_link.clone())
                .unwrap_or_default();
            self.game_link_open = true;
        }
    }

    // ── Profiles panel ────────────────────────────────────────────────────────

    pub(super) fn show_profiles_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("profiles_panel").exact_width(160.0).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.strong("Profiles");
            ui.add_space(2.0);

            ui.horizontal(|ui| {
                if ui.button("New").clicked() {
                    self.new_profile_open = true;
                    self.new_profile_name.clear();
                }
                ui.add_enabled_ui(self.selected.is_some(), |ui| {
                    if ui.button("Delete").clicked() {
                        self.delete_profile_confirm = true;
                    }
                });
            });
            ui.add_space(4.0);

            let mut new_sel: Option<usize> = None;
            let scroll_height = (ui.available_height() - 52.0).max(40.0);
            egui::ScrollArea::vertical()
                .id_source("profile_list_scroll")
                .max_height(scroll_height)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    for (i, p) in self.profiles.iter().enumerate() {
                        if ui.selectable_label(self.selected == Some(i), &p.name).clicked() {
                            new_sel = Some(i);
                        }
                    }
                });

            if let Some(i) = new_sel {
                if self.selected != Some(i) {
                    self.selected = Some(i);
                    self.selected_macro = None;
                }
            }

            ui.add_space(4.0);
            ui.separator();
            ui.label(RichText::new("Debug").small().color(GRAY));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if ui.small_button("Capture").on_hover_text("Open key capture tool").clicked() {
                    spawn_debug("capture");
                }
                if ui.small_button("Log").on_hover_text("Open continuous event log").clicked() {
                    spawn_debug("log");
                }
                if ui.small_button("HID").on_hover_text("Open raw HID probe").clicked() {
                    spawn_debug("hid");
                }
            });
        });
    }

    // ── Dialogs ───────────────────────────────────────────────────────────────

    pub(super) fn show_new_profile_dialog(&mut self, ctx: &egui::Context) {
        if !self.new_profile_open { return; }

        let mut create = false;
        let mut close  = false;
        let mut open   = true;

        egui::Window::new("New Profile")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Profile name:");
                let resp = ui.text_edit_singleline(&mut self.new_profile_name);
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    create = true;
                }
                ui.horizontal(|ui| {
                    let can = !self.new_profile_name.trim().is_empty();
                    ui.add_enabled_ui(can, |ui| {
                        if ui.button("Create").clicked() { create = true; }
                    });
                    if ui.button("Cancel").clicked() { close = true; }
                });
            });

        if create {
            let name = self.new_profile_name.trim().to_string();
            if !name.is_empty() {
                let p = Profile::new(name);
                store::save_profile(&p);
                self.profiles.push(p);
                self.profiles.sort_by(|a, b| a.name.cmp(&b.name));
                self.selected = self.profiles.len().checked_sub(1);
                self.new_profile_open = false;
            }
        }
        if close || !open { self.new_profile_open = false; }
    }

    pub(super) fn show_delete_profile_dialog(&mut self, ctx: &egui::Context) {
        if !self.delete_profile_confirm { return; }

        let name = self.selected
            .and_then(|i| self.profiles.get(i))
            .map(|p| p.name.clone())
            .unwrap_or_default();

        let mut open = true;
        let mut confirmed = false;
        let mut cancelled = false;

        egui::Window::new("Delete Profile")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Delete \"{}\"?", name));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() { confirmed = true; }
                    if ui.button("Cancel").clicked() { cancelled = true; }
                });
            });

        if confirmed {
            if let Some(i) = self.selected {
                let p = self.profiles.remove(i);
                store::delete_profile(&p);
                self.selected = None;
                self.selected_macro = None;
            }
            self.delete_profile_confirm = false;
        }
        if cancelled || !open { self.delete_profile_confirm = false; }
    }

    pub(super) fn show_game_link_dialog(&mut self, ctx: &egui::Context) {
        if !self.game_link_open { return; }

        let mut open  = true;
        let mut close = false;
        let mut save  = false;
        let mut clear = false;

        egui::Window::new("Game Link")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("game_link_grid").num_columns(2).show(ui, |ui| {
                    ui.label("Display name:");
                    ui.text_edit_singleline(&mut self.game_link_draft.display_name);
                    ui.end_row();

                    ui.label("Steam App ID:");
                    let mut steam_str = self.game_link_draft.steam_id
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    if ui.text_edit_singleline(&mut steam_str).changed() {
                        self.game_link_draft.steam_id = steam_str.trim().parse().ok();
                    }
                    ui.end_row();

                    ui.label("Process name:");
                    let mut proc = self.game_link_draft.process.clone().unwrap_or_default();
                    if ui.text_edit_singleline(&mut proc).changed() {
                        self.game_link_draft.process =
                            if proc.trim().is_empty() { None } else { Some(proc.trim().to_string()) };
                    }
                    ui.end_row();
                });

                ui.label(RichText::new(
                    "Auto-detect: launch your game after clicking Detect — \
                     we watch /proc for a new process with STEAM_GAME_ID set."
                ).small().color(GRAY));

                ui.horizontal(|ui| {
                    if ui.button("Save").clicked()       { save  = true; }
                    if ui.button("Clear Link").clicked() { clear = true; }
                    if ui.button("Cancel").clicked()     { close = true; }
                });
            });

        if save {
            let draft = self.game_link_draft.clone();
            let has_content = !draft.display_name.is_empty()
                || draft.steam_id.is_some()
                || draft.process.is_some();
            if let Some(p) = self.profile_mut() {
                p.game_link = if has_content { Some(draft) } else { None };
            }
            self.save_current();
            self.game_link_open = false;
        }
        if clear {
            if let Some(p) = self.profile_mut() { p.game_link = None; }
            self.save_current();
            self.game_link_open = false;
        }
        if close || !open { self.game_link_open = false; }
    }

    // ── Left nav ──────────────────────────────────────────────────────────────

    pub(super) fn show_nav(&mut self, ui: &mut egui::Ui) {
        for (sec, label) in [
            (Section::Modifiers,   "Modifiers"),
            (Section::Macros,      "Macros"),
            (Section::Assignments, "Assignments"),
            (Section::Blocked,     "Blocked"),
            (Section::AutoDetect,  "Auto Detect"),
        ] {
            if ui.selectable_label(self.section == sec, label).clicked() {
                self.section = sec;
            }
        }
        ui.separator();
        if ui.selectable_label(self.section == Section::Log, "Engine Log").clicked() {
            self.section = Section::Log;
        }
    }
}

// ── Debug launcher ────────────────────────────────────────────────────────────

fn spawn_debug(mode: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .args(["debug", "--mode", mode])
            .spawn();
    }
}
