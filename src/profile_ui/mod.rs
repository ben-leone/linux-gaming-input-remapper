use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use uuid::Uuid;

use crate::config::{
    store, AppSettings, GameLink, KeyAssignment, Profile, TriggerMode,
};
use crate::engine::DetectEvent;
use crate::devices::DeviceReader;

mod assignments;
mod auto_detect;
mod log_panel;
mod macros;
mod modifiers;
mod nav;
mod recording;
mod validation;

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
        self.show_profiles_panel(ctx);

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
