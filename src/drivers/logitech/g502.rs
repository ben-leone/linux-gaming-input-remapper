use evdev::{uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key};
use std::sync::mpsc;
use std::time::Instant;

use crate::drivers::SupplementalDriver;
use crate::event_types::{DisplayEvent, EventHighlight};
use crate::hidraw::{self, HidRawDevice};

pub const BUTTON_COUNT: u32 = 2;

pub fn keycode(button_index: u32) -> u16 {
    crate::constants::KEY_MACRO_BASE + button_index as u16
}

/// HID keycode → (button_index, display_name)
/// Report layout (9 bytes, interface 1):
///   [0x01, modifier, 0x00, keycode, 0x00, 0x00, 0x00, 0x00, 0x00]
/// Both buttons send modifier=0x01 (Left Ctrl); they differ only in byte 3.
const BUTTON_MAP: &[(u8, u32, &str)] = &[
    (0x06, 0, "rear"),     // rear button (next to left click)    → KEY_MACRO1
    (0x19, 1, "forward"),  // forward button (next to left click) → KEY_MACRO2
];

/// Decode a G502 SE HERO interface-1 keyboard report into a button bitmask.
/// Returns None if not an interface-1 report, too short, or wrong report ID.
/// Returns Some(mask) where bit N is set if the button at BUTTON_MAP index N is pressed.
pub fn decode(interface: u8, data: &[u8]) -> Option<u32> {
    if interface != 1 || data.len() < 4 || data[0] != 0x01 {
        return None;
    }
    let mut mask: u32 = 0;
    for &byte in &data[3..data.len().min(9)] {
        if byte == 0 { continue; }
        if let Some(&(_, idx, _)) = BUTTON_MAP.iter().find(|&&(code, _, _)| code == byte) {
            mask |= 1 << idx;
        }
    }
    Some(mask)
}

pub struct G502Driver;

impl SupplementalDriver for G502Driver {
    fn vendor_id(&self)  -> u32 { 0x046D }
    fn product_id(&self) -> u32 { 0xC08B }

    fn start(
        self: Box<Self>,
        devices: Vec<HidRawDevice>,
        sender: mpsc::Sender<DisplayEvent>,
        start: Instant,
    ) {
        std::thread::spawn(move || {
            if let Err(e) = run(devices, sender, start) {
                eprintln!("G502Driver error: {e}");
            }
        });
    }
}

fn build_uinput() -> Option<evdev::uinput::VirtualDevice> {
    let mut key_set = AttributeSet::<Key>::new();
    for i in 0..BUTTON_COUNT {
        key_set.insert(Key::new(keycode(i)));
    }
    match VirtualDeviceBuilder::new()
        .and_then(|b| b.name("gameremap: G502 extra buttons").with_keys(&key_set))
        .and_then(|b| b.build())
    {
        Ok(d)  => Some(d),
        Err(e) => {
            eprintln!(
                "Warning: could not create uinput device ({e}). \
                 G502 extra buttons will appear in the UI but won't be injected."
            );
            None
        }
    }
}

fn evdev_nodes_for_hidraw(hidraw_path: &str) -> Vec<String> {
    let name = hidraw_path.rsplit('/').next().unwrap_or("");
    let input_dir = format!("/sys/class/hidraw/{}/device/input", name);
    let mut result = Vec::new();
    let Ok(dir) = std::fs::read_dir(&input_dir) else { return result };
    for entry in dir.filter_map(|e| e.ok()) {
        let Ok(inner) = std::fs::read_dir(entry.path()) else { continue };
        for ie in inner.filter_map(|e| e.ok()) {
            let fname = ie.file_name().to_string_lossy().to_string();
            if fname.starts_with("event") {
                result.push(format!("/dev/input/{}", fname));
            }
        }
    }
    result
}

