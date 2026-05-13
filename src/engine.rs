use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use evdev::{EventType, InputEvent, Key, RelativeAxisType};
use evdev::uinput::VirtualDeviceBuilder;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::{AdvancedEvent, FireMode, Macro, Profile, StepMode, TriggerMode};
use crate::config::store;
use crate::constants;

// ── Diagnostic log ────────────────────────────────────────────────────────────

static LOG_ENABLED: AtomicBool = AtomicBool::new(false);
static LOG_BUF: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn log_buf() -> &'static Mutex<VecDeque<String>> {
    LOG_BUF.get_or_init(|| Mutex::new(VecDeque::with_capacity(constants::ENGINE_LOG_CAPACITY)))
}

fn log_ms() -> u64 {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

macro_rules! engine_log {
    ($($arg:tt)*) => {
        if LOG_ENABLED.load(Ordering::Relaxed) {
            let msg = format!("[{:>8}ms] {}", log_ms(), format_args!($($arg)*));
            let mut buf = log_buf().lock().unwrap();
            if buf.len() >= constants::ENGINE_LOG_CAPACITY { buf.pop_front(); }
            buf.push_back(msg);
        }
    };
}

pub fn set_engine_logging(enabled: bool) {
    LOG_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn drain_engine_log() -> Vec<String> {
    log_buf().lock().unwrap().drain(..).collect()
}

pub fn clear_engine_log() {
    log_buf().lock().unwrap().clear();
}


struct LoopContext {
    pre_macros:   Vec<(crate::config::Macro, u32)>,         // (mac, delay_ms)
    cycle_macros: Vec<(crate::config::Macro, u32, u32)>,    // (mac, order, delay_ms)
    end_events:   Vec<(crate::config::Macro, u32, u32)>,    // (mac, min_loops, delay_ms)
    cycle_count:  Arc<AtomicU64>,
    stop:         Arc<AtomicBool>,
}

struct ActiveLoopEntry {
    join:         tokio::task::JoinHandle<()>,
    stop:         Arc<AtomicBool>,
    release_keys: Vec<Key>, // used only on forced-abort (session shutdown)
}

// ── Auto-detect event ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DetectEvent {
    /// A profile's auto-detect target matched a running process.
    Found(Uuid),
    /// The previously-detected process is no longer running.
    Lost,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// In-process remapping engine. Created once when the profile editor opens;
/// lives until the window closes. All remapping stops when this is dropped.
pub struct Engine {
    rt:        tokio::runtime::Runtime,
    state:     std::sync::Mutex<Option<ActiveSession>>,
    scanner:   std::sync::Mutex<Option<ScannerHandle>>,
    detect_rx: std::sync::Mutex<Option<std::sync::mpsc::Receiver<DetectEvent>>>,
}

struct ScannerHandle {
    cancel: Arc<AtomicBool>,
}

impl Engine {
    pub fn new() -> Self {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (dummy_tx, _) = std::sync::mpsc::channel::<crate::event_types::DisplayEvent>();
        rt.block_on(async {
            crate::drivers::start_supplemental_drivers(dummy_tx, std::time::Instant::now());
            tokio::time::sleep(std::time::Duration::from_millis(constants::DRIVER_STARTUP_WAIT_MS)).await;
        });
        Engine {
            rt,
            state:     std::sync::Mutex::new(None),
            scanner:   std::sync::Mutex::new(None),
            detect_rx: std::sync::Mutex::new(None),
        }
    }

    /// Load and activate a profile. Stops any previously active session first.
    pub fn load(&self, uuid: Uuid) -> Result<String, String> {
        let old = self.state.lock().unwrap().take();
        if let Some(session) = old {
            self.rt.block_on(stop_session(session));
        }
        match self.rt.block_on(start_session(uuid)) {
            Ok(session) => {
                let name = session.profile_name.clone();
                *self.state.lock().unwrap() = Some(session);
                Ok(name)
            }
            Err(e) => Err(e),
        }
    }

    /// Stop remapping; devices are ungrabbed and the engine returns to idle.
    pub fn stop(&self) {
        let old = self.state.lock().unwrap().take();
        if let Some(session) = old {
            self.rt.block_on(stop_session(session));
        }
    }

    pub fn is_active(&self) -> bool {
        self.state.lock().unwrap().is_some()
    }

    pub fn active_profile_name(&self) -> Option<String> {
        self.state.lock().unwrap().as_ref().map(|s| s.profile_name.clone())
    }

    pub fn active_profile_uuid(&self) -> Option<Uuid> {
        self.state.lock().unwrap().as_ref().map(|s| s.profile_id)
    }

    /// Start (or restart) the auto-detect scanner with the given profile list.
    pub fn start_auto_detect(&self, profiles: Vec<Profile>) {
        self.stop_auto_detect();
        let cancel = Arc::new(AtomicBool::new(true));
        let (tx, rx) = std::sync::mpsc::sync_channel::<DetectEvent>(1);
        let c = cancel.clone();
        std::thread::spawn(move || scanner_thread(profiles, tx, c));
        *self.scanner.lock().unwrap()   = Some(ScannerHandle { cancel });
        *self.detect_rx.lock().unwrap() = Some(rx);
    }

    /// Stop the auto-detect scanner if running.
    pub fn stop_auto_detect(&self) {
        if let Some(h) = self.scanner.lock().unwrap().take() {
            h.cancel.store(false, Ordering::Relaxed);
        }
        *self.detect_rx.lock().unwrap() = None;
    }

    /// Returns the next scanner event if one is pending.
    pub fn poll_detect(&self) -> Option<DetectEvent> {
        self.detect_rx.lock().unwrap().as_ref()?.try_recv().ok()
    }

    pub fn is_auto_detect_running(&self) -> bool {
        self.scanner.lock().unwrap().is_some()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_auto_detect();
        let old = self.state.lock().unwrap().take();
        if let Some(session) = old {
            self.rt.block_on(stop_session(session));
        }
    }
}

struct ActiveSession {
    profile_id:   Uuid,
    profile_name: String,
    /// Signals reader threads to exit.
    cancel:    Arc<AtomicBool>,
    /// Dup'd fds for each grabbed device — used to force EVIOCGRAB(0) on stop
    /// without waiting for the reader thread's next event.
    grabbed_fds: Vec<std::os::fd::OwnedFd>,
    /// Tokio task running the remap engine.
    processor: tokio::task::JoinHandle<()>,
    /// Send () to ask the processor to clean up and exit gracefully.
    stop_tx:   tokio::sync::oneshot::Sender<()>,
}

// ── Session lifecycle ─────────────────────────────────────────────────────────

async fn start_session(uuid: Uuid) -> Result<ActiveSession, String> {
    let profile = store::load_profile_by_id(uuid)
        .ok_or_else(|| format!("profile {uuid} not found"))?;

    // Resolve source key names → codes up front so device matching works even
    // when the evdev crate reports some codes as "unknown key: N".
    let source_key_codes: std::collections::HashSet<u16> = profile.assignments.iter()
        .map(|a| a.source_key.as_str())
        .chain(profile.modifiers.iter().map(|m| m.key.as_str()))
        .filter_map(parse_key)
        .map(|k| k.code())
        .collect();

    // Collect capabilities across all devices we'll grab.
    let mut vdev_keys  = evdev::AttributeSet::<Key>::new();
    let mut vdev_rel   = evdev::AttributeSet::<RelativeAxisType>::new();
    let mut has_keys   = false;
    let mut has_rel    = false;
    let mut devices_to_grab: Vec<evdev::Device> = Vec::new();

    for (_, device) in evdev::enumerate() {
        let relevant = device.supported_keys()
            .map(|s| s.iter().any(|k| source_key_codes.contains(&k.code())))
            .unwrap_or(false);
        if !relevant { continue; }

        if let Some(keys) = device.supported_keys() {
            for k in keys.iter() { vdev_keys.insert(k); has_keys = true; }
        }
        if let Some(axes) = device.supported_relative_axes() {
            for a in axes.iter() { vdev_rel.insert(a); has_rel = true; }
        }
        devices_to_grab.push(device);
    }

    if devices_to_grab.is_empty() {
        return Err("no matching devices found".into());
    }

    // Also declare all keys that macros and remaps might *inject* so the vdev
    // accepts those emit() calls (capabilities from the grabbed source device
    // only cover the source keys, not the output keys).
    for mac in &profile.macros {
        for step in &mac.simple_steps {
            if let Some(key) = parse_key(&step.action) {
                vdev_keys.insert(key);
                has_keys = true;
            }
        }
        for step in &mac.advanced_steps {
            if let Some(key) = parse_key(&step.key) {
                vdev_keys.insert(key);
                has_keys = true;
            }
        }
        // Also register keys used by event macros so the vdev can emit them.
        for ev in &mac.events {
            if let Some(ev_mac) = profile.macros.iter().find(|m| m.id == ev.macro_id) {
                for step in &ev_mac.simple_steps {
                    if let Some(key) = parse_key(&step.action) {
                        vdev_keys.insert(key);
                        has_keys = true;
                    }
                }
                for step in &ev_mac.advanced_steps {
                    if let Some(key) = parse_key(&step.key) {
                        vdev_keys.insert(key);
                        has_keys = true;
                    }
                }
            }
        }
    }
    for assignment in &profile.assignments {
        if let Some(remap_name) = &assignment.remap_key {
            for part in remap_name.split('+') {
                if let Some(key) = parse_key(part.trim()) {
                    vdev_keys.insert(key);
                    has_keys = true;
                }
            }
        }
    }

    // Build the virtual output device.
    let mut builder = VirtualDeviceBuilder::new()
        .map_err(|e| format!("uinput init: {e}"))?
        .name("gameremap");
    if has_keys {
        builder = builder.with_keys(&vdev_keys)
            .map_err(|e| format!("uinput keys: {e}"))?;
    }
    if has_rel {
        builder = builder.with_relative_axes(&vdev_rel)
            .map_err(|e| format!("uinput axes: {e}"))?;
    }
    let vdev = builder.build().map_err(|e| format!("uinput build: {e}"))?;
    let vdev = Arc::new(std::sync::Mutex::new(vdev));

    let cancel = Arc::new(AtomicBool::new(true));
    let (event_tx, event_rx) = mpsc::channel::<(InputEvent, String)>(constants::EVENT_CHANNEL_CAPACITY);
    let mut grabbed_fds: Vec<std::os::fd::OwnedFd> = Vec::new();

    for mut device in devices_to_grab {
        let device_name = device.name().unwrap_or("Unknown").to_string();
        if let Err(e) = device.grab() {
            eprintln!("engine: grab failed on {device_name:?}: {e}");
            continue;
        }
        // Dup the fd so stop_session() can release EVIOCGRAB immediately
        // without waiting for the reader thread to unblock from fetch_events().
        use std::os::{fd::OwnedFd, unix::io::{AsRawFd, FromRawFd}};
        let raw_dup = unsafe { libc::dup(device.as_raw_fd()) };
        if raw_dup >= 0 {
            grabbed_fds.push(unsafe { OwnedFd::from_raw_fd(raw_dup) });
        }
        let tx     = event_tx.clone();
        let cancel = cancel.clone();
        std::thread::spawn(move || reader_thread(device, device_name, tx, cancel));
    }
    drop(event_tx); // processor exits when all senders drop

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let profile_name = profile.name.clone();
    let processor = tokio::spawn(run_event_processor(event_rx, vdev, profile, stop_rx));

    Ok(ActiveSession { profile_id: uuid, profile_name, cancel, grabbed_fds, processor, stop_tx })
}

async fn stop_session(session: ActiveSession) {
    // Release EVIOCGRAB on all grabbed devices immediately via the dup'd fds.
    // This unblocks reader threads stuck in fetch_events() and lets other
    // applications receive input right away — no waiting for the next event.
    for fd in &session.grabbed_fds {
        force_ungrab(fd);
    }
    session.cancel.store(false, Ordering::Relaxed);
    // Ask the processor to release held keys and exit cleanly.
    let abort = session.processor.abort_handle();
    let _ = session.stop_tx.send(());
    if tokio::time::timeout(std::time::Duration::from_millis(constants::SESSION_SHUTDOWN_TIMEOUT_MS), session.processor)
        .await
        .is_err()
    {
        abort.abort();
    }
    // grabbed_fds drop here, closing the dup'd fds.
}

/// Call EVIOCGRAB(0) on a dup'd device fd to release exclusive grab immediately.
/// The dup shares the same kernel file description as the reader thread's fd,
/// so the grab is released without touching the reader thread at all.
fn force_ungrab(fd: &std::os::fd::OwnedFd) {
    use std::os::fd::AsRawFd;
    // EVIOCGRAB = _IOW('E', 0x90, int) = 0x40044590 on x86_64
    unsafe { libc::ioctl(fd.as_raw_fd(), 0x40044590u64, 0i32) };
}

// ── Reader thread (blocking evdev → async mpsc) ───────────────────────────────

fn reader_thread(
    mut device: evdev::Device,
    device_name: String,
    tx: mpsc::Sender<(InputEvent, String)>,
    cancel: Arc<AtomicBool>,
) {
    loop {
        if !cancel.load(Ordering::Relaxed) { break; }
        let events = match device.fetch_events() {
            Ok(ev) => ev.collect::<Vec<_>>(),
            Err(e) => { eprintln!("daemon reader: {e}"); break; }
        };
        for ev in events {
            if tx.blocking_send((ev, device_name.clone())).is_err() { return; }
        }
    }
    // Dropping `device` releases EVIOCGRAB automatically.
}

// ── Remap engine helpers ──────────────────────────────────────────────────────

/// Returns the unique set of keys that a loop macro presses (down events).
/// These are emitted as key-up when the macro is aborted, so nothing stays held.
fn macro_release_keys(mac: &crate::config::Macro) -> Vec<Key> {
    use crate::config::{AdvancedEvent, StepMode};
    let mut seen: HashSet<u16> = HashSet::new();
    let mut keys = Vec::new();
    let names: Vec<&str> = match mac.mode {
        StepMode::Simple => mac.simple_steps.iter().map(|s| s.action.as_str()).collect(),
        StepMode::Advanced => mac.advanced_steps.iter()
            .filter(|s| matches!(s.event, AdvancedEvent::KeyDown | AdvancedEvent::MouseDown))
            .map(|s| s.key.as_str())
            .collect(),
    };
    for name in names {
        if let Some(key) = parse_key(name) {
            if seen.insert(key.code()) {
                keys.push(key);
            }
        }
    }
    keys
}

// ── Remap engine ──────────────────────────────────────────────────────────────

async fn run_event_processor(
    mut rx: mpsc::Receiver<(InputEvent, String)>,
    vdev: Arc<std::sync::Mutex<evdev::uinput::VirtualDevice>>,
    profile: Profile,
    mut stop_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let find_mod = |key_name: &str, dev_name: &str| -> Option<Uuid> {
        profile.modifiers.iter().find(|m| {
            m.key == key_name
                && m.source_device.as_deref().map_or(true, |d| d == dev_name)
        }).map(|m| m.id)
    };

    let mut held_mods:    HashSet<Uuid> = HashSet::new();
    // source key code → remapped keys currently held down (may be a modifier+key compound)
    let mut active_remaps: HashMap<u16, Vec<Key>> = HashMap::new();
    // source key code → loop entry (abort handle, release keys, cycle count, end events)
    let mut active_loops: HashMap<u16, ActiveLoopEntry> = HashMap::new();
    // source key code → (press instant, device name, mods held at press) for QuickPress/ShortHold
    let mut pending_triggers: HashMap<u16, (std::time::Instant, String, HashSet<Uuid>)> = HashMap::new();
    // abort handle for the ESC-held-7s emergency-exit timer
    let mut esc_exit_task: Option<tokio::task::AbortHandle> = None;

    loop {
        let (event, device_name) = tokio::select! {
            biased;
            _ = &mut stop_rx => break,
            ev = rx.recv() => match ev { Some(e) => e, None => break },
        };

        use evdev::InputEventKind;

        match event.kind() {
            InputEventKind::Key(key) => {
                let name  = key_name(key);
                let code  = event.code();
                let value = event.value(); // 1=press 0=release 2=repeat

                // Emergency exit: ESC held for 7 seconds kills the daemon.
                // Fires regardless of profile bindings so the user can always escape.
                if key == Key::KEY_ESC {
                    if value == 1 {
                        let handle = tokio::spawn(async {
                            tokio::time::sleep(std::time::Duration::from_secs(constants::ESC_EXIT_HOLD_SECS)).await;
                            eprintln!("daemon: ESC held {} seconds — exiting", constants::ESC_EXIT_HOLD_SECS);
                            std::process::exit(0);
                        });
                        esc_exit_task = Some(handle.abort_handle());
                    } else if value == 0 {
                        if let Some(abort) = esc_exit_task.take() {
                            abort.abort();
                        }
                    }
                }

                if value == 0 {
                    // Release: cancel any active remap/loop unconditionally,
                    // regardless of current modifier state.
                    if let Some(mod_id) = find_mod(&name, &device_name) {
                        held_mods.remove(&mod_id);
                        continue;
                    }
                    // Deferred timed trigger: classify hold duration and fire if it matches.
                    if let Some((press_time, press_device, press_mods)) = pending_triggers.remove(&code) {
                        let elapsed_ms = press_time.elapsed().as_millis() as u64;
                        if let Some(assignment) = find_timed_assignment(&profile, &name, &press_mods, &press_device, elapsed_ms) {
                            if let Some(macro_id) = assignment.macro_id {
                                if let Some(mac) = profile.macros.iter().find(|m| m.id == macro_id) {
                                    if mac.fire == FireMode::Single {
                                        let sink: Arc<dyn KeySink> = Arc::new(VdevSink(vdev.clone()));
                                        tokio::spawn(run_macro(mac.clone(), sink, None));
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    if let Some(rkeys) = active_remaps.remove(&code) {
                        for rkey in rkeys.iter().rev() {
                            emit_key(&vdev, *rkey, 0);
                        }
                        continue;
                    }
                    if let Some(entry) = active_loops.remove(&code) {
                        // Signal the loop to stop after its current iteration.
                        // The loop task itself will then run end events serially
                        // before exiting — no separate spawning needed here.
                        entry.stop.store(true, Ordering::Relaxed);
                        let join = entry.join;
                        tokio::spawn(async move { let _ = join.await; });
                        continue;
                    }
                    // Passthrough release.
                    vdev.lock().unwrap().emit(&[event]).ok();
                    continue;
                }

                if value == 1 {
                    // Modifier key press.
                    if let Some(mod_id) = find_mod(&name, &device_name) {
                        held_mods.insert(mod_id);
                        continue;
                    }

                    // Immediate-fire assignment: remaps (any trigger_mode) or Any-mode macros.
                    if let Some(assignment) = find_assignment(&profile, &name, &held_mods, &device_name) {
                        if let Some(remap_name) = assignment.remap_key.clone() {
                            let rkeys: Vec<Key> = remap_name.split('+')
                                .filter_map(|part| parse_key(part.trim()))
                                .collect();
                            if !rkeys.is_empty() {
                                for &rkey in &rkeys { emit_key(&vdev, rkey, 1); }
                                active_remaps.insert(code, rkeys);
                            }
                        } else if let Some(macro_id) = assignment.macro_id {
                            if let Some(mac) = profile.macros.iter().find(|m| m.id == macro_id) {
                                let is_loop = mac.fire == FireMode::Loop;
                                let release_keys = if is_loop { macro_release_keys(mac) } else { Vec::new() };
                                let sink: Arc<dyn KeySink> = Arc::new(VdevSink(vdev.clone()));
                                if is_loop {
                                    let cycle_count = Arc::new(AtomicU64::new(0));
                                    let stop        = Arc::new(AtomicBool::new(false));
                                    let mut pre_macros   = Vec::new();
                                    let mut cycle_macros = Vec::new();
                                    let mut end_events   = Vec::new();
                                    for ev in &mac.events {
                                        let Some(ev_mac) = profile.macros.iter().find(|m| {
                                            m.id == ev.macro_id
                                                && m.fire == FireMode::Single
                                                && m.events.is_empty()
                                        }) else { continue };
                                        match ev.order {
                                            0  => pre_macros.push((ev_mac.clone(), ev.delay_ms)),
                                            -1 => end_events.push((ev_mac.clone(), ev.min_loops, ev.delay_ms)),
                                            n if n > 0 => cycle_macros.push((ev_mac.clone(), n as u32, ev.delay_ms)),
                                            _ => {}
                                        }
                                    }
                                    let loop_ctx = LoopContext {
                                        pre_macros,
                                        cycle_macros,
                                        end_events,
                                        cycle_count: cycle_count.clone(),
                                        stop: stop.clone(),
                                    };
                                    let handle = tokio::spawn(run_macro(mac.clone(), sink.clone(), Some(loop_ctx)));
                                    active_loops.insert(code, ActiveLoopEntry {
                                        join: handle,
                                        stop,
                                        release_keys,
                                    });
                                } else {
                                    tokio::spawn(run_macro(mac.clone(), sink, None));
                                }
                            }
                        }
                        continue;
                    }

                    // Deferred-trigger assignment (QuickPress / ShortHold): record press time.
                    if has_deferred_trigger(&profile, &name, &held_mods, &device_name) {
                        pending_triggers.insert(code, (std::time::Instant::now(), device_name.clone(), held_mods.clone()));
                        continue;
                    }

                    // Passthrough press.
                    vdev.lock().unwrap().emit(&[event]).ok();
                }
                // value == 2 (repeat): drop — uinput generates its own repeats.
            }

            // Forward mouse movement and sync events preserving the original batching.
            InputEventKind::RelAxis(_) | InputEventKind::Synchronization(_) => {
                vdev.lock().unwrap().emit(&[event]).ok();
            }

            _ => {} // EV_MSC, EV_ABS, etc. — skip for now
        }
    }

    // Graceful cleanup: abort running loop macros and release any held keys.
    for (_, entry) in active_loops.drain() {
        entry.join.abort();
        for key in entry.release_keys {
            emit_key(&vdev, key, 0);
        }
        // Do NOT fire end events on graceful stop (session end, not key release).
    }
    for (_, rkeys) in active_remaps.drain() {
        for rkey in rkeys.iter().rev() {
            emit_key(&vdev, *rkey, 0);
        }
    }
}

/// Returns the best immediately-fireable assignment on key press: any remap, or
/// a macro assignment with TriggerMode::Any.  QuickPress/ShortHold macros are
/// excluded here — they are deferred to key release via `pending_triggers`.
fn find_assignment<'a>(
    profile: &'a Profile,
    key_name: &str,
    held_mods: &HashSet<Uuid>,
    device_name: &str,
) -> Option<&'a crate::config::KeyAssignment> {
    profile.assignments.iter()
        .filter(|a| {
            a.source_key == key_name
                && a.modifiers.iter().all(|m| held_mods.contains(m))
                && a.source_device.as_deref().map_or(true, |d| d == device_name)
                // Remaps always fire immediately; macros only fire immediately on Any.
                && (a.remap_key.is_some() || a.trigger_mode == TriggerMode::Any)
        })
        .max_by_key(|a| (a.modifiers.len(), a.source_device.is_some() as usize))
}

/// Returns true if any QuickPress/ShortHold macro assignment matches this key press.
fn has_deferred_trigger(
    profile: &Profile,
    key_name: &str,
    held_mods: &HashSet<Uuid>,
    device_name: &str,
) -> bool {
    profile.assignments.iter().any(|a| {
        a.source_key == key_name
            && a.macro_id.is_some()
            && matches!(a.trigger_mode, TriggerMode::QuickPress | TriggerMode::ShortHold)
            && a.modifiers.iter().all(|m| held_mods.contains(m))
            && a.source_device.as_deref().map_or(true, |d| d == device_name)
    })
}

/// Finds the deferred macro assignment whose TriggerMode matches the hold duration.
fn find_timed_assignment<'a>(
    profile: &'a Profile,
    key_name: &str,
    held_mods: &HashSet<Uuid>,
    device_name: &str,
    elapsed_ms: u64,
) -> Option<&'a crate::config::KeyAssignment> {
    let trigger = if elapsed_ms < constants::QUICK_PRESS_MAX_MS {
        TriggerMode::QuickPress
    } else if elapsed_ms < constants::SHORT_HOLD_MAX_MS {
        TriggerMode::ShortHold
    } else {
        return None; // held too long — no match
    };
    profile.assignments.iter()
        .filter(|a| {
            a.source_key == key_name
                && a.macro_id.is_some()
                && a.trigger_mode == trigger
                && a.modifiers.iter().all(|m| held_mods.contains(m))
                && a.source_device.as_deref().map_or(true, |d| d == device_name)
        })
        .max_by_key(|a| (a.modifiers.len(), a.source_device.is_some() as usize))
}

// ── Key emission abstraction ──────────────────────────────────────────────────

/// Abstracts key emission so macros can run against a real uinput device or a
/// test recorder without any OS dependency.
trait KeySink: Send + Sync + 'static {
    fn emit_key(&self, key: Key, value: i32);
}

struct VdevSink(Arc<std::sync::Mutex<evdev::uinput::VirtualDevice>>);

impl KeySink for VdevSink {
    fn emit_key(&self, key: Key, value: i32) {
        self.0.lock().unwrap().emit(&[
            InputEvent::new(EventType::KEY, key.code(), value),
            InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
        ]).ok();
    }
}

/// Records emitted events with millisecond timestamps from creation.
/// Usable in tests and in a future `gameremap test-macro` subcommand.
pub(crate) struct RecordingSink {
    t0: std::time::Instant,
    events: std::sync::Mutex<Vec<(u64, Key, i32)>>,
}

impl RecordingSink {
    pub(crate) fn new() -> Self {
        Self { t0: std::time::Instant::now(), events: std::sync::Mutex::new(Vec::new()) }
    }

    pub(crate) fn snapshot(&self) -> Vec<(u64, Key, i32)> {
        self.events.lock().unwrap().clone()
    }
}

impl KeySink for RecordingSink {
    fn emit_key(&self, key: Key, value: i32) {
        let ms = self.t0.elapsed().as_millis() as u64;
        self.events.lock().unwrap().push((ms, key, value));
    }
}

// ── Macro execution ───────────────────────────────────────────────────────────

/// Executes a macro against the given sink.
/// For Loop macros, runs indefinitely until the task is aborted (key release).
fn run_macro(mac: Macro, sink: Arc<dyn KeySink>, loop_ctx: Option<LoopContext>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
    let is_loop = mac.fire == FireMode::Loop;

    if is_loop {
        if let Some(ref ctx) = loop_ctx {
            engine_log!("[{}] loop START  end={} pre={} cycle={}",
                mac.name,
                ctx.end_events.len(),
                ctx.pre_macros.len(),
                ctx.cycle_macros.len(),
            );
        }
    }

    if mac.start_delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(mac.start_delay_ms as u64)).await;
    }

    match mac.mode {
        StepMode::Simple => {
            'simple: loop {
                // Pre events — serial before each iteration body.
                if let Some(ref ctx) = loop_ctx {
                    for (pre, delay_ms) in &ctx.pre_macros {
                        if *delay_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(*delay_ms as u64)).await;
                        }
                        run_macro(pre.clone(), sink.clone(), None).await;
                    }
                }
                for step in &mac.simple_steps {
                    if let Some(key) = parse_key(&step.action) {
                        if !is_loop { engine_log!("[{}] step {} ↓", mac.name, step.action); }
                        sink.emit_key(key, 1);
                        if step.hold_ms > 0 {
                            tokio::time::sleep(
                                std::time::Duration::from_millis(step.hold_ms as u64)
                            ).await;
                        }
                        sink.emit_key(key, 0);
                        if !is_loop { engine_log!("[{}] step {} ↑", mac.name, step.action); }
                    }
                    if step.delay_after_ms > 0 {
                        tokio::time::sleep(
                            std::time::Duration::from_millis(step.delay_after_ms as u64)
                        ).await;
                    }
                }
                if !is_loop { break 'simple; }
                // Cycle events — serial after each complete iteration.
                if let Some(ref ctx) = loop_ctx {
                    let c = ctx.cycle_count.fetch_add(1, Ordering::Relaxed) + 1;
                    engine_log!("[{}] cycle {}", mac.name, c);
                    for (cyc_mac, order, delay_ms) in &ctx.cycle_macros {
                        if c % *order as u64 == 0 {
                            if *delay_ms > 0 {
                                tokio::time::sleep(std::time::Duration::from_millis(*delay_ms as u64)).await;
                            }
                            run_macro(cyc_mac.clone(), sink.clone(), None).await;
                        }
                    }
                    if ctx.stop.load(Ordering::Relaxed) {
                        engine_log!("[{}] stop at cycle {} (post-body)", mac.name, c);
                        if mac.loop_delay_ms > 0 {
                            tokio::time::sleep(
                                std::time::Duration::from_millis(mac.loop_delay_ms as u64)
                            ).await;
                        }
                        break 'simple;
                    }
                }
                if mac.loop_delay_ms > 0 {
                    tokio::time::sleep(
                        std::time::Duration::from_millis(mac.loop_delay_ms as u64)
                    ).await;
                }
                if let Some(ref ctx) = loop_ctx {
                    if ctx.stop.load(Ordering::Relaxed) {
                        engine_log!("[{}] stop at cycle {} (post-delay)",
                            mac.name, ctx.cycle_count.load(Ordering::Relaxed));
                        break 'simple;
                    }
                }
            }
            // End events — serial in this same task after the loop exits.
            if is_loop {
                if let Some(ref ctx) = loop_ctx {
                    let cycles = ctx.cycle_count.load(Ordering::Relaxed);
                    engine_log!("[{}] end-events: cycles={} candidates={}", mac.name, cycles, ctx.end_events.len());
                    for (end_mac, min_loops, delay_ms) in &ctx.end_events {
                        if cycles >= *min_loops as u64 {
                            engine_log!("[{}] → FIRE '{}' (cycles={} >= min_loops={}, delay={}ms)",
                                mac.name, end_mac.name, cycles, min_loops, delay_ms);
                            if *delay_ms > 0 {
                                tokio::time::sleep(std::time::Duration::from_millis(*delay_ms as u64)).await;
                            }
                            run_macro(end_mac.clone(), sink.clone(), None).await;
                        } else {
                            engine_log!("[{}] → SKIP '{}' (cycles={} < min_loops={})",
                                mac.name, end_mac.name, cycles, min_loops);
                        }
                    }
                }
            }
        }

        StepMode::Advanced => {
            'advanced: loop {
                // Pre events — serial before each iteration body.
                if let Some(ref ctx) = loop_ctx {
                    for (pre, delay_ms) in &ctx.pre_macros {
                        if *delay_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(*delay_ms as u64)).await;
                        }
                        run_macro(pre.clone(), sink.clone(), None).await;
                    }
                }
                let t0 = tokio::time::Instant::now();
                for step in &mac.advanced_steps {
                    let target  = std::time::Duration::from_millis(step.time_ms as u64);
                    let elapsed = t0.elapsed();
                    if target > elapsed {
                        tokio::time::sleep(target - elapsed).await;
                    }
                    if let Some(key) = parse_key(&step.key) {
                        let value = match step.event {
                            AdvancedEvent::KeyDown | AdvancedEvent::MouseDown => 1,
                            AdvancedEvent::KeyUp   | AdvancedEvent::MouseUp   => 0,
                        };
                        sink.emit_key(key, value);
                    }
                }
                if !is_loop { break 'advanced; }
                // Cycle events — serial after each complete iteration.
                if let Some(ref ctx) = loop_ctx {
                    let c = ctx.cycle_count.fetch_add(1, Ordering::Relaxed) + 1;
                    engine_log!("[{}] cycle {}", mac.name, c);
                    for (cyc_mac, order, delay_ms) in &ctx.cycle_macros {
                        if c % *order as u64 == 0 {
                            if *delay_ms > 0 {
                                tokio::time::sleep(std::time::Duration::from_millis(*delay_ms as u64)).await;
                            }
                            run_macro(cyc_mac.clone(), sink.clone(), None).await;
                        }
                    }
                    if ctx.stop.load(Ordering::Relaxed) {
                        engine_log!("[{}] stop at cycle {} (post-body)", mac.name, c);
                        if mac.loop_delay_ms > 0 {
                            tokio::time::sleep(
                                std::time::Duration::from_millis(mac.loop_delay_ms as u64)
                            ).await;
                        }
                        break 'advanced;
                    }
                }
                if mac.loop_delay_ms > 0 {
                    tokio::time::sleep(
                        std::time::Duration::from_millis(mac.loop_delay_ms as u64)
                    ).await;
                }
                if let Some(ref ctx) = loop_ctx {
                    if ctx.stop.load(Ordering::Relaxed) {
                        engine_log!("[{}] stop at cycle {} (post-delay)",
                            mac.name, ctx.cycle_count.load(Ordering::Relaxed));
                        break 'advanced;
                    }
                }
            }
            // End events — serial in this same task after the loop exits.
            if is_loop {
                if let Some(ref ctx) = loop_ctx {
                    let cycles = ctx.cycle_count.load(Ordering::Relaxed);
                    engine_log!("[{}] end-events: cycles={} candidates={}", mac.name, cycles, ctx.end_events.len());
                    for (end_mac, min_loops, delay_ms) in &ctx.end_events {
                        if cycles >= *min_loops as u64 {
                            engine_log!("[{}] → FIRE '{}' (cycles={} >= min_loops={}, delay={}ms)",
                                mac.name, end_mac.name, cycles, min_loops, delay_ms);
                            if *delay_ms > 0 {
                                tokio::time::sleep(std::time::Duration::from_millis(*delay_ms as u64)).await;
                            }
                            run_macro(end_mac.clone(), sink.clone(), None).await;
                        } else {
                            engine_log!("[{}] → SKIP '{}' (cycles={} < min_loops={})",
                                mac.name, end_mac.name, cycles, min_loops);
                        }
                    }
                }
            }
        }
    }
    })
}

