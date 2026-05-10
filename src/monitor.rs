/// Watches the daemon's uinput output device and prints every injected event.
/// Run this in a terminal while pressing side buttons to verify the remap pipeline.
pub fn run() {
    let target_name = "gameremap";

    let matching: Vec<(std::path::PathBuf, evdev::Device)> = evdev::enumerate()
        .filter(|(_, d)| d.name().map(|n| n == target_name).unwrap_or(false))
        .collect();

    if matching.is_empty() {
        eprintln!(
            "No evdev device named {:?} found.\n\
             Make sure the daemon is running and a profile is loaded \
             (click Run in the profile editor).",
            target_name
        );
        std::process::exit(1);
    }

    println!("Monitoring {} device(s) named {:?}:", matching.len(), target_name);
    for (path, d) in &matching {
        println!("  {} — {}", path.display(), d.name().unwrap_or("?"));
    }
    println!("Press side buttons or mapped keys. Ctrl-C to stop.\n");

    let handles: Vec<_> = matching
        .into_iter()
        .map(|(path, mut device)| {
            std::thread::spawn(move || loop {
                match device.fetch_events() {
                    Ok(events) => {
                        for event in events {
                            match event.kind() {
                                evdev::InputEventKind::Key(key) => {
                                    let dir = match event.value() {
                                        1 => "press  ",
                                        0 => "release",
                                        2 => "repeat ",
                                        v => {
                                            println!("{} unknown value {v}  {key:?}", path.display());
                                            continue;
                                        }
                                    };
                                    println!("{dir}  {key:?}  [{}]", path.display());
                                }
                                evdev::InputEventKind::Synchronization(_) => {}
                                other => {
                                    println!("evt    {other:?}  [{}]", path.display());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("read error on {}: {e}", path.display());
                        break;
                    }
                }
            })
        })
        .collect();

    for h in handles {
        let _ = h.join();
    }
}
