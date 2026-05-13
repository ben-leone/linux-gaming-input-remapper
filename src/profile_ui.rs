use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use uuid::Uuid;

use crate::config::{
    store, AppSettings, AutoDetectTarget, FireMode, GameLink, KeyAssignment, LoopEvent, Macro,
    ModifierKey, Profile, SimpleStep, StepMode, TriggerMode,
};
use crate::engine::DetectEvent;
use crate::devices::DeviceReader;

const GREEN: Color32 = Color32::from_rgb(80, 200, 80);
const RED:   Color32 = Color32::from_rgb(200, 80, 80);
const GRAY:  Color32 = Color32::from_rgb(140, 140, 140);
const AMBER: Color32 = Color32::from_rgb(255, 160, 0);

// ── Macro recording ───────────────────────────────────────────────────────────

enum RecordMsg {
    Key { name: String, pressed: bool, elapsed_ms: u32 },
    Stop,
}

struct RecordedKey {
    name:       String,
    pressed:    bool,
    elapsed_ms: u32,
}

// ── Learn target ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum LearnTarget {
    NewModifier,
    NewMacroStep,
    NewBlockedKey,
    AssignmentSource(usize),
    AssignmentRemap(usize),
}

// ── Section nav ───────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
enum Section {
    Modifiers,
    Macros,
    Assignments,
    Blocked,
    AutoDetect,
    Log,
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct ProfileApp {
    settings: AppSettings,
    profiles: Vec<Profile>,
    selected: Option<usize>,
    section:  Section,

    // Key capture
    learn_target:  Option<LearnTarget>,
    learn_rx:      Option<mpsc::Receiver<(String, String)>>,
    /// Holds reader thread handles for the duration of a learn session.
    /// Dropping these signals the threads to stop.
    learn_handles: Vec<DeviceReader>,

    // Modifiers panel
    adding_modifier: bool,

    // Macros panel
    selected_macro:  Option<usize>,
    adding_step:     bool,
    new_step_action: String,
    new_step_hold:   String,
    new_step_delay:  String,

    // Macro recording
    recording:      bool,
    record_rx:      Option<mpsc::Receiver<RecordMsg>>,
    record_handles: Vec<DeviceReader>,
    record_buf:     Vec<RecordedKey>,

    // Assignments panel
    adding_assignment: bool,

    // New profile dialog
    new_profile_open: bool,
    new_profile_name: String,

    // Delete profile confirm dialog
    delete_profile_confirm: bool,

    // Game link dialog
    game_link_open:  bool,
    game_link_draft: GameLink,

    // Remapping engine
    engine:     crate::engine::Engine,
    engine_err: String,

    // Warning popup (blocked key or duplicate assignment)
    warn_popup: Option<String>,

    // Auto detect panel
    adding_fragment_to: Option<usize>,
    new_fragment_text:  String,

    // Scanner lifecycle
    scanner_dirty:      bool,
    auto_detected_uuid: Option<Uuid>,

    // Engine log
    log_enabled: bool,
    log_entries: Vec<String>,
}

impl ProfileApp {
    fn build(cc: &eframe::CreationContext<'_>) -> Self {
        let _ = cc;
        let settings = store::load_settings();
        let profiles = store::load_profiles();
        let auto_switch_on = settings.auto_switch_profiles;
        Self {
            settings,
            profiles,
            selected: None,
            section:  Section::Modifiers,
            learn_target:  None,
            learn_rx:      None,
            learn_handles: Vec::new(),
            adding_modifier: false,
            selected_macro:  None,
            adding_step:     false,
            new_step_action: String::new(),
            new_step_hold:   "40".into(),
            new_step_delay:  "60".into(),
            recording:      false,
            record_rx:      None,
            record_handles: Vec::new(),
            record_buf:     Vec::new(),
            adding_assignment: false,
            new_profile_open: false,
            new_profile_name: String::new(),
            delete_profile_confirm: false,
            game_link_open:  false,
            game_link_draft: GameLink::default(),
            engine:     crate::engine::Engine::new(),
            engine_err: String::new(),
            warn_popup: None,
            adding_fragment_to: None,
            new_fragment_text:  String::new(),
            scanner_dirty:      auto_switch_on,
            auto_detected_uuid: None,
            log_enabled: false,
            log_entries: Vec::new(),
        }
    }

