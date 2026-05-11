mod config;
mod constants;
mod engine;
mod debug_ui;
mod devices;
mod diagnose;
mod drivers;
mod event_types;
mod hidraw;
mod injector;
mod monitor;
mod profile_ui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gameremap", about = "Linux gaming input remapper")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the profile / macro / assignment editor
    Profile,
    /// Launch the live key monitor window
    Debug {
        /// Initial view: capture | log | hid
        #[arg(long, default_value = "capture")]
        mode: String,
    },
    /// Inject Corsair G-keys as KEY_MACRO1–KEY_MACRO18 uinput events
    Gkeys,
    /// Watch the remapper's output device and print injected events (for testing)
    Monitor,
    /// Print evdev device enumeration and key name diagnostics
    Diagnose,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Profile  => profile_ui::run(),
        Commands::Debug { mode } => debug_ui::run(&mode),
        Commands::Gkeys    => injector::run(),
        Commands::Monitor  => monitor::run(),
        Commands::Diagnose => diagnose::run(),
    }
}
