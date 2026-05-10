use eframe::egui;
use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::devices::{self, AccessStatus};
use crate::event_types::{DisplayEvent, EventHighlight};
use crate::hidraw::{HidRawDevice, HidRawReport};

const MAX_EVENTS: usize = 2000;

const SETUP_COMMANDS: &str = "\
sudo usermod -aG input $USER
echo 'KERNEL==\"event*\", SUBSYSTEM==\"input\", GROUP=\"input\", MODE=\"0660\"' | sudo tee    /etc/udev/rules.d/99-gameremap.rules
echo 'KERNEL==\"uinput\",  GROUP=\"input\", MODE=\"0660\"'                      | sudo tee -a /etc/udev/rules.d/99-gameremap.rules
echo 'KERNEL==\"hidraw*\", GROUP=\"input\", MODE=\"0660\"'                      | sudo tee -a /etc/udev/rules.d/99-gameremap.rules
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input";

enum SetupState {
    Idle,
    Running,
    Done,
    Failed(String),
}

#[derive(PartialEq, Clone, Copy)]
enum ViewMode {
    SingleCapture,
    ContinuousLog,
    HidProbe,
}

enum CaptureState {
    Idle,
    Capturing,
    Captured(DisplayEvent),
}

const COLOR_UNKNOWN: egui::Color32 = egui::Color32::from_rgb(255, 80,  80);
const COLOR_GAMING:  egui::Color32 = egui::Color32::from_rgb(255, 200, 50);

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 700.0])
            .with_title("gameremap — Key Monitor"),
        ..Default::default()
    };

    eframe::run_native(
        "gameremap Key Monitor",
        options,
        Box::new(|_cc| Ok(Box::new(DebugApp::new()))),
    )
    .unwrap_or_else(|e| eprintln!("UI error: {e}"));
}

const MAX_HID_REPORTS: usize = 500;

struct DebugApp {
    start: Instant,
    events: Vec<DisplayEvent>,
    receiver: mpsc::Receiver<DisplayEvent>,
    device_names: Vec<String>,
    paused: bool,
    hide_syn: bool,
    hide_rel: bool,
    auto_scroll: bool,
    log_auto_clear: bool,
    log_auto_clear_secs: u32,
    show_permission_popup: bool,
    no_devices: bool,
    setup_state: SetupState,
    setup_rx: Option<mpsc::Receiver<Result<(), String>>>,
    teardown_state: SetupState,
    teardown_rx: Option<mpsc::Receiver<Result<(), String>>>,
    view_mode: ViewMode,
    capture_state: CaptureState,
    // Device filtering — empty set means "all pass through"
    selected_evdev: HashSet<String>,
    selected_hid: HashSet<String>,
    // Log auto-stop
    log_auto_stop: bool,
    log_auto_stop_secs: u32,
    capture_deadline: Option<Instant>,
    // HID probe
    hid_start: Option<Instant>,
    hid_devices: Vec<HidRawDevice>,
    hid_reports: Vec<HidRawReport>,
    hid_receiver: Option<mpsc::Receiver<HidRawReport>>,
    hid_paused: bool,
    hid_auto_scroll: bool,
    hid_auto_clear: bool,
    hid_auto_clear_secs: u32,
}

impl DebugApp {
    fn new() -> Self {
        let (show_permission_popup, no_devices) = match devices::check_access() {
            AccessStatus::Ok        => (false, false),
            AccessStatus::Denied    => (true,  false),
            AccessStatus::NoDevices => (true,  true),
        };

        let start = Instant::now();
        let (tx, rx) = mpsc::channel::<DisplayEvent>();
        let device_infos = devices::start_readers(tx.clone(), start);
        let device_names = device_infos.into_iter().map(|d| d.name).collect();
        crate::drivers::start_supplemental_drivers(tx, start);

        Self {
            start,
            events: Vec::with_capacity(MAX_EVENTS),
            receiver: rx,
            device_names,
            paused: true,
            hide_syn: true,
            hide_rel: true,
            auto_scroll: true,
            log_auto_clear: true,
            log_auto_clear_secs: 30,
            show_permission_popup,
            no_devices,
            setup_state: SetupState::Idle,
            setup_rx: None,
            teardown_state: SetupState::Idle,
            teardown_rx: None,
            view_mode: ViewMode::SingleCapture,
            capture_state: CaptureState::Idle,
            selected_evdev: HashSet::new(),
            selected_hid: HashSet::new(),
            log_auto_stop: false,
            log_auto_stop_secs: 10,
            capture_deadline: None,
            hid_start: None,
            hid_devices: Vec::new(),
            hid_reports: Vec::new(),
            hid_receiver: None,
            hid_paused: false,
            hid_auto_scroll: true,
            hid_auto_clear: false,
            hid_auto_clear_secs: 30,
        }
    }

