/// Diagnostic subcommand: prints evdev device enumeration and key name resolution.
/// Run with: cargo run -- diagnose
pub fn run() {
    println!("=== Key name check ===");
    for code in [0x290u16, 0x291, 0x292] {
        let k = evdev::Key::new(code);
        println!("  code 0x{code:03X} = {k:?}");
    }

    println!("\n=== evdev devices (before supplemental drivers) ===");
    print_devices();

    println!("\n=== Starting supplemental drivers ===");
    let (tx, _) = std::sync::mpsc::channel::<crate::event_types::DisplayEvent>();
    crate::drivers::start_supplemental_drivers(tx, std::time::Instant::now());

    println!("Waiting 500ms for uinput to materialise…");
    std::thread::sleep(std::time::Duration::from_millis(500));

    println!("\n=== evdev devices (after supplemental drivers) ===");
    print_devices();

    println!("\n=== Profile assignment source keys ===");
    let profiles = crate::config::store::load_profiles();
    for p in &profiles {
        println!("  profile {:?}", p.name);
        for a in &p.assignments {
            println!("    source_key={:?}  source_device={:?}", a.source_key, a.source_device);
        }
    }

    println!("\n=== Session device match simulation ===");
    for p in &profiles {
        let key_names: Vec<&str> = p.assignments.iter()
            .map(|a| a.source_key.as_str())
            .chain(p.modifiers.iter().map(|m| m.key.as_str()))
            .collect();
        println!("  profile {:?}  looking for: {:?}", p.name, key_names);
        let mut found = 0usize;
        for (path, dev) in evdev::enumerate() {
            let name = dev.name().unwrap_or("(unnamed)").to_string();
            let matched: Vec<&str> = dev.supported_keys()
                .map(|s| key_names.iter().copied()
                    .filter(|&kn| s.iter().any(|k| format!("{k:?}") == kn))
                    .collect::<Vec<_>>())
                .unwrap_or_default();
            if !matched.is_empty() {
                println!("    MATCH  {} {:?}  keys: {:?}", path.display(), name, matched);
                found += 1;
            }
        }
        if found == 0 {
            println!("    NO MATCHING DEVICES");
        }
    }
}

fn print_devices() {
    for (path, dev) in evdev::enumerate() {
        let name = dev.name().unwrap_or("(unnamed)");
        let macro_keys: Vec<String> = dev.supported_keys()
            .map(|s| s.iter()
                .filter(|k| format!("{k:?}").contains("MACRO"))
                .map(|k| format!("{k:?}"))
                .collect())
            .unwrap_or_default();
        if !macro_keys.is_empty() {
            println!("  {} {:?}  MACRO keys: {:?}", path.display(), name, macro_keys);
        } else {
            println!("  {} {:?}", path.display(), name);
        }
    }
}
