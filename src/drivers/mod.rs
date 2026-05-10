use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Instant;

use crate::event_types::DisplayEvent;
use crate::hidraw::HidRawDevice;

pub mod corsair;
pub mod utechsmart;

pub trait SupplementalDriver: Send {
    fn vendor_id(&self) -> u32;
    fn product_id(&self) -> u32;
    fn start(
        self: Box<Self>,
        devices: Vec<HidRawDevice>,
        sender: mpsc::Sender<DisplayEvent>,
        start: Instant,
    );
}

pub fn start_supplemental_drivers(
    sender: mpsc::Sender<DisplayEvent>,
    start: Instant,
) {
    let drivers: Vec<Box<dyn SupplementalDriver>> = corsair::registered_drivers()
        .into_iter()
        .chain(utechsmart::registered_drivers())
        .collect();

    // Enumerate hidraw once per unique VID to avoid redundant sysfs scans.
    let mut all_hid: Vec<HidRawDevice> = Vec::new();
    let mut seen_vids = HashSet::new();
    for driver in &drivers {
        if seen_vids.insert(driver.vendor_id()) {
            all_hid.extend(crate::hidraw::enumerate_by_vid(driver.vendor_id()));
        }
    }

    for driver in drivers {
        let vid = driver.vendor_id();
        let pid = driver.product_id();
        let matching: Vec<HidRawDevice> = all_hid
            .iter()
            .filter(|d| d.vid == vid && d.pid == pid)
            .cloned()
            .collect();
        if !matching.is_empty() {
            driver.start(matching, sender.clone(), start);
        }
    }
}
