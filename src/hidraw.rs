use std::io::Read;
use std::sync::mpsc;
use std::time::Instant;

#[derive(Clone)]
pub struct HidRawDevice {
    pub hidraw_path: String,
    pub name: String,
    pub vid: u32,
    pub pid: u32,
    /// USB interface number (0, 1, 2, …)
    pub interface: u8,
}

#[derive(Clone)]
pub struct HidRawReport {
    pub elapsed: std::time::Duration,
    pub device: HidRawDevice,
    pub data: Vec<u8>,
    /// Bitmask: true at index i if data[i] differs from the previous report on this interface.
    /// Empty if this is the first report seen on this interface.
    pub changed: Vec<bool>,
}

impl HidRawReport {
    pub fn hex(&self) -> String {
        self.data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }
}

/// Parse hidraw entries from `/sys/class/hidraw` with optional VID filtering.
fn parse_hidraw_entries(vid_filter: Option<u32>) -> Vec<HidRawDevice> {
    let Ok(dir) = std::fs::read_dir("/sys/class/hidraw") else {
        return Vec::new();
    };

    let mut devices = Vec::new();
    for entry in dir.filter_map(|e| e.ok()) {
        let hidraw_name = entry.file_name().to_string_lossy().to_string();
        let uevent_path = entry.path().join("device/uevent");
        let Ok(uevent) = std::fs::read_to_string(&uevent_path) else { continue };

        let Some(hid_id) = uevent.lines()
            .find(|l| l.starts_with("HID_ID="))
            .and_then(|l| l.strip_prefix("HID_ID="))
        else { continue };

        let parts: Vec<&str> = hid_id.split(':').collect();
        if parts.len() != 3 { continue; }
        let Ok(dev_vid) = u32::from_str_radix(parts[1], 16) else { continue };
        let Ok(dev_pid) = u32::from_str_radix(parts[2], 16) else { continue };

        if !vid_filter.map_or(true, |f| dev_vid == f) { continue; }

        let interface = uevent.lines()
            .find(|l| l.starts_with("HID_PHYS="))
            .and_then(|l| l.strip_prefix("HID_PHYS="))
            .and_then(|s| s.rsplit('/').next())
            .and_then(|s| s.strip_prefix("input"))
            .and_then(|n| n.parse::<u8>().ok())
            .unwrap_or(0xFF);

        let name = uevent.lines()
            .find(|l| l.starts_with("HID_NAME="))
            .and_then(|l| l.strip_prefix("HID_NAME="))
            .unwrap_or("Unknown")
            .to_string();

        devices.push(HidRawDevice {
            hidraw_path: format!("/dev/{hidraw_name}"),
            name,
            vid: dev_vid,
            pid: dev_pid,
            interface,
        });
    }

    devices.sort_by_key(|d| (d.vid, d.pid, d.interface));
    devices
}

/// Find all hidraw nodes on the system regardless of VID.
pub fn enumerate_all() -> Vec<HidRawDevice> {
    parse_hidraw_entries(None)
}

/// Find all hidraw nodes for a given VID (e.g. 0x1b1c for Corsair).
pub fn enumerate_by_vid(vid: u32) -> Vec<HidRawDevice> {
    parse_hidraw_entries(Some(vid))
}

pub fn start_readers(
    devices: Vec<HidRawDevice>,
    sender: mpsc::Sender<HidRawReport>,
    start: Instant,
) {
    for dev in devices {
        let tx = sender.clone();
        std::thread::spawn(move || {
            let Ok(mut f) = std::fs::File::open(&dev.hidraw_path) else {
                eprintln!("Cannot open {} (permission denied?)", dev.hidraw_path);
                return;
            };

            let mut buf = [0u8; 64];
            let mut prev: Vec<u8> = Vec::new();

            loop {
                match f.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        let changed = if prev.is_empty() {
                            vec![false; n]
                        } else {
                            data.iter().enumerate()
                                .map(|(i, &b)| prev.get(i).map_or(true, |&p| p != b))
                                .collect()
                        };
                        prev = data.clone();
                        let _ = tx.send(HidRawReport {
                            elapsed: start.elapsed(),
                            device: dev.clone(),
                            data,
                            changed,
                        });
                    }
                    Err(e) => {
                        eprintln!("hidraw read error on {}: {e}", dev.hidraw_path);
                        break;
                    }
                }
            }
        });
    }
}