    fn start_hid_probe(&mut self, start: Instant) {
        let devs = crate::hidraw::enumerate_all();
        if devs.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel::<HidRawReport>();
        crate::hidraw::start_readers(devs.clone(), tx, start);
        self.hid_devices = devs;
        self.hid_receiver = Some(rx);
        self.hid_reports.clear();
        self.hid_start = Some(start);
    }

    fn drain_channel(&mut self) {
        let mut count = 0;
        while let Ok(ev) = self.receiver.try_recv() {
            self.events.push(ev);
            count += 1;
            if count >= 500 {
                break;
            }
        }
        if self.events.len() > MAX_EVENTS {
            let drain = self.events.len() - MAX_EVENTS;
            self.events.drain(..drain);
        }
    }

    fn drain_channel_discard(&mut self) {
        let mut count = 0;
        while self.receiver.try_recv().is_ok() {
            count += 1;
            if count >= 500 {
                break;
            }
        }
    }

    /// Drain the channel; return the first key-press found (evdev or G-key).
    fn drain_for_capture(&mut self) -> Option<DisplayEvent> {
        let mut found: Option<DisplayEvent> = None;
        let mut count = 0;
        while let Ok(ev) = self.receiver.try_recv() {
            if found.is_none()
                && (ev.event_type == "EV_KEY" || ev.event_type == "G_KEY")
                && ev.value_str == "press"
            {
                found = Some(ev);
            }
            count += 1;
            if count >= 500 {
                break;
            }
        }
        found
    }

    fn is_visible(&self, ev: &DisplayEvent) -> bool {
        if self.hide_syn && ev.event_type == "EV_SYN" {
            return false;
        }
        if self.hide_rel && ev.event_type == "EV_REL" {
            return false;
        }
        if !self.selected_evdev.is_empty() && !self.selected_evdev.contains(&ev.device_name) {
            return false;
        }
        true
    }
}

