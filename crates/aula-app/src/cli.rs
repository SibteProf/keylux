//! `keylux` — control for AULA keyboard lighting.
//!
//! Phase 4 turns this into an egui app. For now it is a CLI over the same
//! effect engine the GUI will use, which keeps the render loop verified on
//! hardware while the UI is built.

use std::time::{Duration, Instant};

use aula_effects::registry::{Registry, Source};
use aula_effects::{Params, RenderCtx};
use aula_protocol::f75::keymap;
use aula_protocol::{DeviceId, Frame, Keyboard, RgbDevice, ScanOptions};

use crate::settings::Settings;

fn effects_dir() -> std::path::PathBuf {
    // Next to the executable when installed, or the repo's effects/ in dev.
    for candidate in ["effects", "../effects", "../../effects"] {
        let p = std::path::PathBuf::from(candidate);
        if p.is_dir() {
            return p;
        }
    }
    std::path::PathBuf::from("effects")
}

pub fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map(String::as_str).unwrap_or("list");

    let dir = effects_dir();
    let anim_dir = dir
        .parent()
        .map(|p| p.join("animations"))
        .unwrap_or_else(|| std::path::PathBuf::from("animations"));
    let mut reg = Registry::with_all(&dir, &anim_dir);

    for e in &reg.errors {
        eprintln!("script error: {e}");
    }

    if name == "list" {
        println!("Effects directory: {}\n", dir.display());
        for e in &reg.entries {
            let tag = match &e.source {
                Source::Builtin => "built-in".to_string(),
                Source::Script(p) => format!(
                    "script: {}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ),
                Source::Animation(p) => format!(
                    "animation: {}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ),
                Source::Composition(p) => format!(
                    "composition: {}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ),
            };
            println!("  {:<10} {:<28} [{tag}]", e.meta.id, e.meta.name);
            if !e.meta.description.is_empty() {
                println!("             {}", e.meta.description);
            }
            for p in &e.meta.params {
                println!("               --{:<10} {:?}", p.id, p.kind);
            }
        }
        println!("\nRun one:  keylux <id> [seconds] [--param value ...]");
        println!("Add your own: drop a .rhai file into {}", dir.display());
        return Ok(());
    }

    let idx = reg
        .find(name)
        .ok_or_else(|| anyhow::anyhow!("no effect called {name:?}; try `list`"))?;

    let seconds: f32 = args
        .get(1)
        .filter(|s| !s.starts_with("--"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(20.0);

    let meta = reg.entries[idx].meta.clone();
    let mut params = Params::from_specs(&meta.params);
    apply_cli_overrides(&args, &meta, &mut params);

    let mut kb = open_device(&args)?;
    println!("Using {} ({})", kb.name(), kb.hid_path());
    // Once, before streaming. A config write inside the loop would trigger an
    // async repaint that overwrites frames.
    if kb.ensure_per_key_mode()? {
        println!("Switched the board into per-key mode.");
    }

    let layout = kb.layout().to_vec();
    let max_x = keymap::max_x(&layout);
    let max_row = f32::from(keymap::max_row(&layout));
    let fps = kb.max_fps();
    let period = Duration::from_secs_f32(1.0 / fps as f32);

    println!(
        "Running {:?} at {fps} FPS for {seconds}s. Ctrl+C to stop.",
        meta.name
    );

    let start = Instant::now();
    let mut frames = 0u32;
    let mut reported_error: Option<String> = None;

    while start.elapsed().as_secs_f32() < seconds {
        let ctx = RenderCtx {
            t: start.elapsed().as_secs_f32(),
            layout: &layout,
            max_x,
            max_row,
            params: &params,
        };
        let mut frame = Frame::black(kb.led_count());
        reg.entries[idx].effect_mut().render(&ctx, &mut frame);
        kb.stream(&frame)?;
        frames += 1;

        // Surface a script's runtime error once rather than every frame.
        if let Some(err) = script_error(&reg.entries[idx]) {
            if reported_error.as_deref() != Some(err.as_str()) {
                eprintln!("script error: {err}");
                reported_error = Some(err);
            }
        }

        let target = start + period * frames;
        if let Some(wait) = target.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }

    let secs = start.elapsed().as_secs_f32();
    println!("Stopped. {frames} frames, {:.1} FPS.", frames as f32 / secs);
    Ok(())
}

fn script_error(entry: &aula_effects::registry::Entry) -> Option<String> {
    entry.runtime_error().map(str::to_string)
}

/// `--speed 1.5 --color #ff00aa` style overrides, matched against declared params.
/// Open the board a headless run should drive.
///
/// `--device vid:pid` targets one specifically; otherwise this honours whatever
/// the GUI was last pinned to, so a user who picked their receiver in the app
/// does not have to name it again here.
fn open_device(args: &[String]) -> anyhow::Result<Keyboard> {
    let settings = Settings::load();
    let opts = ScanOptions {
        allow: settings.allow_list(),
        ..Default::default()
    };
    let explicit = args
        .iter()
        .position(|a| a == "--device")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.parse::<DeviceId>())
        .transpose()?;

    Ok(match explicit.or_else(|| settings.pinned_device()) {
        Some(id) => Keyboard::open_id(id, &opts)?,
        None => Keyboard::open_best(&opts)?,
    })
}

fn apply_cli_overrides(args: &[String], meta: &aula_effects::EffectMeta, params: &mut Params) {
    use aula_effects::{ParamKind, Value};
    use aula_protocol::Rgb;

    let mut i = 0;
    while i < args.len() {
        let Some(key) = args[i].strip_prefix("--") else {
            i += 1;
            continue;
        };
        let Some(raw) = args.get(i + 1) else { break };
        if let Some(spec) = meta.params.iter().find(|p| p.id == key) {
            let v = match &spec.kind {
                ParamKind::Float { .. } => raw.parse::<f32>().ok().map(Value::Float),
                ParamKind::Int { .. } => raw.parse::<i64>().ok().map(Value::Int),
                ParamKind::Bool { .. } => raw.parse::<bool>().ok().map(Value::Bool),
                ParamKind::Color { .. } => Rgb::from_hex(raw).map(Value::Color),
                ParamKind::Text { .. } => Some(Value::Text(raw.clone())),
                ParamKind::Choice { .. } => raw.parse::<i64>().ok().map(Value::Int),
            };
            match v {
                Some(v) => params.set(key, v),
                None => eprintln!("could not parse --{key} {raw:?}, using default"),
            }
        } else {
            eprintln!("unknown parameter --{key} for this effect");
        }
        i += 2;
    }
}