    // ── Profile helpers ───────────────────────────────────────────────────────

    fn profile(&self) -> Option<&Profile> {
        self.selected.and_then(|i| self.profiles.get(i))
    }

    fn profile_mut(&mut self) -> Option<&mut Profile> {
        self.selected.and_then(|i| self.profiles.get_mut(i))
    }

    fn save_current(&mut self) {
        if let Some(p) = self.profile() {
            store::save_profile(p);
        }
        if self.settings.auto_switch_profiles {
            self.scanner_dirty = true;
        }
    }

    // ── Key learning ──────────────────────────────────────────────────────────

    fn poll_learn(&mut self) -> Option<(String, String)> {
        let result = self.learn_rx.as_ref()?.try_recv().ok()?;
        self.learn_rx      = None;
        self.learn_handles = Vec::new();
        Some(result)
    }

    fn start_learn(&mut self, target: LearnTarget) {
        let mod_keys: Vec<String> = match &target {
            LearnTarget::AssignmentSource(_) | LearnTarget::AssignmentRemap(_) => self.profile()
                .map(|p| p.modifiers.iter().map(|m| m.key.clone()).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        self.learn_target = Some(target);
        let (rx, handles) = start_key_learn(mod_keys);
        self.learn_rx      = Some(rx);
        self.learn_handles = handles;
    }

    fn cancel_learn(&mut self) {
        self.learn_target  = None;
        self.learn_rx      = None;
        self.learn_handles = Vec::new(); // drops handles → threads stop
    }

    // ── Macro recording ───────────────────────────────────────────────────────

    fn start_record_session(&mut self) {
        let (rx, handles) = start_recording();
        self.recording      = true;
        self.record_rx      = Some(rx);
        self.record_handles = handles;
        self.record_buf     = Vec::new();
    }

    /// Drains the record channel into `record_buf`.  On Stop, converts the
    /// buffer to steps and appends them to the currently selected macro.
    fn poll_recording(&mut self) {
        loop {
            let Some(rx) = self.record_rx.as_ref() else { break };
            match rx.try_recv() {
                Ok(RecordMsg::Key { name, pressed, elapsed_ms }) => {
                    self.record_buf.push(RecordedKey { name, pressed, elapsed_ms });
                }
                Ok(RecordMsg::Stop) => {
                    self.finish_recording();
                    break;
                }
                Err(mpsc::TryRecvError::Empty)        => break,
                Err(mpsc::TryRecvError::Disconnected) => { self.finish_recording(); break; }
            }
        }
    }

    fn finish_recording(&mut self) {
        self.recording      = false;
        self.record_rx      = None;
        self.record_handles = Vec::new();

        let Some(profile_idx) = self.selected     else { self.record_buf.clear(); return };
        let Some(macro_idx)   = self.selected_macro else { self.record_buf.clear(); return };

        let buf = std::mem::take(&mut self.record_buf);
        let m   = &mut self.profiles[profile_idx].macros[macro_idx];

        match m.mode {
            StepMode::Simple   => m.simple_steps.extend(buf_to_simple_steps(&buf)),
            StepMode::Advanced => m.advanced_steps.extend(buf_to_advanced_steps(&buf)),
        }

        store::save_profile(&self.profiles[profile_idx]);
    }

    fn handle_learned(&mut self, key: String, device: String) {
        let Some(target) = self.learn_target.take() else { return };
        match target {
            LearnTarget::NewModifier => {
                let mut m = ModifierKey::new(&key, &key);
                m.source_device = if device.is_empty() { None } else { Some(device) };
                if let Some(p) = self.profile_mut() {
                    p.modifiers.push(m);
                }
                self.adding_modifier = false;
                self.save_current();
            }
            LearnTarget::NewMacroStep => {
                if let (Some(profile_idx), Some(macro_idx)) = (self.selected, self.selected_macro) {
                    let hold:  u32 = self.new_step_hold.trim().parse().unwrap_or(40);
                    let delay: u32 = self.new_step_delay.trim().parse().unwrap_or(60);
                    let step = crate::config::SimpleStep { action: key, hold_ms: hold, delay_after_ms: delay };
                    self.profiles[profile_idx].macros[macro_idx].simple_steps.push(step);
                    self.adding_step = false;
                    self.new_step_action.clear();
                    self.new_step_hold  = "40".into();
                    self.new_step_delay = "60".into();
                    self.save_current();
                }
            }
            LearnTarget::AssignmentSource(idx) => {
                let parts: Vec<&str> = key.split('+').collect();
                let actual_key = parts.last().copied().unwrap_or(&key).to_string();
                let held_names: &[&str] = if parts.len() > 1 { &parts[..parts.len()-1] } else { &[] };
                let mod_uuids: Vec<Uuid> = self.profile()
                    .map(|p| p.modifiers.iter()
                        .filter(|m| held_names.contains(&m.key.as_str()))
                        .map(|m| m.id)
                        .collect())
                    .unwrap_or_default();
                let trigger_mode = self.profile()
                    .and_then(|p| p.assignments.get(idx))
                    .map(|a| a.trigger_mode.clone())
                    .unwrap_or_default();
                if let Some(msg) = self.assignment_guard(&actual_key, &mod_uuids, Some(&device), &trigger_mode, idx) {
                    self.warn_popup = Some(msg);
                } else {
                    if let Some(a) = self.profile_mut().and_then(|p| p.assignments.get_mut(idx)) {
                        a.source_key    = actual_key;
                        a.source_device = Some(device);
                        a.modifiers     = mod_uuids;
                    }
                    self.save_current();
                }
            }
            LearnTarget::NewBlockedKey => {
                if let Some(p) = self.profile_mut() {
                    if !p.blocked_keys.contains(&key) {
                        p.blocked_keys.push(key);
                    }
                }
                self.save_current();
            }
            LearnTarget::AssignmentRemap(idx) => {
                if let Some(a) = self.profile_mut().and_then(|p| p.assignments.get_mut(idx)) {
                    a.remap_key = Some(key);
                    a.macro_id  = None;
                }
                self.save_current();
            }
        }
    }

    // ── Assignment guards ─────────────────────────────────────────────────────

    fn is_blocked(&self, key: &str) -> bool {
        self.profile().map_or(false, |p| p.blocked_keys.iter().any(|k| k == key))
    }

    /// Returns true if `source_key` + `modifiers` + `source_device` + `trigger_mode` already
    /// exists in the current profile, skipping `skip_idx` (pass `usize::MAX` when adding).
    fn is_duplicate_assignment(&self, source_key: &str, modifiers: &[Uuid], source_device: Option<&str>, trigger_mode: &TriggerMode, skip_idx: usize) -> bool {
        let Some(p) = self.profile() else { return false };
        let mut sorted_mods = modifiers.to_vec();
        sorted_mods.sort();
        p.assignments.iter().enumerate().any(|(i, a)| {
            i != skip_idx
                && a.source_key == source_key
                && {
                    let mut a_mods = a.modifiers.clone();
                    a_mods.sort();
                    a_mods == sorted_mods
                }
                && a.source_device.as_deref() == source_device
                && &a.trigger_mode == trigger_mode
        })
    }

    /// Check blocked and duplicate guards for a candidate source key.
    /// Returns Some(message) if the key should be rejected, None if it's fine.
    fn assignment_guard(&self, key: &str, modifiers: &[Uuid], source_device: Option<&str>, trigger_mode: &TriggerMode, skip_idx: usize) -> Option<String> {
        if self.is_blocked(key) {
            return Some(format!("{key} is in the blocked list and cannot be assigned."));
        }
        if self.is_duplicate_assignment(key, modifiers, source_device, trigger_mode, skip_idx) {
            return Some(format!("{key} is already assigned with the same modifiers and trigger mode."));
        }
        None
    }

    fn show_warn_popup(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.warn_popup.clone() else { return };
        let mut close = false;
        egui::Window::new("Cannot Assign")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(&msg);
                ui.add_space(6.0);
                if ui.button("OK").clicked() { close = true; }
            });
        if close { self.warn_popup = None; }
    }

