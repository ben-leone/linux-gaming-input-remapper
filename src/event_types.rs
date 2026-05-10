use std::time::Duration;

#[derive(Clone)]
pub struct DisplayEvent {
    pub elapsed: Duration,
    pub device_name: String,
    pub event_type: &'static str,
    pub code_name: String,
    pub value_str: String,
    pub highlight: EventHighlight,
}

#[derive(Clone, PartialEq)]
pub enum EventHighlight {
    Normal,
    /// Key code unknown to the kernel — raw vendor scan code; interesting for device DB
    Unknown,
    /// Named gaming key (KEY_MACRO*, BTN_TRIGGER_HAPPY*, KEY_F13-F24)
    Gaming,
    Sync,
}

impl DisplayEvent {
    pub fn format_elapsed(d: Duration) -> String {
        let ms = d.as_millis();
        format!("{:02}:{:02}.{:03}", ms / 60_000, (ms % 60_000) / 1000, ms % 1000)
    }
}
