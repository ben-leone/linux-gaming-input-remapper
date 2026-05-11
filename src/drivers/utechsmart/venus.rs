use evdev::{uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key};
use std::sync::mpsc;
use std::time::Instant;

use crate::drivers::SupplementalDriver;
use crate::event_types::{DisplayEvent, EventHighlight};
use crate::hidraw::{self, HidRawDevice};

/// Number of programmable side buttons on the Venus.
pub const BUTTON_COUNT: u32 = 12;

/// Linux key codes for side buttons 1–12.
/// Maps button index 0–11 to KEY_MACRO1–KEY_MACRO12.
pub fn keycode(button_index: u32) -> u16 {
    crate::constants::KEY_MACRO_BASE + button_index as u16
}

/// HID keyboard keycodes reported by the Venus for side buttons 1–12.
/// Index in this array == button index (0 = button 1, 11 = button 12).
const HID_KEYCODES: [u8; BUTTON_COUNT as usize] = [
    0x1E, // button 1
    0x1F, // button 2
    0x20, // button 3
    0x21, // button 4
    0x22, // button 5
    0x23, // button 6
    0x24, // button 7
    0x25, // button 8
    0x26, // button 9
    0x27, // button 10
    0x56, // button 11
    0x57, // button 12
];

/// Decode a Venus interface-1 keyboard report into a 12-bit button bitmask.
///
/// Report layout (HID keyboard boot protocol, 8 bytes):
///   Byte 0: modifier bitmask (unused here)
///   Byte 1: reserved
///   Bytes 2–7: up to 6 simultaneous keycodes (0x00 = empty slot)
///
/// Returns `None` if the report is shorter than 8 bytes or originates from
/// an interface other than 1 (caller should pre-filter, but this is a
/// belt-and-suspenders guard for the decode function itself).
///
/// Returns `Some(mask)` where bit N is set if button N+1 is currently held.
pub fn decode(interface: u8, data: &[u8]) -> Option<u32> {
    if interface != 1 || data.len() < 8 {
        return None;
    }

    let mut mask: u32 = 0;
    for &hid_code in &data[2..8] {
        if hid_code == 0x00 {
            continue;
        }
        if let Some(idx) = HID_KEYCODES.iter().position(|&k| k == hid_code) {
            mask |= 1 << idx;
        }
    }
    Some(mask)
}

pub struct VenusDriver;

impl SupplementalDriver for VenusDriver {
    fn vendor_id(&self) -> u32  { 0x04D9 }
    fn product_id(&self) -> u32 { 0xFA58 }

