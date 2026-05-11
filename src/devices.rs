use evdev::{InputEventKind, MiscType};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;

use crate::event_types::{DisplayEvent, EventHighlight};

// ── Cancellable reader handle ─────────────────────────────────────────────────

/// Handle to a running evdev reader thread.
/// Dropping signals the thread to exit at its next event loop iteration.
/// Because `fetch_events()` is blocking, the thread may linger until the
/// device produces its next event — acceptable for gaming peripherals.
pub struct DeviceReader {
    cancel: Arc<AtomicBool>,
}

impl DeviceReader {
    pub(crate) fn new_with_cancel() -> (Self, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(true));
        (Self { cancel: cancel.clone() }, cancel)
    }
}

impl Drop for DeviceReader {
    fn drop(&mut self) {
        self.cancel.store(false, Ordering::Relaxed);
    }
}

pub enum AccessStatus {
    Ok,
    /// Event nodes exist but can't be opened — user not in 'input' group
    Denied,
    /// No event nodes found at all (unusual)
    NoDevices,
}

/// Write a temp setup script and run it as root via pkexec.
/// Blocks until complete; intended to be called from a background thread.
pub fn run_setup_as_root() -> Result<(), String> {
    let username = std::env::var("USER")
        .unwrap_or_else(|_| "$(id -un)".to_string());

    if !username.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Err(format!("Unexpected characters in username {username:?}; aborting setup."));
    }

    // Script runs as root so no sudo prefix needed inside it.
    // Use a unique heredoc delimiter to avoid any nesting issues.
    let script = format!(
        r#"#!/bin/sh
set -e
usermod -aG input {username}
cat > /etc/udev/rules.d/99-gameremap.rules << 'GAMEREMAP_RULES'
KERNEL=="event*", SUBSYSTEM=="input", GROUP="input", MODE="0660"
KERNEL=="uinput", GROUP="input", MODE="0660"
KERNEL=="hidraw*", GROUP="input", MODE="0660"
GAMEREMAP_RULES
udevadm control --reload
udevadm trigger --subsystem-match=input
"#
    );

    let script_path = format!("/tmp/gameremap-setup-{}.sh", std::process::id());
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&script_path)
            .and_then(|mut f| f.write_all(script.as_bytes()))
            .map_err(|e| format!("Could not write setup script: {e}"))?;
    }

    let status = std::process::Command::new("pkexec")
        .args(["sh", &script_path])
        .status()
        .map_err(|e| format!("Could not launch pkexec: {e}. Is polkit installed?"))?;

    let _ = std::fs::remove_file(&script_path);

    if status.success() {
        Ok(())
    } else {
        match status.code() {
            Some(126) => Err("Authentication cancelled.".to_string()),
            Some(127) => Err("pkexec: command not found.".to_string()),
            Some(n)   => Err(format!("Setup script exited with code {n}.")),
            None      => Err("Setup process terminated by signal.".to_string()),
        }
    }
}

/// Remove the gameremap udev rules file and reload udev.
/// Blocks; intended to be called from a background thread.
pub fn run_teardown_as_root() -> Result<(), String> {
    let script = r#"#!/bin/sh
rm -f /etc/udev/rules.d/99-gameremap.rules
udevadm control --reload
udevadm trigger --subsystem-match=input
"#;

    let script_path = format!("/tmp/gameremap-teardown-{}.sh", std::process::id());
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&script_path)
            .and_then(|mut f| f.write_all(script.as_bytes()))
            .map_err(|e| format!("Could not write teardown script: {e}"))?;
    }

    let status = std::process::Command::new("pkexec")
        .args(["sh", &script_path])
        .status()
        .map_err(|e| format!("Could not launch pkexec: {e}. Is polkit installed?"))?;

    let _ = std::fs::remove_file(&script_path);

    if status.success() {
        Ok(())
    } else {
        match status.code() {
            Some(126) => Err("Authentication cancelled.".to_string()),
            Some(127) => Err("pkexec: command not found.".to_string()),
            Some(n)   => Err(format!("Teardown script exited with code {n}.")),
            None      => Err("Teardown process terminated by signal.".to_string()),
        }
    }
}

/// Quick pre-flight check: can we open any /dev/input/event* node?
pub fn check_access() -> AccessStatus {
    let Ok(dir) = std::fs::read_dir("/dev/input") else {
        return AccessStatus::NoDevices;
    };

    let mut found_event_node = false;
    for entry in dir.filter_map(|e| e.ok()) {
        if entry.file_name().to_string_lossy().starts_with("event") {
            found_event_node = true;
            if std::fs::File::open(entry.path()).is_ok() {
                return AccessStatus::Ok;
            }
        }
    }

    if found_event_node { AccessStatus::Denied } else { AccessStatus::NoDevices }
}

pub struct DeviceInfo {
    pub path: String,
    pub name: String,
}

/// Spawn one reader thread per accessible input device.
/// Returns a list of devices that were successfully opened.
/// Threads run until the sender is closed (debug window exit).
pub fn start_readers(sender: mpsc::Sender<DisplayEvent>, start: Instant) -> Vec<DeviceInfo> {
    let found: Vec<_> = evdev::enumerate().collect();

    if found.is_empty() {
        eprintln!("No readable input devices found — is this user in the 'input' group?");
    }

    found.into_iter()
        .map(|(path, device)| {
            let name = device.name().unwrap_or("Unknown").to_string();
            let path_str = path.display().to_string();
            // Discard the handle — debug readers run until the process exits.
            let _ = spawn_device_reader(path, device, sender.clone(), start);
            DeviceInfo { path: path_str, name }
        })
        .collect()
}

