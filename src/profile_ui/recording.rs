use std::sync::atomic::Ordering;
use std::sync::mpsc;
use uuid::Uuid;

use crate::config::{store, AdvancedEvent, AdvancedStep, ModifierKey, SimpleStep, StepMode};
use crate::devices::DeviceReader;
use super::{LearnTarget, ProfileApp, RecordMsg, RecordedKey};

impl ProfileApp {
    // ── Key learning ──────────────────────────────────────────────────────────

    pub(super) fn poll_learn(&mut self) -> Option<(String, String)> {
        let result = self.learn_rx.as_ref()?.try_recv().ok()?;
        self.learn_rx      = None;
        self.learn_handles = Vec::new();
        Some(result)
    }

    pub(super) fn start_learn(&mut self, target: LearnTarget) {
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

    pub(super) fn cancel_learn(&mut self) {
        self.learn_target  = None;
        self.learn_rx      = None;
        self.learn_handles = Vec::new(); // drops handles → threads stop
    }

    // ── Macro recording ───────────────────────────────────────────────────────

    pub(super) fn start_record_session(&mut self) {
        let (rx, handles) = start_recording();
        self.recording      = true;
        self.record_rx      = Some(rx);
        self.record_handles = handles;
        self.record_buf     = Vec::new();
    }

    /// Drains the record channel into `record_buf`.  On Stop, converts the
    /// buffer to steps and appends them to the currently selected macro.
    pub(super) fn poll_recording(&mut self) {
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

        let Some(profile_idx) = self.selected      else { self.record_buf.clear(); return };
        let Some(macro_idx)   = self.selected_macro else { self.record_buf.clear(); return };

        let buf = std::mem::take(&mut self.record_buf);
        let m   = &mut self.profiles[profile_idx].macros[macro_idx];

        match m.mode {
            StepMode::Simple   => m.simple_steps.extend(buf_to_simple_steps(&buf)),
            StepMode::Advanced => m.advanced_steps.extend(buf_to_advanced_steps(&buf)),
        }

        store::save_profile(&self.profiles[profile_idx]);
    }

    pub(super) fn handle_learned(&mut self, key: String, device: String) {
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
                    let step = SimpleStep { action: key, hold_ms: hold, delay_after_ms: delay };
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

fn buf_to_advanced_steps(buf: &[RecordedKey]) -> Vec<AdvancedStep> {
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