    fn start(
        self: Box<Self>,
        devices: Vec<HidRawDevice>,
        sender: mpsc::Sender<DisplayEvent>,
        start: Instant,
    ) {
        std::thread::spawn(move || {
            if let Err(e) = run(devices, sender, start) {
                eprintln!("VenusDriver error: {e}");
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
        .and_then(|b| b.name("gameremap: UTechSmart Venus side buttons").with_keys(&key_set))
        .and_then(|b| b.build())
    {
        Ok(d)  => Some(d),
        Err(e) => {
            eprintln!(
                "Warning: could not create uinput device ({e}). \
                 Venus side buttons will appear in the UI but won't be injected."
            );
            None
        }
    }
}

/// Walk sysfs to find the /dev/input/eventN node(s) owned by a hidraw device.
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

    // Grab the evdev node(s) for the keyboard interface (interface 1) so that
    // the raw KEY_1…KEY_F11 events don't leak through to games.
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
        // Only process reports from the keyboard interface (interface 1).
        let Some(mask) = decode(report.device.interface, &report.data) else {
            continue;
        };
        if mask == prev_mask {
            continue;
        }

        let changed = mask ^ prev_mask;
        let mut uinput_events: Vec<InputEvent> = Vec::new();

        for i in 0..BUTTON_COUNT {
            if changed & (1 << i) != 0 {
                let pressed = mask & (1 << i) != 0;
                let value_str = if pressed { "press" } else { "release" }.to_string();

                let _ = sender.send(DisplayEvent {
                    elapsed: report.elapsed,
                    device_name: report.device.name.clone(),
                    event_type: "SIDE_BUTTON",
                    code_name: format!("M{}", i + 1),
                    value_str,
                    highlight: EventHighlight::Gaming,
                });

                if vdev.is_some() {
                    uinput_events.push(InputEvent::new(
                        EventType::KEY,
                        keycode(i),
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

    fn make_report(keycodes: &[u8]) -> Vec<u8> {
        let mut data = [0u8; 8];
        for (i, &k) in keycodes.iter().enumerate().take(6) {
            data[2 + i] = k;
        }
        data.to_vec()
    }

    // --- interface guard ---

    #[test]
    fn wrong_interface_returns_none() {
        let data = make_report(&[0x1E]);
        assert_eq!(decode(0, &data), None);
        assert_eq!(decode(2, &data), None);
    }

    #[test]
    fn report_too_short_returns_none() {
        assert_eq!(decode(1, &[0x00; 7]), None);
        assert_eq!(decode(1, &[]), None);
    }

    // --- single button presses ---

    #[test]
    fn button_1_through_10() {
        let expected_hid = [
            0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
        ];
        for (idx, &hid) in expected_hid.iter().enumerate() {
            let data = make_report(&[hid]);
            assert_eq!(
                decode(1, &data),
                Some(1 << idx),
                "button {} (HID 0x{hid:02X}) should set bit {idx}",
                idx + 1
            );
        }
    }

    #[test]
    fn button_11() {
        let data = make_report(&[0x56]);
        assert_eq!(decode(1, &data), Some(1 << 10));
    }

    #[test]
    fn button_12() {
        let data = make_report(&[0x57]);
        assert_eq!(decode(1, &data), Some(1 << 11));
    }

    // --- all-zeros report (all released) ---

    #[test]
    fn all_zeros_is_no_buttons() {
        let data = [0u8; 8].to_vec();
        assert_eq!(decode(1, &data), Some(0));
    }

    // --- simultaneous presses ---

    #[test]
    fn simultaneous_two_buttons() {
        // Button 1 (0x1E) + Button 2 (0x1F)
        let data = make_report(&[0x1E, 0x1F]);
        assert_eq!(decode(1, &data), Some((1 << 0) | (1 << 1)));
    }

    #[test]
    fn simultaneous_non_adjacent() {
        // Button 3 (0x20) + Button 11 (0x56) + Button 12 (0x57)
        let data = make_report(&[0x20, 0x56, 0x57]);
        assert_eq!(decode(1, &data), Some((1 << 2) | (1 << 10) | (1 << 11)));
    }

    #[test]
    fn all_six_slots_with_duplicates_ignored() {
        // Fill all 6 slots: buttons 1–5 + unknown code 0xFF (should be ignored)
        let data = make_report(&[0x1E, 0x1F, 0x20, 0x21, 0x22, 0xFF]);
        assert_eq!(
            decode(1, &data),
            Some((1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4))
        );
    }

    #[test]
    fn unknown_hid_code_does_not_panic_or_set_bits() {
        let data = make_report(&[0xAA, 0xBB]);
        assert_eq!(decode(1, &data), Some(0));
    }

    // --- longer-than-minimum reports are accepted ---

    #[test]
    fn longer_report_accepted() {
        let mut data = vec![0u8; 16];
        data[2] = 0x1E; // button 1
        assert_eq!(decode(1, &data), Some(1 << 0));
    }

    // --- keycode helper ---

    #[test]
    fn keycode_range() {
        assert_eq!(keycode(0),  crate::constants::KEY_MACRO_BASE);
        assert_eq!(keycode(11), crate::constants::KEY_MACRO_BASE + 11);
    }
}
