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
use aula_protocol::f75::{keymap, F75};
use aula_protocol::{Frame, KeyPos, RgbDevice};

/// What the GUI asks the engine to do.
pub enum Cmd {
    SelectEffect(usize),
    /// Whole parameter set, so UI and engine cannot drift apart.
    SetParams(Params),
    SetRunning(bool),
    /// Re-scan the effects directory now.
    Rescan,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeviceStatus {
    Connected { name: String, path: String },
    Disconnected(String),
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
        }
    }
}

pub struct Engine {
    pub shared: Arc<Mutex<Shared>>,
    tx: Sender<Cmd>,
}

impl Engine {
    /// Spawn the engine thread.
    pub fn spawn(effects_dir: std::path::PathBuf, repaint: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Mutex::new(Shared::new()));
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);

        std::thread::Builder::new()
            .name("aula-render".into())
            .spawn(move || run(worker_shared, rx, effects_dir, repaint))
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
    repaint: impl Fn() + Send,
) {
    let anim_dir = effects_dir
        .parent()
        .map(|p| p.join("animations"))
        .unwrap_or_else(|| std::path::PathBuf::from("animations"));
    let mut reg = Registry::with_all(&effects_dir, &anim_dir);
    publish_effects(&shared, &reg);

    let mut device: Option<F75> = None;
    let mut last_open_attempt: Option<Instant> = None;
    let mut selected = 0usize;
    let mut params = current_params(&reg, selected);
    let mut effect_start = Instant::now();
    let mut running = true;

    let mut fps_window = Instant::now();
    let mut fps_frames = 0u32;
    let mut last_rescan = Instant::now();

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
                match F75::open() {
                    Ok(mut kb) => {
                        // Once, before any streaming.
                        let mode = kb.ensure_per_key_mode();
                        let mut s = shared.lock().unwrap();
                        match mode {
                            Ok(_) => {
                                s.status = DeviceStatus::Connected {
                                    name: kb.name().to_string(),
                                    path: kb.hid_path().to_string(),
                                };
                                s.layout = kb.layout().to_vec();
                                s.frame = Frame::black(kb.led_count());
                                drop(s);
                                device = Some(kb);
                            }
                            Err(e) => {
                                s.status = DeviceStatus::Disconnected(e.to_string());
                            }
                        }
                    }
                    Err(e) => {
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

        // ---- render one frame ----
        let layout = kb.layout().to_vec();
        let mut frame = Frame::black(kb.led_count());

        if running && !reg.is_empty() {
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
                // Most likely unplugged; drop the handle and let the retry loop
                // pick it up again.
                let mut s = shared.lock().unwrap();
                s.status = DeviceStatus::Disconnected(e.to_string());
                s.fps = 0.0;
                drop(s);
                device = None;
            }
        }

        repaint();

        // stream() already enforces the hardware minimum gap; this just avoids
        // spinning when the effect renders faster than the device can accept.
        std::thread::sleep(Duration::from_millis(2));
    }
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