/// Spawn reader threads only for devices whose key capabilities include at least
/// one name from `key_names` (e.g. `&["KEY_LEFTCTRL", "KEY_MACRO1"]`).
/// Returns handles — dropping a handle signals its thread to stop.
/// Use this when a profile is active: only read devices that have mapped keys.
pub fn start_filtered_readers(
    key_names: &[&str],
    sender: mpsc::Sender<DisplayEvent>,
    start: Instant,
) -> Vec<DeviceReader> {
    evdev::enumerate()
        .filter(|(_, device)| device_supports_any(device, key_names))
        .map(|(path, device)| spawn_device_reader(path, device, sender.clone(), start))
        .collect()
}

/// Returns true if the device reports support for any key whose debug name
/// matches one of the provided strings (e.g. "KEY_LEFTALT").
pub fn device_supports_any(device: &evdev::Device, key_names: &[&str]) -> bool {
    let Some(supported) = device.supported_keys() else { return false };
    supported.iter().any(|key| {
        let name = format!("{key:?}");
        key_names.iter().any(|&n| n == name)
    })
}

/// Core thread spawn shared by all reader variants.
fn spawn_device_reader(
    path: std::path::PathBuf,
    mut device: evdev::Device,
    sender: mpsc::Sender<DisplayEvent>,
    start: Instant,
) -> DeviceReader {
    let name     = device.name().unwrap_or("Unknown").to_string();
    let path_str = path.display().to_string();
    let (handle, cancel) = DeviceReader::new_with_cancel();

    std::thread::spawn(move || {
        // MSC_SCAN arrives just before KEY_UNKNOWN; carry it forward one event.
        let mut pending_scan: Option<u32> = None;

        loop {
            if !cancel.load(Ordering::Relaxed) { break; }

            let events = match device.fetch_events() {
                Ok(ev) => ev,
                Err(e) => {
                    eprintln!("Read error on {path_str}: {e}");
                    break;
                }
            };

            for event in events {
                if !cancel.load(Ordering::Relaxed) { return; }

                let (ev_type, code_name, value_str, highlight) =
                    classify(&event, &mut pending_scan);

                if sender.send(DisplayEvent {
                    elapsed: start.elapsed(),
                    device_name: name.clone(),
                    event_type: ev_type,
                    code_name,
                    value_str,
                    highlight,
                }).is_err() {
                    return; // receiver dropped, exit cleanly
                }
            }
        }
    });

    handle
}

fn classify(
    event: &evdev::InputEvent,
    pending_scan: &mut Option<u32>,
) -> (&'static str, String, String, EventHighlight) {
    let value = event.value();

    match event.kind() {
        InputEventKind::Synchronization(_) => {
            *pending_scan = None;
            ("EV_SYN", format!("{:?}", event.kind()), String::new(), EventHighlight::Sync)
        }

        InputEventKind::Misc(MiscType::MSC_SCAN) => {
            *pending_scan = Some(value as u32);
            ("EV_MSC", format!("MSC_SCAN"), format!("0x{value:x}"), EventHighlight::Normal)
        }

        InputEventKind::Key(key) => {
            let scan = pending_scan.take();
            let raw_name = format!("{key:?}");
            let code = event.code();

            let (code_name, highlight) = if raw_name.starts_with("Key(") {
                // Kernel has no name for this code — show raw scan if we have it
                let detail = scan.map_or(String::new(), |s| format!(" [scan=0x{s:x}]"));
                (format!("KEY_UNKNOWN{detail}"), EventHighlight::Unknown)
            } else if is_gaming_key(code) {
                (raw_name, EventHighlight::Gaming)
            } else {
                (raw_name, EventHighlight::Normal)
            };

            let value_str = match value {
                1 => "press".to_string(),
                0 => "release".to_string(),
                2 => "repeat".to_string(),
                _ => format!("{value}"),
            };

            ("EV_KEY", code_name, value_str, highlight)
        }

        InputEventKind::RelAxis(_) => {
            *pending_scan = None;
            let name = format!("{:?}", event.kind());
            let val = if value >= 0 { format!("+{value}") } else { format!("{value}") };
            ("EV_REL", name, val, EventHighlight::Normal)
        }

        InputEventKind::AbsAxis(_) => {
            *pending_scan = None;
            ("EV_ABS", format!("{:?}", event.kind()), format!("{value}"), EventHighlight::Normal)
        }

        _ => {
            *pending_scan = None;
            ("EV_?", format!("{:?}", event.kind()), format!("{value}"), EventHighlight::Normal)
        }
    }
}

fn is_gaming_key(code: u16) -> bool {
    use crate::constants::*;
    matches!(code,
        KEY_MACRO_BASE..=KEY_MACRO_MAX
        | BTN_TRIGGER_HAPPY_BASE..=BTN_TRIGGER_HAPPY_MAX
        | 183..=194   // KEY_F13–KEY_F24
        | 149..=152   // KEY_PROG1–KEY_PROG4
    )
}
