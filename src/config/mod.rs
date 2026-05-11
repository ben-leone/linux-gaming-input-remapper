pub mod store;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── App-level settings ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub auto_switch_profiles: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { auto_switch_profiles: false }
    }
}

// ── Profile ───────────────────────────────────────────────────────────────────

fn default_blocked_keys() -> Vec<String> {
    vec!["KEY_ESC".into(), "BTN_LEFT".into(), "BTN_RIGHT".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub game_link: Option<GameLink>,
    #[serde(default)]
    pub modifiers: Vec<ModifierKey>,
    #[serde(default)]
    pub macros: Vec<Macro>,
    #[serde(default)]
    pub assignments: Vec<KeyAssignment>,
    #[serde(default = "default_blocked_keys")]
    pub blocked_keys: Vec<String>,
    #[serde(default)]
    pub auto_detect: Vec<AutoDetectTarget>,
}

impl Profile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            game_link: None,
            modifiers: Vec::new(),
            macros: Vec::new(),
            assignments: Vec::new(),
            blocked_keys: default_blocked_keys(),
            auto_detect: Vec::new(),
        }
    }
}

// ── Game link ─────────────────────────────────────────────────────────────────

/// At least one of steam_id / process should be set for detection to work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameLink {
    pub display_name: String,
    pub steam_id: Option<u32>,
    pub process: Option<String>,
}

// ── Auto-detect targets ───────────────────────────────────────────────────────

/// One auto-detect target: ALL fragments must appear (case-insensitive) in the
/// process exe path or cmdline for this target to match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDetectTarget {
    pub id: Uuid,
    #[serde(default)]
    pub fragments: Vec<String>,
}

impl AutoDetectTarget {
    pub fn new() -> Self {
        Self { id: Uuid::new_v4(), fragments: Vec::new() }
    }
}

// ── Modifier keys ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifierKey {
    pub id: Uuid,
    /// evdev key name, e.g. "KEY_LEFTALT"
    pub key: String,
    /// User-visible label, e.g. "Alt"
    pub name: String,
}

impl ModifierKey {
    pub fn new(key: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4(), key: key.into(), name: name.into() }
    }
}

// ── Macros ────────────────────────────────────────────────────────────────────

fn is_zero_u32(v: &u32) -> bool { *v == 0 }
fn default_event_delay_ms() -> u32 { crate::constants::DEFAULT_EVENT_DELAY_MS }
fn is_default_event_delay(v: &u32) -> bool { *v == crate::constants::DEFAULT_EVENT_DELAY_MS }

/// Controls when a Single-macro assignment fires relative to the key hold duration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    /// Fires immediately on key press (default behaviour).
    #[default]
    Any,
    /// Fires on key release if held less than 50 ms.
    QuickPress,
    /// Fires on key release if held 50–200 ms.
    ShortHold,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FireMode {
    /// Executes once on key press.
    Single,
    /// Repeats continuously while key is held.
    Loop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepMode {
    /// Each step presses-then-releases a key, with an optional delay after.
    Simple,
    /// Full control over individual press/release events and their timing.
    Advanced,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Macro {
    pub id: Uuid,
    pub name: String,
    pub fire: FireMode,
    pub mode: StepMode,
    /// Delay (ms) before the first step fires.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub start_delay_ms: u32,
    /// For Loop macros: delay (ms) inserted between the end of one iteration and the start of the next.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub loop_delay_ms: u32,
    #[serde(default)]
    pub simple_steps: Vec<SimpleStep>,
    #[serde(default)]
    pub advanced_steps: Vec<AdvancedStep>,
    /// Events that fire at specific lifecycle points during loop execution.
    /// Only meaningful when fire == FireMode::Loop.
    #[serde(default)]
    pub events: Vec<LoopEvent>,
}

impl Macro {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            fire: FireMode::Single,
            mode: StepMode::Simple,
            start_delay_ms: 0,
            loop_delay_ms: 0,
            simple_steps: Vec::new(),
            advanced_steps: Vec::new(),
            events: Vec::new(),
        }
    }
}

/// One step in simple mode: hold `action` for `hold_ms`, then wait `delay_after_ms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleStep {
    /// evdev key/button name, e.g. "KEY_Q" or "BTN_LEFT"
    pub action: String,
    /// How long to hold the key between DOWN and UP, in milliseconds.
    #[serde(default)]
    pub hold_ms: u32,
    /// How long to wait after the UP event before the next step, in milliseconds.
    pub delay_after_ms: u32,
}

/// One step in advanced mode: a single down or up event at a specific time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvancedEvent {
    KeyDown,
    KeyUp,
    MouseDown,
    MouseUp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedStep {
    pub event: AdvancedEvent,
    /// evdev key/button name
    pub key: String,
    /// Absolute time offset from macro start, in milliseconds.
    pub time_ms: u32,
}

/// One event attached to a loop macro. Events fire at specific points
/// in the loop lifecycle and call a referenced Single-fire macro.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopEvent {
    pub id: Uuid,
    /// UUID of a Single-fire macro to invoke when this event fires.
    pub macro_id: Uuid,
    /// Scheduling order:
    ///   0  = fires before every loop iteration (pre-action)
    ///  -1  = fires when key is released / loop ends (end action)
    ///   N  = fires at the end of every Nth complete cycle (N > 0)
    pub order: i32,
    /// For end events (order == -1): minimum complete cycles that must
    /// have run before this end event fires. Ignored for other orders.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub min_loops: u32,
    /// Milliseconds to wait before invoking the event macro.
    #[serde(default = "default_event_delay_ms", skip_serializing_if = "is_default_event_delay")]
    pub delay_ms: u32,
}

// ── Key assignments ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyAssignment {
    pub id: Uuid,
    /// Source evdev key name, e.g. "KEY_MACRO1"
    pub source_key: String,
    /// Optional device filter: evdev device name (from `Device::name()`).
    /// `None` matches any device; `Some(name)` only matches that device.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_device: Option<String>,
    /// Modifier key IDs (from this profile) that must be held simultaneously.
    #[serde(default)]
    pub modifiers: Vec<Uuid>,
    /// If set, remap to this key (mutually exclusive with macro_id).
    pub remap_key: Option<String>,
    /// If set, trigger this macro (mutually exclusive with remap_key).
    pub macro_id: Option<Uuid>,
    /// For Single-macro assignments: when to fire relative to hold duration.
    /// Ignored for remap assignments.
    #[serde(default)]
    pub trigger_mode: TriggerMode,
}

impl KeyAssignment {
    pub fn new(source_key: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            source_key: source_key.into(),
            source_device: None,
            modifiers: Vec::new(),
            remap_key: None,
            macro_id: None,
            trigger_mode: TriggerMode::Any,
        }
    }
}
