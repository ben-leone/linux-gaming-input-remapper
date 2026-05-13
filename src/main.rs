mod config;
mod constants;
mod engine;
mod debug_ui;
mod devices;
mod diagnose;
mod drivers;
mod event_types;
mod hidraw;
mod monitor;
mod profile_ui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gameremap", about = "Linux gaming input remapper")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the profile / macro / assignment editor (default)
    Profile,
    /// Launch the live key monitor window
    Debug {
        /// Initial view: capture | log | hid
        #[arg(long, default_value = "capture")]
        mode: String,
    },
    /// Watch the remapper's output device and print injected events (for testing)
    Monitor,
    /// Print evdev device enumeration and key name diagnostics
    Diagnose,
}

fn main() {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Profile) {
        Commands::Profile      => profile_ui::run(),
        Commands::Debug { mode } => debug_ui::run(&mode),
        Commands::Monitor      => monitor::run(),
        Commands::Diagnose     => diagnose::run(),
    }
}
