//! The render engine: a background thread that owns the keyboard.
//!
//! Device writes block for ~14 ms each, so they must never run on the UI
//! thread. The GUI sends commands in and reads a published snapshot out; it
//! never touches the HID handle.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use aula_effects::registry::{Registry, Source};
use aula_effects::{EffectMeta, Params, RenderCtx};
use aula_protocol::f75::keymap;
use aula_protocol::f75::protocol as p;
use aula_protocol::transport::{self, Choice};
use aula_protocol::Keyboard;
use aula_protocol::{ChannelOrder, DeviceId, Frame, KeyPos, Link, RgbDevice, ScanOptions};

/// What the GUI asks the engine to do.
pub enum Cmd {
    SelectEffect(usize),
    /// Whole parameter set, so UI and engine cannot drift apart.
    SetParams(Params),
    SetRunning(bool),
    /// Cap the write rate below the hardware ceiling.
    SetMaxFps(u32),
    /// Re-scan the effects directory now.
    Rescan,
    /// Drive this device, or `None` for automatic (wired preferred).
    SelectDevice(Option<DeviceId>),
    /// Re-enumerate devices now. `deep` also probes unrecognised hardware and
    /// is only ever sent because the user asked for it.
    ScanDevices {
        deep: bool,
    },
    Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeviceStatus {
    Connected {
        name: String,
        path: String,
        id: DeviceId,
        /// False when nothing has confirmed this device, so lighting works but
        /// a mode change is refused.
        can_change_mode: bool,
    },
    /// The user pinned a device that is not plugged in.
    ///
    /// Kept apart from a plain failure because it is not one: the app is doing
    /// exactly what it was told, and the UI can offer a way out rather than an
    /// error message.
    Waiting {
        pinned: DeviceId,
        label: String,
    },
    Disconnected(String),
}

/// One row in the device picker.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceEntry {
    pub id: DeviceId,
    pub label: String,
    pub link: Link,
    pub confirmed: bool,
    /// False for a pinned device that has gone away. The row stays so the pin
    /// is visible, rather than silently vanishing from the list.
    pub present: bool,
}

/// One entry in the effect list, as the UI needs it.
#[derive(Clone)]
pub struct EffectInfo {
    pub meta: EffectMeta,
    pub is_script: bool,
    pub is_animation: bool,
    pub file: Option<String>,
}

/// Published state. The GUI reads this every repaint.
pub struct Shared {
    pub frame: Frame,
    pub layout: Vec<KeyPos>,
    pub fps: f32,
    pub status: DeviceStatus,
    pub effects: Vec<EffectInfo>,
    pub selected: usize,
    /// Script load failures, newest last.
    pub errors: Vec<String>,
    /// Why the *selected* effect's last frame failed, if it did. Kept apart
    /// from `errors` because it clears itself the moment the script renders
    /// again, where a load failure persists until the file is fixed.
    pub script_error: Option<String>,
    pub running: bool,
    pub frames_sent: u64,
    /// Devices the last scan found, for the picker.
    pub devices: Vec<DeviceEntry>,
    pub pinned: Option<DeviceId>,
    /// A scan is in flight; the picker shows a spinner.
    pub scanning: bool,
    /// The connected device's own frame-rate ceiling. The wireless link is
    /// slower than the wired one, and a slider offering a rate the device will
    /// silently clamp looks like a bug.
    pub max_fps_ceiling: u32,
}

impl Shared {
    fn new() -> Self {
        Self {
            frame: Frame::black(126),
            layout: keymap::layout(),
            fps: 0.0,
            status: DeviceStatus::Disconnected("starting…".into()),
            effects: Vec::new(),
            selected: 0,
            errors: Vec::new(),
            script_error: None,
            running: true,
            frames_sent: 0,
            devices: Vec::new(),
            pinned: None,
            scanning: false,
            max_fps_ceiling: p::MAX_FPS,
        }
    }
}

pub struct Engine {
    pub shared: Arc<Mutex<Shared>>,
    tx: Sender<Cmd>,
}