    // ── Top bar ───────────────────────────────────────────────────────────────

    fn show_top_bar(&mut self, ctx: &egui::Context) {
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

    // ── Dialogs ───────────────────────────────────────────────────────────────

    fn show_new_profile_dialog(&mut self, ctx: &egui::Context) {
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

    fn show_delete_profile_dialog(&mut self, ctx: &egui::Context) {
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

    fn show_game_link_dialog(&mut self, ctx: &egui::Context) {
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

    fn show_nav(&mut self, ui: &mut egui::Ui) {
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

    // ── Modifiers section ─────────────────────────────────────────────────────

    fn show_modifiers(&mut self, ui: &mut egui::Ui) {
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

    fn show_add_modifier(&mut self, ui: &mut egui::Ui) {
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

    // ── Macros section ────────────────────────────────────────────────────────

    fn show_macros(&mut self, ui: &mut egui::Ui) {
        // Snapshot macro list for rendering
        let macro_list: Vec<(Uuid, String, FireMode, StepMode)> = self.profile()
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
                                                    use crate::config::AdvancedEvent::*;
                                                    egui::ComboBox::from_id_source(format!("adv_ev_{i}"))
                                                        .selected_text(format!("{:?}", m.advanced_steps[i].event))
                                                        .width(ui.available_width())
                                                        .show_ui(ui, |ui| {
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, KeyDown,   "KeyDown").clicked()   { changed = true; }
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, KeyUp,     "KeyUp").clicked()     { changed = true; }
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, MouseDown, "MouseDown").clicked() { changed = true; }
                                                            if ui.selectable_value(&mut m.advanced_steps[i].event, MouseUp,   "MouseUp").clicked()   { changed = true; }
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

    fn show_add_step(&mut self, ui: &mut egui::Ui, _profile_idx: usize, _macro_idx: usize, _changed: &mut bool) {
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

    fn show_loop_events(&mut self, ui: &mut egui::Ui, profile_idx: usize, macro_idx: usize, changed: &mut bool) {
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

    // ── Auto detect section ───────────────────────────────────────────────────

    fn show_auto_detect(&mut self, ui: &mut egui::Ui) {
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

        let mut delete_target:   Option<usize>        = None;
        let mut delete_fragment: Option<(usize, usize)> = None;
        let mut confirm_fragment: Option<usize>        = None;
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

    // ── Blocked keys section ──────────────────────────────────────────────────

    fn show_blocked(&mut self, ui: &mut egui::Ui) {
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

    // ── Assignments section ───────────────────────────────────────────────────

    fn show_assignments(&mut self, ui: &mut egui::Ui) {
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

    fn show_add_assignment(&mut self, ui: &mut egui::Ui, _profile_idx: usize, _needs_save: &mut bool) {
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

    // ── Engine log section ────────────────────────────────────────────────────

    fn show_log(&mut self, ui: &mut egui::Ui) {
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

// ── Trigger mode label ────────────────────────────────────────────────────────

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

// ── Debug launcher ────────────────────────────────────────────────────────────

fn spawn_debug(mode: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .args(["debug", "--mode", mode])
            .spawn();
    }
}

// ── Key learning helper ───────────────────────────────────────────────────────

/// Spawns one reader thread per evdev device.
///
/// When `profile_mod_keys` is non-empty those evdev key names are treated as
/// modifiers: their state is tracked globally across all device threads (so the
/// modifier and key may come from different physical devices), and when a
/// non-modifier key is pressed with modifiers held the result is a
/// `+`-delimited compound string such as `"KEY_MACRO1+KEY_A"`.  The returned
/// device name is always the device that produced the non-modifier key.
///
/// When `profile_mod_keys` is empty every key is treated as a plain key and
/// the first press is returned immediately.
///
/// A 150 ms timestamp gate on non-modifier presses prevents the click that
/// opened the learn dialog from being captured.  Modifier tracking has no gate
/// so the user can hold a modifier immediately after clicking.
fn start_key_learn(profile_mod_keys: Vec<String>) -> (mpsc::Receiver<(String, String)>, Vec<DeviceReader>) {
    use std::sync::{Arc, Mutex};

    let (tx, rx) = mpsc::channel::<(String, String)>();
    let threshold = std::time::SystemTime::now()
        + std::time::Duration::from_millis(150);
    let mod_keys  = Arc::new(profile_mod_keys);
    let held_mods: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let handles: Vec<DeviceReader> = evdev::enumerate()
        .map(|(_, mut device)| {
            let device_name = device.name().unwrap_or("Unknown").to_string();
            let tx        = tx.clone();
            let mod_keys  = mod_keys.clone();
            let held_mods = held_mods.clone();
            let (handle, cancel) = DeviceReader::new_with_cancel();

            std::thread::spawn(move || {
                loop {
                    if !cancel.load(Ordering::Relaxed) { break; }
                    let Ok(events) = device.fetch_events() else { break };
                    for event in events {
                        if !cancel.load(Ordering::Relaxed) { return; }
                        if let evdev::InputEventKind::Key(key) = event.kind() {
                            let name = crate::engine::key_name(key);
                            if mod_keys.contains(&name) {
                                // Track held state across all devices; no timestamp gate.
                                let mut mods = held_mods.lock().unwrap();
                                if event.value() == 1 {
                                    if !mods.contains(&name) { mods.push(name); }
                                } else if event.value() == 0 {
                                    mods.retain(|m| m != &name);
                                }
                            } else if event.value() == 1 {
                                if event.timestamp() < threshold { continue; }
                                let mods = held_mods.lock().unwrap().clone();
                                let result = if mods.is_empty() {
                                    name
                                } else {
                                    let mut parts = mods;
                                    parts.push(name);
                                    parts.join("+")
                                };
                                let _ = tx.send((result, device_name.clone()));
                                return;
                            }
                        }
                    }
                }
            });

            handle
        })
        .collect();

    (rx, handles)
}

// ── Macro recording helper ────────────────────────────────────────────────────

/// Spawns one reader thread per evdev device.
/// Sends every key-down and key-up event (with ms offset) to the channel.
/// Sending `RecordMsg::Stop` when Escape is pressed signals the UI to finalise.
fn start_recording() -> (mpsc::Receiver<RecordMsg>, Vec<DeviceReader>) {
    let (tx, rx) = mpsc::channel::<RecordMsg>();
    let start    = std::time::Instant::now();

    let handles: Vec<DeviceReader> = evdev::enumerate()
        .map(|(_, mut device)| {
            let tx = tx.clone();
            let (handle, cancel) = DeviceReader::new_with_cancel();

            std::thread::spawn(move || {
                loop {
                    if !cancel.load(Ordering::Relaxed) { break; }
                    let Ok(events) = device.fetch_events() else { break };
                    for event in events {
                        if !cancel.load(Ordering::Relaxed) { return; }
                        if let evdev::InputEventKind::Key(key) = event.kind() {
                            let value = event.value();
                            if value != 0 && value != 1 { continue; } // skip auto-repeat
                            let name       = format!("{key:?}");
                            let elapsed_ms = start.elapsed().as_millis() as u32;
                            if name == "KEY_ESC" && value == 1 {
                                let _ = tx.send(RecordMsg::Stop);
                                return;
                            }
                            let _ = tx.send(RecordMsg::Key {
                                name,
                                pressed: value == 1,
                                elapsed_ms,
                            });
                        }
                    }
                }
            });

            handle
        })
        .collect();

    (rx, handles)
}

fn buf_to_simple_steps(buf: &[RecordedKey]) -> Vec<SimpleStep> {
    let presses: Vec<&RecordedKey> = buf.iter().filter(|k| k.pressed).collect();
    presses.iter().enumerate().map(|(i, k)| {
        let delay = presses.get(i + 1)
            .map(|next| next.elapsed_ms.saturating_sub(k.elapsed_ms))
            .unwrap_or(0);
        SimpleStep { action: k.name.clone(), hold_ms: 0, delay_after_ms: delay }
    }).collect()
}

fn buf_to_advanced_steps(buf: &[RecordedKey]) -> Vec<crate::config::AdvancedStep> {
    use crate::config::{AdvancedEvent, AdvancedStep};
    buf.iter().map(|k| {
        let event = match (k.pressed, k.name.starts_with("BTN_")) {
            (true,  true)  => AdvancedEvent::MouseDown,
            (false, true)  => AdvancedEvent::MouseUp,
            (true,  false) => AdvancedEvent::KeyDown,
            (false, false) => AdvancedEvent::KeyUp,
        };
        AdvancedStep { event, key: k.name.clone(), time_ms: k.elapsed_ms }
    }).collect()
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for ProfileApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll macro recording every frame
        self.poll_recording();
        if self.recording {
            ctx.request_repaint_after(std::time::Duration::from_millis(crate::constants::UI_REPAINT_FAST_MS));
        }

        // Poll key learning every frame
        if let Some((key, device)) = self.poll_learn() {
            // Special case: new assignment source key
            if matches!(self.learn_target, Some(LearnTarget::AssignmentSource(usize::MAX))) {
                self.learn_target  = None;
                self.learn_handles = Vec::new(); // stop remaining reader threads
                let parts: Vec<&str> = key.split('+').collect();
                let actual_key = parts.last().copied().unwrap_or(&key).to_string();
                let held_names: &[&str] = if parts.len() > 1 { &parts[..parts.len()-1] } else { &[] };
                let mod_uuids: Vec<Uuid> = self.profile()
                    .map(|p| p.modifiers.iter()
                        .filter(|m| held_names.contains(&m.key.as_str()))
                        .map(|m| m.id)
                        .collect())
                    .unwrap_or_default();
                if let Some(msg) = self.assignment_guard(&actual_key, &mod_uuids, Some(&device), &TriggerMode::Any, usize::MAX) {
                    self.warn_popup = Some(msg);
                } else if let Some(p) = self.profile_mut() {
                    let mut a = KeyAssignment::new(actual_key);
                    a.source_device = Some(device);
                    a.modifiers     = mod_uuids;
                    p.assignments.push(a);
                    store::save_profile(p);
                }
                self.adding_assignment = false;
            } else {
                self.handle_learned(key, device);
            }
        }
        if self.learn_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(crate::constants::UI_REPAINT_FAST_MS));
        }

        // Auto-detect scanner lifecycle
        if self.settings.auto_switch_profiles {
            if self.scanner_dirty {
                self.engine.start_auto_detect(self.profiles.clone());
                self.scanner_dirty = false;
            }
            match self.engine.poll_detect() {
                Some(DetectEvent::Found(uuid)) => {
                    if self.engine.active_profile_uuid() != Some(uuid) {
                        if let Err(e) = self.engine.load(uuid) {
                            self.engine_err = e;
                        } else {
                            self.auto_detected_uuid = Some(uuid);
                            if let Some(i) = self.profiles.iter().position(|p| p.id == uuid) {
                                self.selected = Some(i);
                            }
                        }
                    }
                }
                Some(DetectEvent::Lost) => {
                    // Only stop if the currently-running session was auto-detected,
                    // not manually loaded by the user.
                    if self.auto_detected_uuid.is_some()
                        && self.engine.active_profile_uuid() == self.auto_detected_uuid
                    {
                        self.engine.stop();
                        self.auto_detected_uuid = None;
                    }
                }
                None => {}
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(crate::constants::UI_REPAINT_AUTODETECT_SECS));
        }

        // Drain engine log when enabled
        if self.log_enabled {
            let new = crate::engine::drain_engine_log();
            if !new.is_empty() {
                self.log_entries.extend(new);
                if self.log_entries.len() > crate::constants::ENGINE_LOG_CAPACITY {
                    let excess = self.log_entries.len() - crate::constants::ENGINE_LOG_CAPACITY;
                    self.log_entries.drain(0..excess);
                }
                ctx.request_repaint();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(crate::constants::UI_REPAINT_NORMAL_MS));
        }

        self.show_new_profile_dialog(ctx);
        self.show_delete_profile_dialog(ctx);
        self.show_game_link_dialog(ctx);
        self.show_warn_popup(ctx);

        self.show_top_bar(ctx);

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

        egui::SidePanel::left("nav_panel").exact_width(110.0).show(ctx, |ui| {
            ui.add_space(4.0);
            self.show_nav(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.section == Section::Log {
                self.show_log(ui);
                return;
            }
            if self.selected.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("Create or select a profile to get started.")
                            .color(GRAY)
                            .size(16.0),
                    );
                });
                return;
            }
            match self.section.clone() {
                Section::Modifiers   => self.show_modifiers(ui),
                Section::Macros      => self.show_macros(ui),
                Section::Assignments => self.show_assignments(ui),
                Section::Blocked     => self.show_blocked(ui),
                Section::AutoDetect  => self.show_auto_detect(ui),
                Section::Log         => unreachable!(),
            }
        });
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("gameremap — Profile Editor")
            .with_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "gameremap-profile",
        options,
        Box::new(|cc| Ok(Box::new(ProfileApp::build(cc)))),
    )
    .expect("Failed to start profile editor");
}
