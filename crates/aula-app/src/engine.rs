//! The render engine: a background thread that owns the keyboard.
//!
//! Device writes block for ~14 ms each, so they must never run on the UI
//! thread. The GUI sends commands in and reads a published snapshot out; it
//! never touches the HID handle.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::backend::{self, Candidate, Device};
use crate::profiles::{self, AppLighting, Foreground, ProfileStatus, Selection};
use aula_effects::registry::{Registry, Source};
use aula_effects::{EffectMeta, Params, RenderCtx};
use aula_protocol::f75::keymap;
use aula_protocol::f75::protocol as p;
use aula_protocol::{DeviceId, Frame, KeyPos, Link, RgbDevice, ScanOptions};

/// What the GUI asks the engine to do.
pub enum Cmd {
    SelectEffect(usize),
    /// Whole parameter set, so UI and engine cannot drift apart.
    SetParams {
        revision: u64,
        params: Params,
    },
    ConfigureAppLighting(AppLighting),
    SetRunning(bool),
    /// Cap the write rate below the hardware ceiling.
    SetMaxFps(u32),
    /// Re-scan the effects directory now.
    Rescan,
    /// Stream this exact frame instead of the selected effect — the timeline
    /// editor's "send to keyboard" preview. `None` hands control back to the
    /// effect engine.
    LivePreview(Option<(String, Frame)>),
    /// Drive this device, or `None` for automatic (wired preferred).
    SelectDevice(Option<String>),
    ConfigureOpenRgb(Result<Option<std::net::SocketAddr>, String>),
    /// Re-enumerate devices now. `deep` also probes unrecognised hardware and
    /// is only ever sent because the user asked for it.
    ScanDevices {
        deep: bool,
    },
    Shutdown,
    WriteToNvram {
        target: String,
        frame: Frame,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeviceStatus {
    Connected {
        name: String,
        path: String,
        id: String,
        can_save: bool,
        has_matrix: bool,
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
        pinned: String,
        label: String,
    },
    Disconnected(String),
}

/// One row in the device picker.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceEntry {
    pub id: String,
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
    pub is_composition: bool,
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
    pub params: Params,
    pub selection_revision: u64,
    pub profile_status: ProfileStatus,
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
    pub pinned: Option<String>,
    pub discovery_warnings: Vec<String>,
    /// A scan is in flight; the picker shows a spinner.
    pub scanning: bool,
    /// The connected device's own frame-rate ceiling. The wireless link is
    /// slower than the wired one, and a slider offering a rate the device will
    /// silently clamp looks like a bug.
    pub max_fps_ceiling: u32,
    /// Which link the connected device is on, or `None` when disconnected. The
    /// UI uses this to restrict wireless to solid colours: the radio cannot keep
    /// up with full-board animation, so animations are blocked over the dongle.
    pub link: Option<Link>,
    pub last_nvram_result: Option<Result<(), String>>,
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
            params: Params::default(),
            selection_revision: 0,
            profile_status: ProfileStatus::default(),
            errors: Vec::new(),
            script_error: None,
            running: true,
            frames_sent: 0,
            devices: Vec::new(),
            pinned: None,
            discovery_warnings: Vec::new(),
            scanning: false,
            max_fps_ceiling: p::MAX_FPS,
            link: None,
            last_nvram_result: None,
        }
    }
}

pub struct Engine {
    pub shared: Arc<Mutex<Shared>>,
    tx: Sender<Cmd>,
}

struct WorkerConfig {
    effects_dir: std::path::PathBuf,
    pinned: Option<String>,
    allow: Vec<DeviceId>,
    endpoint: Result<Option<std::net::SocketAddr>, String>,
    app_lighting: AppLighting,
}

impl Engine {
    /// Spawn the engine thread.
    pub fn spawn(
        effects_dir: std::path::PathBuf,
        pinned: Option<String>,
        allow: Vec<DeviceId>,
        endpoint: Result<Option<std::net::SocketAddr>, String>,
        app_lighting: AppLighting,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let mut initial = Shared::new();
        initial.pinned = pinned.clone();
        let shared = Arc::new(Mutex::new(initial));
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);