impl eframe::App for DebugApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match self.view_mode {
            ViewMode::ContinuousLog => {
                if !self.paused {
                    if let Some(dl) = self.capture_deadline {
                        if Instant::now() >= dl {
                            self.paused = true;
                            self.capture_deadline = None;
                        }
                    }
                    if !self.paused {
                        self.drain_channel();
                    }
                }
                if self.log_auto_clear {
                    let now = self.start.elapsed();
                    let window = Duration::from_secs(self.log_auto_clear_secs as u64);
                    self.events.retain(|ev| now.saturating_sub(ev.elapsed) <= window);
                }
            }
            ViewMode::SingleCapture => {
                if matches!(self.capture_state, CaptureState::Capturing) {
                    if let Some(ev) = self.drain_for_capture() {
                        self.capture_state = CaptureState::Captured(ev);
                    }
                } else {
                    self.drain_channel_discard();
                }
            }
            ViewMode::HidProbe => {
                self.drain_channel_discard();
                if !self.hid_paused {
                    if let Some(rx) = &self.hid_receiver {
                        let mut count = 0;
                        while let Ok(report) = rx.try_recv() {
                            self.hid_reports.push(report);
                            count += 1;
                            if count >= 200 { break; }
                        }
                        if self.hid_reports.len() > MAX_HID_REPORTS {
                            let drain = self.hid_reports.len() - MAX_HID_REPORTS;
                            self.hid_reports.drain(..drain);
                        }
                    }
                }
                if self.hid_auto_clear {
                    if let Some(hid_start) = self.hid_start {
                        let now = hid_start.elapsed();
                        let window = Duration::from_secs(self.hid_auto_clear_secs as u64);
                        self.hid_reports.retain(|r| now.saturating_sub(r.elapsed) <= window);
                    }
                }
            }
        }

        // Lazily start HID probe when the mode is first selected
        if self.view_mode == ViewMode::HidProbe && self.hid_receiver.is_none() {
            self.start_hid_probe(Instant::now());
        }
        let _ = frame; // suppress unused warning

        // ── Permission popup ──────────────────────────────────────────
        if self.show_permission_popup {
            let screen = ctx.screen_rect();
            ctx.layer_painter(egui::LayerId::new(egui::Order::PanelResizeLine, "dim".into()))
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));

            let heading = if self.no_devices {
                "No Input Devices Found"
            } else {
                "⚠  Input Access Required"
            };

            let popup_size = egui::Vec2::new(screen.width() * 0.55, screen.height() * 0.55);

            egui::Window::new(heading)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .fixed_size(popup_size)
                .show(ctx, |ui| {
                    ui.add_space(6.0);

                    if self.no_devices {
                        ui.label("No /dev/input/event* nodes were found.");
                        ui.label("This is unusual — input devices may not be loaded.");
                    } else {
                        ui.label(egui::RichText::new(
                            "gameremap cannot read /dev/input/event* devices.\n\
                             Your user needs to be in the 'input' group."
                        ).size(15.0));
                    }

                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Run the following, then log out and back in:").size(14.0));
                    ui.add_space(6.0);

                    let button_row_height = 36.0;
                    let available = ui.available_height() - button_row_height - 16.0;
                    egui::Frame::dark_canvas(ui.style()).show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(available)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(
                                    egui::RichText::new(SETUP_COMMANDS).monospace()
                                ));
                            });
                    });

                    if let Some(rx) = &self.setup_rx {
                        if let Ok(result) = rx.try_recv() {
                            self.setup_state = match result {
                                Ok(())   => SetupState::Done,
                                Err(msg) => SetupState::Failed(msg),
                            };
                            self.setup_rx = None;
                        }
                    }
                    if let Some(rx) = &self.teardown_rx {
                        if let Ok(result) = rx.try_recv() {
                            self.teardown_state = match result {
                                Ok(())   => SetupState::Done,
                                Err(msg) => SetupState::Failed(msg),
                            };
                            self.teardown_rx = None;
                        }
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("📋  Copy commands").clicked() {
                            ctx.copy_text(SETUP_COMMANDS.to_string());
                        }

                        ui.separator();

                        match &self.setup_state {
                            SetupState::Idle => {
                                if ui.button("🔧  Run setup for me").clicked() {
                                    let (tx, rx) = mpsc::channel();
                                    self.setup_rx    = Some(rx);
                                    self.setup_state = SetupState::Running;
                                    std::thread::spawn(move || {
                                        let _ = tx.send(devices::run_setup_as_root());
                                    });
                                }
                            }
                            SetupState::Running => {
                                ui.spinner();
                                ui.label("Waiting for authentication…");
                            }
                            SetupState::Done => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 220, 100),
                                    "✓  Done — log out and back in to apply",
                                );
                            }
                            SetupState::Failed(msg) => {
                                ui.colored_label(egui::Color32::from_rgb(255, 80, 80),
                                    format!("✗  {msg}"));
                                if ui.button("Retry").clicked() {
                                    self.setup_state = SetupState::Idle;
                                }
                            }
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Continue anyway").clicked() {
                                self.show_permission_popup = false;
                            }
                        });
                    });

                    ui.add_space(4.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Uninstall:").weak());
                        match &self.teardown_state {
                            SetupState::Idle => {
                                if ui.button("🗑  Remove udev rules").clicked() {
                                    let (tx, rx) = mpsc::channel();
                                    self.teardown_rx    = Some(rx);
                                    self.teardown_state = SetupState::Running;
                                    std::thread::spawn(move || {
                                        let _ = tx.send(devices::run_teardown_as_root());
                                    });
                                }
                            }
                            SetupState::Running => {
                                ui.spinner();
                                ui.label("Removing…");
                            }
                            SetupState::Done => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 220, 100),
                                    "✓  Rules removed",
                                );
                            }
                            SetupState::Failed(msg) => {
                                ui.colored_label(egui::Color32::from_rgb(255, 80, 80),
                                    format!("✗  {msg}"));
                                if ui.button("Retry").clicked() {
                                    self.teardown_state = SetupState::Idle;
                                }
                            }
                        }
                        ui.weak("(does not remove user from 'input' group)");
                    });
                    ui.add_space(4.0);
                });
        }

        // ── Controls bar ─────────────────────────────────────────────
        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view_mode, ViewMode::SingleCapture, "Capture");
                ui.selectable_value(&mut self.view_mode, ViewMode::ContinuousLog, "Log");
                ui.selectable_value(&mut self.view_mode, ViewMode::HidProbe, "HID Probe");
                ui.separator();

                match self.view_mode {
                    ViewMode::ContinuousLog => {
                        let pause_label = if self.paused { "▶ Resume" } else { "⏸ Pause" };
                        if ui.button(pause_label).clicked() {
                            self.paused = !self.paused;
                            if !self.paused && self.log_auto_stop {
                                self.capture_deadline = Some(
                                    Instant::now() + Duration::from_secs(self.log_auto_stop_secs as u64)
                                );
                            } else {
                                self.capture_deadline = None;
                            }
                        }
                        if ui.button("🗑 Clear").clicked() {
                            self.events.clear();
                        }
                        ui.separator();
                        ui.checkbox(&mut self.hide_syn, "Hide EV_SYN");
                        ui.checkbox(&mut self.hide_rel, "Hide EV_REL");
                        ui.separator();
                        ui.checkbox(&mut self.auto_scroll, "Auto-scroll");
                        ui.separator();
                        ui.checkbox(&mut self.log_auto_stop, "Stop after");
                        ui.add_enabled(
                            self.log_auto_stop,
                            egui::DragValue::new(&mut self.log_auto_stop_secs)
                                .range(1..=120)
                                .suffix("s"),
                        );
                        if let Some(dl) = self.capture_deadline {
                            let left = dl.saturating_duration_since(Instant::now());
                            ui.label(
                                egui::RichText::new(format!("({:.1}s)", left.as_secs_f32()))
                                    .color(COLOR_GAMING),
                            );
                        }
                        ui.separator();
                        ui.checkbox(&mut self.log_auto_clear, "Clear after");
                        ui.add_enabled(
                            self.log_auto_clear,
                            egui::DragValue::new(&mut self.log_auto_clear_secs)
                                .range(5..=300)
                                .suffix("s"),
                        );
                        ui.separator();
                        let visible = self.events.iter().filter(|e| self.is_visible(e)).count();
                        ui.label(format!("{visible} events | {} devices", self.device_names.len()));
                    }
                    ViewMode::HidProbe => {
                        let pause_label = if self.hid_paused { "▶ Resume" } else { "⏸ Pause" };
                        if ui.button(pause_label).clicked() {
                            self.hid_paused = !self.hid_paused;
                        }
                        if ui.button("🗑 Clear").clicked() {
                            self.hid_reports.clear();
                        }
                        if ui.button("📋 Copy").clicked() {
                            let filter = &self.selected_hid;
                            let text = self.hid_reports.iter()
                                .filter(|r| filter.is_empty() || filter.contains(&r.device.hidraw_path))
                                .map(|r| {
                                    let iface = r.device.hidraw_path.rsplit('/').next().unwrap_or("?");
                                    let hex = r.data.iter()
                                        .map(|b| format!("{b:02X}"))
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    format!("{:.3}  {} if{}  {}", r.elapsed.as_secs_f64(), iface, r.device.interface, hex)
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            ctx.copy_text(text);
                        }
                        ui.separator();
                        ui.checkbox(&mut self.hid_auto_scroll, "Auto-scroll");
                        ui.separator();
                        ui.checkbox(&mut self.hid_auto_clear, "Clear after");
                        ui.add_enabled(
                            self.hid_auto_clear,
                            egui::DragValue::new(&mut self.hid_auto_clear_secs)
                                .range(5..=300)
                                .suffix("s"),
                        );
                        ui.separator();
                        ui.label(format!(
                            "{} reports | {} hidraw interfaces",
                            self.hid_reports.len(),
                            self.hid_devices.len(),
                        ));
                    }
                    ViewMode::SingleCapture => {
                        ui.label(format!("{} devices", self.device_names.len()));
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(COLOR_UNKNOWN, "■ KEY_UNKNOWN");
                    ui.label("  ");
                    ui.colored_label(COLOR_GAMING, "■ gaming key");
                    ui.separator();
                    if ui.button("⚙ Setup…").clicked() {
                        self.show_permission_popup = true;
                    }
                });
            });
        });

        // ── Side panel: device list ───────────────────────────────────
        egui::SidePanel::left("devices")
            .resizable(true)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.heading("Devices");
                ui.separator();

                match self.view_mode {
                    ViewMode::SingleCapture => {
                        // Highlight whichever device the last captured event came from
                        let captured_device = if let CaptureState::Captured(ev) = &self.capture_state {
                            Some(ev.device_name.as_str())
                        } else {
                            None
                        };
                        if self.device_names.is_empty() {
                            ui.colored_label(egui::Color32::RED, "No devices found.");
                            ui.label("Add user to 'input' group:");
                            ui.code("sudo usermod -aG input $USER");
                        } else {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for name in &self.device_names {
                                    if captured_device == Some(name.as_str()) {
                                        ui.label(
                                            egui::RichText::new(name)
                                                .strong()
                                                .color(egui::Color32::from_rgb(100, 200, 255)),
                                        );
                                    } else {
                                        ui.label(name);
                                    }
                                }
                            });
                        }
                    }

                    ViewMode::ContinuousLog => {
                        if self.device_names.is_empty() {
                            ui.colored_label(egui::Color32::RED, "No devices found.");
                            ui.label("Add user to 'input' group:");
                            ui.code("sudo usermod -aG input $USER");
                        } else {
                            if !self.selected_evdev.is_empty() {
                                if ui.small_button("Show all").clicked() {
                                    self.selected_evdev.clear();
                                }
                            }
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for name in &self.device_names {
                                    let sel = self.selected_evdev.contains(name.as_str());
                                    if ui.selectable_label(sel, name).clicked() {
                                        if sel {
                                            self.selected_evdev.remove(name.as_str());
                                        } else {
                                            self.selected_evdev.insert(name.clone());
                                        }
                                    }
                                }
                            });
                            if !self.selected_evdev.is_empty() {
                                ui.separator();
                                ui.label(
                                    egui::RichText::new("Filtering active").weak().italics()
                                );
                            }
                        }
                    }

                    ViewMode::HidProbe => {
                        if self.hid_devices.is_empty() {
                            ui.label(egui::RichText::new("No hidraw devices found.").weak());
                        } else {
                            if !self.selected_hid.is_empty() {
                                if ui.small_button("Show all").clicked() {
                                    self.selected_hid.clear();
                                }
                            }
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for dev in &self.hid_devices {
                                    let label = format!(
                                        "{} (if{})",
                                        dev.hidraw_path.rsplit('/').next().unwrap_or("?"),
                                        dev.interface,
                                    );
                                    let sel = self.selected_hid.contains(&dev.hidraw_path);
                                    if ui.selectable_label(sel, &label).clicked() {
                                        if sel {
                                            self.selected_hid.remove(&dev.hidraw_path);
                                        } else {
                                            self.selected_hid.insert(dev.hidraw_path.clone());
                                        }
                                    }
                                }
                            });
                            if !self.selected_hid.is_empty() {
                                ui.separator();
                                ui.label(
                                    egui::RichText::new("Filtering active").weak().italics()
                                );
                            }
                        }
                    }
                }
            });

        // ── Central panel ─────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.view_mode == ViewMode::HidProbe {
                // ── HID raw probe ─────────────────────────────────────
                if self.hid_devices.is_empty() {
                    ui.add_space(ui.available_height() * 0.35);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("No HID devices found.").size(16.0));
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(
                            "Check that /dev/hidraw* nodes are readable\n\
                             (user in 'input' group, or udev rules applied)."
                        ).weak());
                    });
                } else {
                    // Column headers
                    egui::Grid::new("hid_header")
                        .num_columns(5)
                        .min_col_width(60.0)
                        .show(ui, |ui| {
                            ui.strong("Time");
                            ui.strong("Interface");
                            ui.strong("Len");
                            ui.strong("Keys");
                            ui.strong("Bytes (hex)");
                            ui.end_row();
                        });
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .stick_to_bottom(self.hid_auto_scroll)
                        .show(ui, |ui| {
                            egui::Grid::new("hid_reports")
                                .num_columns(5)
                                .min_col_width(60.0)
                                .striped(true)
                                .show(ui, |ui| {
                                    for report in self.hid_reports.iter().filter(|r| {
                                        self.selected_hid.is_empty()
                                            || self.selected_hid.contains(&r.device.hidraw_path)
                                    }) {
                                        ui.label(DisplayEvent::format_elapsed(report.elapsed));
                                        ui.label(format!(
                                            "{} if{}",
                                            report.device.hidraw_path
                                                .rsplit('/').next().unwrap_or("?"),
                                            report.device.interface,
                                        ));
                                        ui.label(format!("{}", report.data.len()));

                                        // Decoded G-key names (if this is a G-key report)
                                        match crate::drivers::corsair::gkeys::decode(&report.data) {
                                            Some(mask) if mask > 0 => {
                                                ui.label(
                                                    egui::RichText::new(crate::drivers::corsair::gkeys::names(mask))
                                                        .strong()
                                                        .color(COLOR_GAMING),
                                                );
                                            }
                                            Some(_) => { ui.label(""); } // release
                                            None    => { ui.label(""); } // other report type
                                        }

                                        // Raw bytes — highlight changed ones in yellow
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            for (i, &byte) in report.data.iter().enumerate() {
                                                let txt = egui::RichText::new(
                                                    format!("{byte:02X}")
                                                ).monospace();
                                                let changed = report.changed.get(i).copied().unwrap_or(false);
                                                if changed {
                                                    ui.label(txt.color(COLOR_GAMING));
                                                } else {
                                                    ui.label(txt);
                                                }
                                            }
                                        });
                                        ui.end_row();
                                    }
                                });
                        });
                }
            } else if self.view_mode == ViewMode::ContinuousLog {
                // ── Event table ───────────────────────────────────────
                egui::Grid::new("header")
                    .num_columns(5)
                    .min_col_width(60.0)
                    .show(ui, |ui| {
                        ui.strong("Time");
                        ui.strong("Device");
                        ui.strong("Type");
                        ui.strong("Code");
                        ui.strong("Value");
                        ui.end_row();
                    });

                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .stick_to_bottom(self.auto_scroll)
                    .show(ui, |ui| {
                        egui::Grid::new("events")
                            .num_columns(5)
                            .min_col_width(60.0)
                            .striped(true)
                            .show(ui, |ui| {
                                for ev in self.events.iter().filter(|e| self.is_visible(e)) {
                                    ui.label(DisplayEvent::format_elapsed(ev.elapsed));
                                    let name = if ev.device_name.len() > 28 {
                                        format!("{}…", &ev.device_name[..27])
                                    } else {
                                        ev.device_name.clone()
                                    };
                                    ui.label(name);
                                    ui.label(ev.event_type);
                                    match ev.highlight {
                                        EventHighlight::Unknown =>
                                            ui.colored_label(COLOR_UNKNOWN, &ev.code_name),
                                        EventHighlight::Gaming =>
                                            ui.colored_label(COLOR_GAMING, &ev.code_name),
                                        _ =>
                                            ui.label(&ev.code_name),
                                    };
                                    ui.label(&ev.value_str);
                                    ui.end_row();
                                }
                            });
                    });
            } else {
                // ── Single capture UI ─────────────────────────────────
                let mut next_state: Option<CaptureState> = None;

                ui.add_space(ui.available_height() * 0.28);
                ui.vertical_centered(|ui| {
                    match &self.capture_state {
                        CaptureState::Idle => {
                            if ui.button(egui::RichText::new("▶  Start Capture").size(22.0)).clicked() {
                                next_state = Some(CaptureState::Capturing);
                            }
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new(
                                "Press the key or button you want to identify."
                            ).weak());
                        }

                        CaptureState::Capturing => {
                            ui.spinner();
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("Listening… press any key").size(18.0));
                            ui.add_space(14.0);
                            if ui.button("Cancel").clicked() {
                                next_state = Some(CaptureState::Idle);
                            }
                        }

                        CaptureState::Captured(ev) => {
                            let code_name   = ev.code_name.clone();
                            let device_name = ev.device_name.clone();
                            let highlight   = ev.highlight.clone();

                            let text = egui::RichText::new(&code_name).size(42.0).strong();
                            let text = match highlight {
                                EventHighlight::Unknown => text.color(COLOR_UNKNOWN),
                                EventHighlight::Gaming  => text.color(COLOR_GAMING),
                                _                       => text,
                            };
                            ui.label(text);
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(format!("from: {device_name}"))
                                    .size(13.0)
                                    .weak(),
                            );
                            ui.add_space(22.0);
                            if ui.button(egui::RichText::new("▶  Capture Again").size(18.0)).clicked() {
                                next_state = Some(CaptureState::Capturing);
                            }
                        }
                    }
                });

                if let Some(state) = next_state {
                    self.capture_state = state;
                }
            }
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}
