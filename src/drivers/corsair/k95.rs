use evdev::{uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key};
use std::sync::mpsc;
use std::time::Instant;

use crate::drivers::SupplementalDriver;
use crate::event_types::{DisplayEvent, EventHighlight};
use crate::hidraw::{self, HidRawDevice};
use super::gkeys;

pub struct CorsairK95Driver;

impl SupplementalDriver for CorsairK95Driver {
    fn vendor_id(&self) -> u32  { 0x1B1C }
    fn product_id(&self) -> u32 { 0x1B11 }

    fn start(
        self: Box<Self>,
        devices: Vec<HidRawDevice>,
        sender: mpsc::Sender<DisplayEvent>,
        start: Instant,
    ) {
        std::thread::spawn(move || {
            if let Err(e) = run(devices, sender, start) {
                eprintln!("CorsairK95Driver error: {e}");
            }
        });
    }
}

fn build_uinput() -> Option<evdev::uinput::VirtualDevice> {
    let mut key_set = AttributeSet::<Key>::new();
    for i in 0..gkeys::GKEY_COUNT {
        key_set.insert(Key::new(gkeys::keycode(i)));
    }

    match VirtualDeviceBuilder::new()
        .and_then(|b| b.name("gameremap: Corsair K95 G-keys").with_keys(&key_set))
        .and_then(|b| b.build())
    {
        Ok(d)  => Some(d),
        Err(e) => {
            eprintln!(
                "Warning: could not create uinput device ({e}). \
                 G-keys will appear in the UI but won't be injected."
            );
            None
        }
    }
}

fn run(
    devices: Vec<HidRawDevice>,
    sender: mpsc::Sender<DisplayEvent>,
    start: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut vdev = build_uinput();

    let (hid_tx, hid_rx) = mpsc::channel();
    hidraw::start_readers(devices, hid_tx, start);

    let mut prev_mask: u32 = 0;

    for report in hid_rx {
        let Some(mask) = gkeys::decode(&report.data) else {
            continue;
        };
        if mask == prev_mask {
            continue;
        }

        let changed = mask ^ prev_mask;
        let mut uinput_events: Vec<InputEvent> = Vec::new();

        for i in 0..gkeys::GKEY_COUNT {
            if changed & (1 << i) != 0 {
                let pressed = mask & (1 << i) != 0;
                let value_str = if pressed { "press" } else { "release" }.to_string();

                let _ = sender.send(DisplayEvent {
                    elapsed: report.elapsed,
                    device_name: report.device.name.clone(),
                    event_type: "G_KEY",
                    code_name: format!("G{}", i + 1),
                    value_str,
                    highlight: EventHighlight::Gaming,
                });

                if vdev.is_some() {
                    uinput_events.push(InputEvent::new(
                        EventType::KEY,
                        gkeys::keycode(i),
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