        std::thread::Builder::new()
            .name("aula-render".into())
            .spawn(move || {
                run(
                    worker_shared,
                    rx,
                    WorkerConfig {
                        effects_dir,
                        pinned,
                        allow,
                        endpoint,
                        app_lighting,
                    },
                    repaint,
                )
            })
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
    config: WorkerConfig,
    repaint: impl Fn() + Send,
) {
    let WorkerConfig {
        effects_dir,
        mut pinned,
        allow,
        mut endpoint,
        mut app_lighting,
    } = config;
    let anim_dir = effects_dir
        .parent()
        .map(|p| p.join("animations"))
        .unwrap_or_else(|| std::path::PathBuf::from("animations"));
    let mut reg = Registry::with_all(&effects_dir, &anim_dir);
    publish_effects(&shared, &reg);

    let mut device: Option<Device> = None;
    let mut last_open_attempt: Option<Instant> = None;
    let mut selected = 0usize;
    let mut params = current_params(&reg, selected);
    if let Some(i) = reg.find(&app_lighting.default.effect) {
        selected = i;
        params = app_lighting.default.restore(&reg.entries[i].meta.params);
    }
    publish_selection(&shared, selected, &params);
    let mut applied_profile: Option<Selection> = None;
    let mut last_foreground_poll: Option<Instant> = None;
    let mut restore_default = false;
    let mut foreground_detector = profiles::ForegroundDetector::default();
    let mut effect_start = Instant::now();
    let mut running = true;
    let mut max_fps = p::MAX_FPS;
    // When set, the editor is driving the board directly; the effect engine
    // stands aside until it clears.
    let mut live_preview: Option<Frame> = None;

    let mut fps_window = Instant::now();
    let mut fps_frames = 0u32;
    let mut last_rescan = Instant::now();

    let mut opts = ScanOptions {
        allow,
        ..Default::default()
    };
    let mut discovery_cache = backend::DiscoveryCache::default();
    // Enumerating is cheap; probing means opening handles, so it only happens
    // when the set of plugged-in HID devices has actually changed.
    let mut needs_scan = true;
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
                        publish_selection(&shared, selected, &params);
                        applied_profile = None;
                    }
                }
                Ok(Cmd::SetParams {
                    revision,
                    params: p,
                }) => {
                    // A focus switch can happen while the UI is drawing. Never
                    // write old sliders into the newly activated profile.
                    if shared.lock().unwrap().selection_revision == revision {
                        params = p;
                        publish_selection(&shared, selected, &params);
                        applied_profile = None;
                    }
                }
                Ok(Cmd::ConfigureAppLighting(config)) => {
                    restore_default = !config.enabled;
                    app_lighting = config;
                    applied_profile = None;
                    last_foreground_poll = None;
                }
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
                    if let Some(id) = pinned.as_ref().and_then(|p| p.parse::<DeviceId>().ok()) {
                        if !opts.allow.contains(&id) {
                            opts.allow.push(id);
                        }
                    }
                    // Drop the handle so the next pass reconnects to whatever
                    // was just asked for, and force a fresh probe.
                    device = None;
                    needs_scan = true;
                    last_open_attempt = None;
                    live_preview = None;
                    shared.lock().unwrap().pinned = pinned.clone();
                }
                Ok(Cmd::ConfigureOpenRgb(value)) => {
                    endpoint = value;
                    device = None;
                    live_preview = None;
                    needs_scan = true;
                    last_open_attempt = None;
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
                    let found = discover(&mut discovery_cache, &wide, &endpoint, true);
                    let candidates = found.candidates;
                    shared.lock().unwrap().discovery_warnings = found.warnings;
                    publish_devices(&shared, &candidates, pinned.as_deref());
                    needs_scan = false;
                    let mut s = shared.lock().unwrap();
                    s.scanning = false;
                }
                Ok(Cmd::Rescan) => {
                    let id = reg.entries.get(selected).map(|e| e.meta.id.clone());
                    reg = Registry::with_all(&effects_dir, &anim_dir);
                    selected = id.as_deref().and_then(|id| reg.find(id)).unwrap_or(0);
                    params = merge_params(&reg, selected, params);
                    publish_effects(&shared, &reg);
                    publish_selection(&shared, selected, &params);
                    applied_profile = None;
                }
                Ok(Cmd::LivePreview(frame)) => {
                    live_preview = frame.and_then(|(target, frame)| {
                        device
                            .as_ref()
                            .filter(|kb| kb.id() == target)
                            .map(|_| frame)
                    });
                }
                Ok(Cmd::WriteToNvram { target, frame }) => {
                    let result = match device.as_mut() {
                        Some(kb) if kb.id() != target => Err(
                            "The selected keyboard changed; try again on the intended keyboard"
                                .into(),
                        ),
                        Some(kb) if frame.len() != kb.led_count() => {
                            Err("The frame belongs to a different keyboard layout".into())
                        }
                        Some(kb) => kb.set_static(&frame).map_err(|e| e.to_string()),
                        None => Err("No device connected".into()),
                    };
                    shared.lock().unwrap().last_nvram_result = Some(result);
                    repaint();
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        // ---- hot reload: pick up edited scripts ----
        if last_rescan.elapsed() > Duration::from_millis(750) {
            last_rescan = Instant::now();
            let id = reg.entries.get(selected).map(|e| e.meta.id.clone());
            if reg.refresh() {
                selected = id.as_deref().and_then(|id| reg.find(id)).unwrap_or(0);
                params = merge_params(&reg, selected, params);
                publish_effects(&shared, &reg);
                publish_selection(&shared, selected, &params);
                applied_profile = None;
            }
        }

        // ---- device ----
        if device.is_none() {
            let due = last_open_attempt
                .map(|t| t.elapsed() > Duration::from_secs(2))
                .unwrap_or(true);
            if due {
                last_open_attempt = Some(Instant::now());

                // OpenRGB can start or rescan without the HID device set changing.
                // Rediscover on every disconnected retry, as well as manual scans.
                let found = discover(&mut discovery_cache, &opts, &endpoint, needs_scan);
                let candidates = found.candidates;
                shared.lock().unwrap().discovery_warnings = found.warnings;
                publish_devices(&shared, &candidates, pinned.as_deref());
                needs_scan = false;

                match backend::choose(&candidates, pinned.as_deref()) {
                    Some(i) => {
                        let cand = candidates[i].clone();
                        match Device::open(&cand, endpoint.as_ref().ok().copied().flatten()) {
                            Ok(mut kb) => {
                                let ceiling = kb.fps_ceiling();
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
                                            can_save: kb.can_save(),
                                            has_matrix: kb.has_matrix(),
                                        };
                                        s.layout = kb.layout().to_vec();
                                        s.frame = Frame::black(kb.led_count());
                                        s.max_fps_ceiling = ceiling;
                                        s.link = Some(kb.link());
                                        drop(s);
                                        write_failures = 0;
                                        live_preview = None;
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
                                needs_scan = true;
                                shared.lock().unwrap().status =
                                    DeviceStatus::Disconnected(e.to_string());
                            }
                        }
                    }
                    // A pin is never silently substituted. Lighting the wired
                    // board because the pinned receiver vanished would look
                    // like the app ignoring the setting.
                    None if pinned.is_some() => {
                        let id = pinned.as_ref().unwrap();
                        let label = shared
                            .lock()
                            .unwrap()
                            .devices
                            .iter()
                            .find(|d| &d.id == id)
                            .map(|d| d.label.clone())
                            .unwrap_or_else(|| id.to_string());
                        shared.lock().unwrap().status = DeviceStatus::Waiting {
                            pinned: id.clone(),
                            label,
                        };
                    }
                    None => {
                        shared.lock().unwrap().status = DeviceStatus::Disconnected(
                            "No keyboard found. Connect an AULA F75, or start OpenRGB's SDK server for other models. See Settings for scan details.".into());
                    }
                }
            }
        }

        // Independent of painting, pause, and connection state: profiles keep
        // following focus in the tray and apply on the next hardware reconnect.
        if restore_default
            || (profiles::supported()
                && app_lighting.enabled
                && last_foreground_poll
                    .map(|t| t.elapsed() >= Duration::from_millis(250))
                    .unwrap_or(true))
        {
            last_foreground_poll = Some(Instant::now());
            let foreground: Option<String> = if restore_default {
                None
            } else {
                match foreground_detector.poll() {
                    #[cfg(any(target_os = "windows", target_os = "linux"))]
                    Foreground::Application(path) => Some(path),
                    #[cfg(any(target_os = "windows", target_os = "linux"))]
                    Foreground::OwnWindow => None,
                    Foreground::Unavailable => None,
                }
            };
            if restore_default || foreground.is_some() {
                let wireless = device.as_ref().is_some_and(|kb| kb.link() == Link::Dongle);
                if let Some((selection, status)) =
                    profiles::resolve(&app_lighting, &reg, foreground.as_deref(), wireless)
                {
                    if applied_profile.as_ref() != Some(&selection) {
                        selected = selection.index;
                        params = selection.preset.restore(&reg.entries[selected].meta.params);
                        effect_start = Instant::now();
                        publish_selection(&shared, selected, &params);
                        applied_profile = Some(selection);
                    }
                    shared.lock().unwrap().profile_status = status;
                }
                restore_default = false;
            }
        }

        let Some(kb) = device.as_mut() else {
            std::thread::sleep(Duration::from_millis(200));
            repaint();
            continue;
        };

        // Live preview from the editor overrides everything, paused or not: the
        // user is actively painting and expects the board to follow the brush.
        if let Some(preview) = live_preview.clone() {
            if preview.len() != kb.led_count() {
                live_preview = None;
                shared.lock().unwrap().errors.push("Preview size does not match the selected keyboard; reopen or recreate the animation.".into());
                continue;
            }
            match kb.stream(&preview) {
                Ok(()) => {
                    write_failures = 0;
                    shared.lock().unwrap().frame = preview;
                }
                Err(e) => {
                    write_failures += 1;
                    if write_failures >= WRITE_FAILURES_BEFORE_DISCONNECT {
                        let mut s = shared.lock().unwrap();
                        s.status = DeviceStatus::Disconnected(e.to_string());
                        s.link = None;
                        s.fps = 0.0;
                        device = None;
                        live_preview = None;
                        needs_scan = true;
                    }
                }
            }
            repaint();
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }

        // The radio cannot stream full-board animation smoothly, so wireless is
        // solid-colours-only for now. The UI blocks picking an animation over
        // the dongle, but the physical side-switch can flip the link under a
        // running animation — this is the backstop: fall back to the first
        // non-animation effect rather than stutter.
        if kb.link() == Link::Dongle && blocks_wireless(&reg, selected) {
            if let Some(i) = first_wireless_safe(&reg) {
                selected = i;
                params = current_params(&reg, selected);
                effect_start = Instant::now();
                publish_selection(&shared, selected, &params);
                applied_profile = None;
            }
        }

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
                    s.link = None;
                    drop(s);
                    device = None;
                    write_failures = 0;
                    // Whatever is plugged in may have changed, so the next
                    // open attempt must probe rather than reuse the old list.
                    needs_scan = true;
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
fn publish_selection(shared: &Arc<Mutex<Shared>>, selected: usize, params: &Params) {
    let mut state = shared.lock().unwrap();
    state.selected = selected;
    state.params = params.clone();
    state.selection_revision = state.selection_revision.wrapping_add(1);
    state.script_error = None;
}

