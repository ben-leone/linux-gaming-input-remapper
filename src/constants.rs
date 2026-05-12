// ── Engine: trigger timing ────────────────────────────────────────────────────

/// Upper bound (ms) for a QuickPress trigger: hold shorter than this → quick press.
pub const QUICK_PRESS_MAX_MS: u64 = 250;

/// Upper bound (ms) for a ShortHold trigger: hold between QUICK_PRESS_MAX_MS and this → short hold.
pub const SHORT_HOLD_MAX_MS: u64 = 500;

// ── Engine: lifecycle timing ──────────────────────────────────────────────────

/// Seconds ESC must be held continuously before the emergency process exit fires.
pub const ESC_EXIT_HOLD_SECS: u64 = 7;

/// Milliseconds to wait after starting supplemental drivers before first use.
pub const DRIVER_STARTUP_WAIT_MS: u64 = 200;

/// Milliseconds to wait for the event processor to exit gracefully before aborting it.
pub const SESSION_SHUTDOWN_TIMEOUT_MS: u64 = 300;

// ── Engine: buffer sizes ──────────────────────────────────────────────────────

/// Tokio mpsc channel capacity for raw evdev events (InputEvent, device_name).
pub const EVENT_CHANNEL_CAPACITY: usize = 512;

/// Maximum entries kept in the in-process engine diagnostic log.
pub const ENGINE_LOG_CAPACITY: usize = 500;

// ── Auto-detect scanner ───────────────────────────────────────────────────────

/// How long (ms) the scanner waits between /proc scans when no game is running.
pub const AUTODETECT_SEARCH_INTERVAL_MS: u64 = 10_000;

/// How long (ms) the scanner waits between liveness checks once a game is detected.
pub const AUTODETECT_MONITOR_INTERVAL_MS: u64 = 30_000;

/// Brief pause (ms) after a game is lost before resuming search mode.
pub const AUTODETECT_LOST_PAUSE_MS: u64 = 2_000;

/// Granularity (ms) of the cancellable sleep used by the scanner thread.
pub const AUTODETECT_SLEEP_GRANULE_MS: u64 = 100;

// ── Key code ranges ───────────────────────────────────────────────────────────

/// evdev code of KEY_MACRO1; KEY_MACROx = KEY_MACRO_BASE + (x - 1).
pub const KEY_MACRO_BASE: u16 = 0x290;

/// evdev code of KEY_MACRO30 (last macro key supported).
pub const KEY_MACRO_MAX: u16 = 0x2AD;

/// evdev code of BTN_TRIGGER_HAPPY1.
pub const BTN_TRIGGER_HAPPY_BASE: u16 = 0x2C0;

/// evdev code of BTN_TRIGGER_HAPPY40.
pub const BTN_TRIGGER_HAPPY_MAX: u16 = 0x2E7;

// ── HID ──────────────────────────────────────────────────────────────────────

/// Read buffer size for a single HID report (bytes).
pub const HID_READ_BUF_SIZE: usize = 64;

// ── UI: repaint intervals ─────────────────────────────────────────────────────

/// Fast repaint interval (ms) used during key-learn and macro recording polling.
pub const UI_REPAINT_FAST_MS: u64 = 50;

/// Normal repaint interval (ms) used for engine log drain.
pub const UI_REPAINT_NORMAL_MS: u64 = 100;

/// Slow repaint interval (seconds) used by the auto-detect scanner heartbeat.
pub const UI_REPAINT_AUTODETECT_SECS: u64 = 10;

/// Debug UI repaint interval (ms) — targets ~60 fps for the event log.
pub const DEBUG_UI_REPAINT_MS: u64 = 16;

// ── Config defaults ───────────────────────────────────────────────────────────

/// Default delay (ms) inserted before a loop-event macro fires.
/// Gives the game time to register the last emitted key before the next action.
pub const DEFAULT_EVENT_DELAY_MS: u32 = 100;
