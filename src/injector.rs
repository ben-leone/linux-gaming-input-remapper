use std::sync::mpsc;
use std::time::Instant;

/// Headless G-key injector: detects Corsair devices and runs all supplemental
/// drivers, emitting uinput events until Ctrl-C.
pub fn run() {
    let (tx, _rx) = mpsc::channel();
    crate::drivers::start_supplemental_drivers(tx, Instant::now());

    println!("Supplemental drivers started. G-keys active. Press Ctrl-C to stop.");

    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