fn publish_devices(shared: &Arc<Mutex<Shared>>, candidates: &[Candidate], pinned: Option<&str>) {
    let mut devices: Vec<DeviceEntry> = candidates
        .iter()
        .map(|c| DeviceEntry {
            id: c.id(),
            label: c.label(),
            link: c.link(),
            confirmed: c.confirmed(),
            present: true,
        })
        .collect();

    if let Some(id) = pinned {
        if !devices.iter().any(|d| d.id == id) {
            devices.push(DeviceEntry {
                id: id.to_string(),
                label: shared
                    .lock()
                    .unwrap()
                    .devices
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| d.label.clone())
                    .unwrap_or_else(|| {
                        if id.starts_with("openrgb:") {
                            "Selected OpenRGB keyboard".into()
                        } else {
                            id.to_string()
                        }
                    }),
                link: if id.starts_with("openrgb:") {
                    Link::OpenRgb
                } else {
                    Link::Unknown
                },
                confirmed: false,
                present: false,
            });
        }
    }

    let mut s = shared.lock().unwrap();
    s.devices = devices;
    s.pinned = pinned.map(str::to_string);
}

fn discover(
    cache: &mut backend::DiscoveryCache,
    opts: &ScanOptions,
    endpoint: &Result<Option<std::net::SocketAddr>, String>,
    force: bool,
) -> backend::Discovery {
    let mut found = cache.scan(opts, endpoint.as_ref().ok().copied().flatten(), force);
    if let Err(e) = endpoint {
        found.warnings.push(e.clone());
    }
    found
}

fn publish_effects(shared: &Arc<Mutex<Shared>>, reg: &Registry) {
    let effects = reg
        .entries
        .iter()
        .map(|e| EffectInfo {
            meta: e.meta.clone(),
            is_script: matches!(e.source, Source::Script(_)),
            is_animation: matches!(e.source, Source::Animation(_)),
            is_composition: matches!(e.source, Source::Composition(_)),
            file: match &e.source {
                Source::Script(p) | Source::Animation(p) | Source::Composition(p) => Some(
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

/// Does the effect at `idx` stream full-board motion the radio can't keep up
/// with? Keyframe animations and layered compositions both do, so both are
/// blocked over the wireless link; built-ins and scripts are not.
fn blocks_wireless(reg: &Registry, idx: usize) -> bool {
    reg.entries
        .get(idx)
        .map(|e| matches!(e.source, Source::Animation(_) | Source::Composition(_)))
        .unwrap_or(false)
}

/// The first effect safe to drive over the wireless link, for the fallback.
fn first_wireless_safe(reg: &Registry) -> Option<usize> {
    reg.entries
        .iter()
        .position(|e| !matches!(e.source, Source::Animation(_) | Source::Composition(_)))
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
