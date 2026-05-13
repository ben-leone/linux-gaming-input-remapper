use eframe::egui;
use uuid::Uuid;

use crate::config::TriggerMode;
use super::ProfileApp;

impl ProfileApp {
    // ── Assignment guards ─────────────────────────────────────────────────────

    pub(super) fn is_blocked(&self, key: &str) -> bool {
        self.profile().map_or(false, |p| p.blocked_keys.iter().any(|k| k == key))
    }

    /// Returns true if `source_key` + `modifiers` + `source_device` + `trigger_mode` already
    /// exists in the current profile, skipping `skip_idx` (pass `usize::MAX` when adding).
    pub(super) fn is_duplicate_assignment(&self, source_key: &str, modifiers: &[Uuid], source_device: Option<&str>, trigger_mode: &TriggerMode, skip_idx: usize) -> bool {
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
    pub(super) fn assignment_guard(&self, key: &str, modifiers: &[Uuid], source_device: Option<&str>, trigger_mode: &TriggerMode, skip_idx: usize) -> Option<String> {
        if self.is_blocked(key) {
            return Some(format!("{key} is in the blocked list and cannot be assigned."));
        }
        if self.is_duplicate_assignment(key, modifiers, source_device, trigger_mode, skip_idx) {
            return Some(format!("{key} is already assigned with the same modifiers and trigger mode."));
        }
        None
    }

    pub(super) fn show_warn_popup(&mut self, ctx: &egui::Context) {
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
}