impl Engine {
    /// Spawn the engine thread.
    pub fn spawn(
        effects_dir: std::path::PathBuf,
        pinned: Option<DeviceId>,
        allow: Vec<DeviceId>,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let mut initial = Shared::new();
        initial.pinned = pinned;
        let shared = Arc::new(Mutex::new(initial));
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);

        std::thread::Builder::new()
            .name("aula-render".into())
            .spawn(move || run(worker_shared, rx, effects_dir, pinned, allow, repaint))
            .expect("spawn render thread");

        Self { shared, tx }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// A sender for code that must reach the engine from another thread, such
    /// as the tray reader threads.
    pub fn sender(&self) -> Sender<Cmd> {
        self.tx.clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

fn run(
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<Cmd>,
    effects_dir: std::path::PathBuf,
    mut pinned: Option<DeviceId>,
    allow: Vec<DeviceId>,
    repaint: impl Fn() + Send,
) {
    let anim_dir = effects_dir
        .parent()
        .map(|p| p.join("animations"))
        .unwrap_or_else(|| std::path::PathBuf::from("animations"));
    let mut reg = Registry::with_all(&effects_dir, &anim_dir);
    publish_effects(&shared, &reg);

    let mut device: Option<Keyboard> = None;
    let mut last_open_attempt: Option<Instant> = None;
    let mut selected = 0usize;
    let mut params = current_params(&reg, selected);
    let mut effect_start = Instant::now();
    let mut running = true;
    let mut max_fps = p::MAX_FPS;

    let mut fps_window = Instant::now();
    let mut fps_frames = 0u32;
    let mut last_rescan = Instant::now();

    let mut opts = ScanOptions {
        allow,
        ..Default::default()
    };
    let mut candidates = Vec::new();
    // Enumerating is cheap; probing means opening handles, so it only happens
    // when the set of plugged-in HID devices has actually changed.
    let mut last_enum: Option<Vec<String>> = None;
    // A radio link times out the occasional write where a cable never would.
    // Dropping to "disconnected" on the first one would strobe the status bar
    // and thrash the reconnect loop.
    const WRITE_FAILURES_BEFORE_DISCONNECT: u32 = 3;
    let mut write_failures = 0u32;

    loop {
        // ---- commands ----
        loop {
            match rx.try_recv() {
                Ok(Cmd::Shutdown) | Err(TryRecvError::Disconnected) => return,
                Ok(Cmd::SelectEffect(i)) => {
                    if i < reg.len() {
                        selected = i;
                        params = current_params(&reg, selected);
                        effect_start = Instant::now();
                        let mut s = shared.lock().unwrap();
                        s.selected = selected;
                    }
                }
                Ok(Cmd::SetParams(p)) => params = p,
                Ok(Cmd::SetRunning(r)) => {
                    running = r;
                    shared.lock().unwrap().running = r;
                }
                Ok(Cmd::SetMaxFps(f)) => {
                    max_fps = f;
                    if let Some(kb) = device.as_mut() {
                        kb.set_max_fps(max_fps);
                    }
                }
                Ok(Cmd::SelectDevice(p)) => {
                    pinned = p;
                    if let Some(id) = p {
                        if !opts.allow.contains(&id) {
                            opts.allow.push(id);
                        }
                    }
                    // Drop the handle so the next pass reconnects to whatever
                    // was just asked for, and force a fresh probe.
                    device = None;
                    last_enum = None;
                    last_open_attempt = None;
                    shared.lock().unwrap().pinned = pinned;
                }
                Ok(Cmd::ScanDevices { deep }) => {
                    shared.lock().unwrap().scanning = true;
                    repaint();
                    let wide = ScanOptions {
                        allow: opts.allow.clone(),
                        deep,
                        include_non_keyboards: false,
                    };
                    // Runs on this thread, not the UI thread: a deep scan opens
                    // handles and can take a noticeable moment.
                    candidates = Keyboard::discover(&wide).unwrap_or_default();
                    last_enum = transport::enumeration_signature().ok();
                    publish_devices(&shared, &candidates, pinned);
                    let mut s = shared.lock().unwrap();
                    s.scanning = false;
                }
                Ok(Cmd::Rescan) => {
                    reg = Registry::with_all(&effects_dir, &anim_dir);
                    selected = selected.min(reg.len().saturating_sub(1));
                    params = current_params(&reg, selected);
                    publish_effects(&shared, &reg);
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        // ---- hot reload: pick up edited scripts ----
        if last_rescan.elapsed() > Duration::from_millis(750) {
            last_rescan = Instant::now();
            let before = reg.len();
            if reg.refresh() {
                if reg.len() != before {
                    selected = selected.min(reg.len().saturating_sub(1));
                }
                params = merge_params(&reg, selected, params);
                publish_effects(&shared, &reg);
            }
        }

        // ---- device ----
        if device.is_none() {
            let due = last_open_attempt
                .map(|t| t.elapsed() > Duration::from_secs(2))
                .unwrap_or(true);
            if due {
                last_open_attempt = Some(Instant::now());

                // Re-probe only when something was plugged or unplugged.
                let sig = transport::enumeration_signature().ok();
                if sig != last_enum {
                    last_enum = sig;
                    candidates = Keyboard::discover(&opts).unwrap_or_default();
                    publish_devices(&shared, &candidates, pinned);
                }

                match transport::choose(&candidates, pinned) {
                    Choice::Use(i) => {
                        let cand = candidates[i].clone();
                        match Keyboard::open_candidate(&cand, ChannelOrder::Rgb) {
                            Ok(mut kb) => {
                                kb.set_max_fps(max_fps);
                                // Once, before any streaming.
                                let mode = kb.ensure_per_key_mode();
                                let mut s = shared.lock().unwrap();
                                match mode {
                                    Ok(_) => {
                                        s.status = DeviceStatus::Connected {
                                            name: kb.name().to_string(),
                                            path: kb.hid_path().to_string(),
                                            id: kb.id(),
                                            can_change_mode: kb.can_change_mode(),
                                        };
                                        s.layout = kb.layout().to_vec();
                                        s.frame = Frame::black(kb.led_count());
                                        s.max_fps_ceiling = kb.max_fps();
                                        drop(s);
                                        write_failures = 0;
                                        device = Some(kb);
                                    }
                                    Err(e) => {
                                        s.status = DeviceStatus::Disconnected(e.to_string());
                                    }
                                }
                            }
                            Err(e) => {
                                // The path went stale between enumerating and
                                // opening; a fresh probe is the fix.
                                last_enum = None;
                                shared.lock().unwrap().status =
                                    DeviceStatus::Disconnected(e.to_string());
                            }
                        }
                    }
                    // A pin is never silently substituted. Lighting the wired
                    // board because the pinned receiver vanished would look
                    // like the app ignoring the setting.
                    Choice::PinnedMissing(id) => {
                        let label = shared
                            .lock()
                            .unwrap()
                            .devices
                            .iter()
                            .find(|d| d.id == id)
                            .map(|d| d.label.clone())
                            .unwrap_or_else(|| id.to_string());
                        shared.lock().unwrap().status = DeviceStatus::Waiting { pinned: id, label };
                    }
                    Choice::None => {
                        let e = aula_protocol::Error::NotFound {
                            searched: p::KNOWN.len()
                                + aula_protocol::dongle::KNOWN.len()
                                + opts.allow.len(),
                        };
                        shared.lock().unwrap().status = DeviceStatus::Disconnected(e.to_string());
                    }
                }
            }
        }

        let Some(kb) = device.as_mut() else {
            std::thread::sleep(Duration::from_millis(200));
            repaint();
            continue;
        };

        // Paused means paused: no rendering and no writes, so the board holds
        // its last frame and the keyboard gets the bus entirely to itself.
        // Blanking it and then streaming black would be all of the cost and
        // none of the point.
        if !running {
            {
                let mut s = shared.lock().unwrap();
                s.fps = 0.0;
            }
            std::thread::sleep(Duration::from_millis(60));
            repaint();
            continue;
        }

        // ---- render one frame ----
        let layout = kb.layout().to_vec();
        let mut frame = Frame::black(kb.led_count());

        if !reg.is_empty() {
            let ctx = RenderCtx {
                t: effect_start.elapsed().as_secs_f32(),
                layout: &layout,
                max_x: keymap::max_x(&layout),
                max_row: f32::from(keymap::max_row(&layout)),
                params: &params,
            };
            reg.entries[selected].effect_mut().render(&ctx, &mut frame);
        }

        // A script that throws leaves the previous frame up, which on its own
        // looks like the app has quietly frozen. Publish the reason.
        let script_error = reg
            .entries
            .get(selected)
            .and_then(|e| e.runtime_error())
            .map(str::to_string);
        {
            let mut s = shared.lock().unwrap();
            if s.script_error != script_error {
                s.script_error = script_error;
            }
        }

        match kb.stream(&frame) {
            Ok(()) => {
                fps_frames += 1;
                write_failures = 0;
                let mut s = shared.lock().unwrap();
                s.frame = frame;
                s.frames_sent += 1;
                if fps_window.elapsed() >= Duration::from_secs(1) {
                    s.fps = fps_frames as f32 / fps_window.elapsed().as_secs_f32();
                    fps_frames = 0;
                    fps_window = Instant::now();
                }
            }
            Err(e) => {
                // Usually unplugged — but over the radio it can equally be one
                // timed-out write, so give it a couple of chances before
                // tearing the connection down and telling the user it is gone.
                write_failures += 1;
                if write_failures >= WRITE_FAILURES_BEFORE_DISCONNECT {
                    let mut s = shared.lock().unwrap();
                    s.status = DeviceStatus::Disconnected(e.to_string());
                    s.fps = 0.0;
                    drop(s);
                    device = None;
                    write_failures = 0;
                    // Whatever is plugged in may have changed, so the next
                    // open attempt must probe rather than reuse the old list.
                    last_enum = None;
                }
            }
        }

        repaint();

        // stream() already enforces the hardware minimum gap; this just avoids
        // spinning when the effect renders faster than the device can accept.
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Publish the picker's device list.
///
/// A pinned device that is not plugged in keeps its row, marked absent: a pin
/// that silently disappears from the list looks like the app forgetting it.
fn publish_devices(
    shared: &Arc<Mutex<Shared>>,
    candidates: &[aula_protocol::DeviceCandidate],
    pinned: Option<DeviceId>,
) {
    let mut devices: Vec<DeviceEntry> = candidates
        .iter()
        .map(|c| DeviceEntry {
            id: c.id,
            label: c.label(),
            link: c.link,
            confirmed: c.confirmed,
            present: true,
        })
        .collect();

    if let Some(id) = pinned {
        if !devices.iter().any(|d| d.id == id) {
            devices.push(DeviceEntry {
                id,
                label: format!("{id}"),
                link: Link::Dongle,
                confirmed: false,
                present: false,
            });
        }
    }

    let mut s = shared.lock().unwrap();
    s.devices = devices;
    s.pinned = pinned;
}

fn publish_effects(shared: &Arc<Mutex<Shared>>, reg: &Registry) {
    let effects = reg
        .entries
        .iter()
        .map(|e| EffectInfo {
            meta: e.meta.clone(),
            is_script: matches!(e.source, Source::Script(_)),
            is_animation: matches!(e.source, Source::Animation(_)),
            file: match &e.source {
                Source::Script(p) | Source::Animation(p) => Some(
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                ),
                Source::Builtin => None,
            },
        })
        .collect();

    let mut s = shared.lock().unwrap();
    s.effects = effects;
    s.errors = reg.errors.clone();
}

fn current_params(reg: &Registry, idx: usize) -> Params {
    reg.entries
        .get(idx)
        .map(|e| Params::from_specs(&e.meta.params))
        .unwrap_or_default()
}

/// After a hot reload, keep values the user has already set where the parameter
/// still exists, so editing a script does not reset every slider.
fn merge_params(reg: &Registry, idx: usize, old: Params) -> Params {
    let Some(entry) = reg.entries.get(idx) else {
        return Params::default();
    };
    let mut fresh = Params::from_specs(&entry.meta.params);
    for spec in &entry.meta.params {
        if let Some(v) = old.get(&spec.id) {
            fresh.set(&spec.id, v.clone());
        }
    }
    fresh
}