// ── uinput helpers ────────────────────────────────────────────────────────────

fn emit_key(vdev: &Arc<std::sync::Mutex<evdev::uinput::VirtualDevice>>, key: Key, value: i32) {
    vdev.lock().unwrap().emit(&[
        InputEvent::new(EventType::KEY, key.code(), value),
        InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
    ]).ok();
}

/// Convert an evdev key name (e.g. "KEY_A") to a `Key` by scanning all codes.
/// Handles KEY_MACRO1–KEY_MACRO30 (0x290–0x2AD) explicitly because older evdev
/// crate versions report those as "unknown key: N" rather than their canonical name.
fn parse_key(name: &str) -> Option<Key> {
    static MAP: OnceLock<HashMap<String, Key>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for code in 0u16..0x300 {
            let key = Key::new(code);
            m.insert(format!("{key:?}"), key);
        }
        // KEY_MACRO1–KEY_MACRO30 may be reported as "unknown key: N" by older
        // evdev crate versions, so insert them explicitly under their canonical names.
        for idx in 1u16..=30 {
            m.insert(format!("KEY_MACRO{idx}"), Key::new(constants::KEY_MACRO_BASE + idx - 1));
        }
        m
    });
    map.get(name).copied()
}

/// Canonical string name for a key, the inverse of `parse_key`.
/// Fills in KEY_MACRO1–KEY_MACRO30 for evdev crate versions that report them as
/// "unknown key: N".
pub(crate) fn key_name(key: Key) -> String {
    let s = format!("{key:?}");
    if s.starts_with("unknown key:") {
        let code = key.code();
        if (constants::KEY_MACRO_BASE..=constants::KEY_MACRO_MAX).contains(&code) {
            return format!("KEY_MACRO{}", code - constants::KEY_MACRO_BASE + 1);
        }
    }
    s
}