fn run(
    devices: Vec<HidRawDevice>,
    sender: mpsc::Sender<DisplayEvent>,
    start: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut vdev = build_uinput();

    // Grab interface-1 evdev nodes so KEY_LEFTCTRL/KEY_C/KEY_V don't bleed through.
    let _grabbed: Vec<evdev::Device> = devices.iter()
        .filter(|d| d.interface == 1)
        .flat_map(|d| evdev_nodes_for_hidraw(&d.hidraw_path))
        .filter_map(|path| {
            match evdev::Device::open(&path) {
                Ok(mut d) => {
                    if let Err(e) = d.grab() {
                        eprintln!("Warning: could not grab {path}: {e}");
                    }
                    Some(d)
                }
                Err(e) => {
                    eprintln!("Warning: could not open {path}: {e}");
                    None
                }
            }
        })
        .collect();

    let (hid_tx, hid_rx) = mpsc::channel();
    hidraw::start_readers(devices, hid_tx, start);

    let mut prev_mask: u32 = 0;

    for report in hid_rx {
        let Some(mask) = decode(report.device.interface, &report.data) else {
            continue;
        };
        if mask == prev_mask { continue; }

        let changed = mask ^ prev_mask;
        let mut uinput_events: Vec<InputEvent> = Vec::new();

        for &(_, idx, _) in BUTTON_MAP {
            if changed & (1 << idx) != 0 {
                let pressed = mask & (1 << idx) != 0;

                let _ = sender.send(DisplayEvent {
                    elapsed: report.elapsed,
                    device_name: report.device.name.clone(),
                    event_type: "G_KEY",
                    code_name: format!("KEY_MACRO{}", idx + 1),
                    value_str: if pressed { "press" } else { "release" }.to_string(),
                    highlight: EventHighlight::Gaming,
                });

                if vdev.is_some() {
                    uinput_events.push(InputEvent::new(
                        EventType::KEY,
                        keycode(idx),
                        if pressed { 1 } else { 0 },
                    ));
                }
            }
        }

        if let Some(ref mut d) = vdev {
            if !uinput_events.is_empty() {
                uinput_events.push(InputEvent::new(EventType::SYNCHRONIZATION, 0, 0));
                let _ = d.emit(&uinput_events);
            }
        }

        prev_mask = mask;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report(modifier: u8, keycodes: &[u8]) -> Vec<u8> {
        let mut data = [0u8; 9];
        data[0] = 0x01;
        data[1] = modifier;
        for (i, &k) in keycodes.iter().enumerate().take(6) {
            data[3 + i] = k;
        }
        data.to_vec()
    }

    #[test]
    fn wrong_interface_returns_none() {
        let data = make_report(0x01, &[0x06]);
        assert_eq!(decode(0, &data), None);
        assert_eq!(decode(2, &data), None);
    }

    #[test]
    fn wrong_report_id_returns_none() {
        let mut data = make_report(0x01, &[0x06]);
        data[0] = 0x02;
        assert_eq!(decode(1, &data), None);
    }

    #[test]
    fn too_short_returns_none() {
        assert_eq!(decode(1, &[0x01, 0x01, 0x00]), None);
    }

    #[test]
    fn rear_button_press() {
        let data = make_report(0x01, &[0x06]);
        assert_eq!(decode(1, &data), Some(1 << 0));
    }

    #[test]
    fn forward_button_press() {
        let data = make_report(0x01, &[0x19]);
        assert_eq!(decode(1, &data), Some(1 << 1));
    }

    #[test]
    fn release_report() {
        let data = make_report(0x00, &[]);
        assert_eq!(decode(1, &data), Some(0));
    }

    #[test]
    fn unknown_keycode_ignored() {
        let data = make_report(0x01, &[0xFF]);
        assert_eq!(decode(1, &data), Some(0));
    }

    #[test]
    fn keycode_range() {
        assert_eq!(keycode(0), crate::constants::KEY_MACRO_BASE);
        assert_eq!(keycode(1), crate::constants::KEY_MACRO_BASE + 1);
    }
}