// ── Auto-detect scanner ───────────────────────────────────────────────────────

/// Background thread with two modes:
///
/// **Search** (no active match) — scans /proc every 2 s.  On first match,
/// sends `Found(uuid)` and switches to Monitor mode.
///
/// **Monitor** (game running) — re-checks every 30 s.  If the game is gone,
/// sends `Lost` and switches back to Search mode.
///
/// Exits when `cancel` is set to false.
fn scanner_thread(profiles: Vec<Profile>, tx: std::sync::mpsc::SyncSender<DetectEvent>, cancel: Arc<AtomicBool>) {
    let mut monitoring: Option<Uuid> = None;

    loop {
        if !cancel.load(Ordering::Relaxed) { break; }

        let proc_strings = collect_proc_strings();

        match monitoring {
            Some(uuid) => {
                // Monitor: verify the game is still running.
                let alive = profiles.iter()
                    .find(|p| p.id == uuid)
                    .map(|p| profile_matches(p, &proc_strings))
                    .unwrap_or(false);

                if alive {
                    sleep_cancellable(constants::AUTODETECT_MONITOR_INTERVAL_MS, &cancel);
                } else {
                    monitoring = None;
                    let _ = tx.try_send(DetectEvent::Lost);
                    sleep_cancellable(constants::AUTODETECT_LOST_PAUSE_MS, &cancel);
                }
            }
            None => {
                // Search: find a match.
                if let Some(p) = profiles.iter().find(|p| profile_matches(p, &proc_strings)) {
                    monitoring = Some(p.id);
                    let _ = tx.try_send(DetectEvent::Found(p.id));
                    // No sleep — enter monitoring mode immediately on the next iteration.
                } else {
                    sleep_cancellable(constants::AUTODETECT_SEARCH_INTERVAL_MS, &cancel);
                }
            }
        }
    }
}

/// Returns true if any of `profile`'s auto-detect targets match `proc_strings`.
fn profile_matches(profile: &Profile, proc_strings: &[String]) -> bool {
    profile.auto_detect.iter().any(|target| {
        !target.fragments.is_empty()
            && target.fragments.iter().all(|frag| {
                let lower = frag.to_lowercase();
                proc_strings.iter().any(|s| s.contains(&lower))
            })
    })
}

/// Sleep `total_ms` in 100 ms increments, returning early if `cancel` is cleared.
fn sleep_cancellable(total_ms: u64, cancel: &Arc<AtomicBool>) {
    for _ in 0..(total_ms / 100) {
        if !cancel.load(Ordering::Relaxed) { return; }
        std::thread::sleep(std::time::Duration::from_millis(constants::AUTODETECT_SLEEP_GRANULE_MS));
    }
}

/// Read exe symlink and cmdline for every numeric /proc entry.
/// Returns a flat list of lowercased strings (one per exe path, one per cmdline).
fn collect_proc_strings() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return out };

    for entry in dir.flatten() {
        let name = entry.file_name();
        let pid  = name.to_string_lossy();
        if !pid.bytes().all(|b| b.is_ascii_digit()) { continue; }

        if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
            out.push(exe.to_string_lossy().to_lowercase());
        }

        if let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) {
            // cmdline is NUL-separated; replace NULs with spaces for substring matching.
            let s: String = bytes.iter().map(|&b| if b == 0 { ' ' } else { b as char }).collect();
            out.push(s.to_lowercase());
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AdvancedEvent, AdvancedStep, FireMode, Macro, SimpleStep, StepMode};

    fn make_loop_macro(steps: Vec<AdvancedStep>) -> Macro {
        Macro {
            id: uuid::Uuid::new_v4(),
            name: "test".into(),
            fire: FireMode::Loop,
            mode: StepMode::Advanced,
            start_delay_ms: 0,
            loop_delay_ms: 0,
            simple_steps: vec![],
            advanced_steps: steps,
            events: vec![],
        }
    }

    /// Run the macro for `window_ms` milliseconds and return the recorded events.
    /// Prints a human-readable timing table — pass `--nocapture` to see it:
    ///   cargo test macro_ -- --nocapture
    async fn run_for(mac: Macro, window_ms: u64) -> Vec<(u64, Key, i32)> {
        let sink = Arc::new(RecordingSink::new());
        let task = tokio::spawn(run_macro(mac, sink.clone(), None));
        tokio::time::sleep(std::time::Duration::from_millis(window_ms)).await;
        task.abort();
        let _ = task.await;
        let events = sink.snapshot();

        println!("\n  {:>6}  {:<12}  dir", "time", "key");
        println!("  ------  ------------  ---");
        for (ms, key, value) in &events {
            println!("  {:>5}ms  {:<12}  {}",
                ms, format!("{key:?}"), if *value == 1 { "DOWN" } else { "UP" });
        }
        events
    }

    #[tokio::test]
    async fn macro_lasgun_loop_timing() {
        // Current darktide profile: DOWN at 25 ms, UP at 800 ms → 775 ms hold, 25 ms gap.
        let mac = make_loop_macro(vec![
            AdvancedStep { event: AdvancedEvent::MouseDown, key: "BTN_LEFT".into(), time_ms: 25 },
            AdvancedStep { event: AdvancedEvent::MouseUp,   key: "BTN_LEFT".into(), time_ms: 800 },
        ]);

        // 3 full cycles take ~2400 ms; run for 2600 ms to capture 3 complete iterations.
        let events = run_for(mac, 2600).await;

        // Expect at least 3 complete DOWN/UP pairs.
        assert!(events.len() >= 6, "too few events: got {}", events.len());

        let downs: Vec<u64> = events.iter().filter(|e| e.2 == 1).map(|e| e.0).collect();
        let ups:   Vec<u64> = events.iter().filter(|e| e.2 == 0).map(|e| e.0).collect();

        print_cycle_table(&downs, &ups);

        // Every hold must be within ±50 ms of the configured 775 ms.
        for (&d, &u) in downs.iter().zip(ups.iter()) {
            let hold = u.saturating_sub(d);
            assert!((725..=825).contains(&hold),
                "hold {hold}ms outside [725, 825]ms — iteration timing is inconsistent");
        }
    }

    fn print_cycle_table(downs: &[u64], ups: &[u64]) {
        println!("\n  iter  hold(ms)  gap_to_next(ms)");
        println!("  ----  --------  ---------------");
        for i in 0..downs.len().min(ups.len()) {
            let hold = ups[i].saturating_sub(downs[i]);
            let gap = if i + 1 < downs.len() {
                format!("{}", downs[i + 1].saturating_sub(ups[i]))
            } else {
                "(last)".into()
            };
            println!("  {:>4}  {:>8}  {}", i + 1, hold, gap);
        }
    }

    fn make_simple_loop_macro(steps: Vec<SimpleStep>) -> Macro {
        Macro {
            id: uuid::Uuid::new_v4(),
            name: "test".into(),
            fire: FireMode::Loop,
            mode: StepMode::Simple,
            start_delay_ms: 0,
            loop_delay_ms: 0,
            simple_steps: steps,
            advanced_steps: vec![],
            events: vec![],
        }
    }

    #[tokio::test]
    async fn macro_fast_fire_timing() {
        // Fast Fire: BTN_LEFT, hold_ms=0, delay_after_ms=5 → instant DOWN/UP, 5ms gap.
        let mac = make_simple_loop_macro(vec![
            SimpleStep { action: "BTN_LEFT".into(), hold_ms: 0, delay_after_ms: 5 },
        ]);

        // 5ms/cycle → ~40 cycles in 200ms; run for 200ms.
        let events = run_for(mac, 200).await;

        assert!(events.len() >= 20, "too few events: got {} (expected ~40 DOWN/UP pairs in 200ms)", events.len());

        let downs: Vec<u64> = events.iter().filter(|e| e.2 == 1).map(|e| e.0).collect();
        let ups:   Vec<u64> = events.iter().filter(|e| e.2 == 0).map(|e| e.0).collect();

        print_cycle_table(&downs, &ups);

        // hold_ms=0 → DOWN/UP synchronous, hold should be <10ms.
        for (&d, &u) in downs.iter().zip(ups.iter()) {
            let hold = u.saturating_sub(d);
            assert!(hold < 10, "hold {hold}ms unexpectedly long — hold_ms=0 should be near-instant");
        }

        // Gap between UP and next DOWN should be close to 5ms (allow up to 30ms for scheduler jitter).
        for i in 0..ups.len().saturating_sub(1) {
            let gap = downs[i + 1].saturating_sub(ups[i]);
            assert!(gap <= 30, "gap {gap}ms between cycle {i} and {} is too large", i + 1);
        }
    }

    #[tokio::test]
    async fn macro_fast_fire_with_hold() {
        // Same as Fast Fire but with hold_ms=30: each click holds for 30ms before release.
        let mac = make_simple_loop_macro(vec![
            SimpleStep { action: "BTN_LEFT".into(), hold_ms: 30, delay_after_ms: 5 },
        ]);

        // 35ms/cycle → ~8 cycles in 300ms; run for 300ms.
        let events = run_for(mac, 300).await;

        assert!(events.len() >= 10, "too few events: got {}", events.len());

        let downs: Vec<u64> = events.iter().filter(|e| e.2 == 1).map(|e| e.0).collect();
        let ups:   Vec<u64> = events.iter().filter(|e| e.2 == 0).map(|e| e.0).collect();

        print_cycle_table(&downs, &ups);

        // hold_ms=30 → each hold should be within ±15ms of 30ms.
        for (&d, &u) in downs.iter().zip(ups.iter()) {
            let hold = u.saturating_sub(d);
            assert!((15..=45).contains(&hold),
                "hold {hold}ms outside [15, 45]ms — expected ~30ms hold");
        }
    }
}
