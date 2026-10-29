//! Keyboard discovery and headless effects, using the same backends as the GUI.

use std::time::{Duration, Instant};

use aula_effects::registry::{Registry, Source};
use aula_effects::{Params, RenderCtx};
use aula_protocol::f75::keymap;
use aula_protocol::{DeviceId, Frame, RgbDevice, ScanOptions};

use crate::backend::{self, Device};
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
    if matches!(name, "--version" | "-V") {
        println!("keylux {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if matches!(name, "help" | "--help" | "-h") {
        println!("keylux devices [--openrgb IP:PORT | --native]\nkeylux keyboards\nkeylux list\nkeylux <effect> [seconds] [--device SELECTOR | --keyboard NAME] [--openrgb IP:PORT | --native] [--fps N] [--param value ...]\n\nUse devices to list connected keyboard selectors. OpenRGB keyboards require its SDK server.\n--keyboard matches one exact device name and rejects duplicates.\n--native limits discovery to AULA HID. --openrgb restricts discovery to that SDK server.\nWith no selector, the saved GUI selection is used, then automatic selection (AULA preferred).");
        return Ok(());
    }
    if name == "keyboards" {
        println!("Native: AULA F75 (wired and receiver)\nOpenRGB targets (USB; upstream Direct support, physical testing still required):");
        for model in aula_protocol::openrgb::catalog::MODELS {
            println!("  {}  [{}]", model.name, model.usb_ids);
        }
        println!("\nOther keyboards exposing per-key Direct mode in OpenRGB are discovered too.\nSee docs/KEYBOARDS.md for setup, sources, and connection limitations.");
        return Ok(());
    }
    if name == "devices" {
        let settings = connection_settings(&args)?;
        let found = discover(args.as_slice(), &settings)?;
        for c in &found.candidates {
            println!("{}\n  --device {}", c.label(), c.id());
        }
        for warning in &found.warnings {
            eprintln!("{warning}");
        }
        if found.candidates.is_empty() {
            anyhow::bail!("No compatible keyboards found");
        }
        return Ok(());
    }

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
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(20.0);
    anyhow::ensure!(
        seconds.is_finite() && seconds > 0.0 && seconds <= 86400.0,
        "duration must be between 0 and 86400 seconds"
    );

    let meta = reg.entries[idx].meta.clone();
    let mut params = Params::from_specs(&meta.params);
    apply_cli_overrides(&args, &meta, &mut params);

    let mut kb = open_device(&args)?;
    if let Some(raw) = option(&args, "--fps")? {
        let fps = raw.parse::<u32>()?;
        anyhow::ensure!(fps > 0, "--fps must be positive");
        kb.set_max_fps(fps);
    }
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
fn scan_options(settings: &Settings) -> ScanOptions {
    ScanOptions {
        allow: settings.allow_list(),
        ..Default::default()
    }
}

fn connection_settings(args: &[String]) -> anyhow::Result<Settings> {
    let mut settings = Settings::load();
    let remote = option(args, "--openrgb")?;
    let native = args.iter().any(|s| s == "--native");
    anyhow::ensure!(
        !(native && remote.is_some()),
        "--native and --openrgb cannot be combined"
    );
    if let Some(endpoint) = remote {
        settings.openrgb_enabled = true;
        settings.openrgb_endpoint = endpoint.into();
    }
    if native {
        settings.openrgb_enabled = false;
    }
    Ok(settings)
}

fn open_device(args: &[String]) -> anyhow::Result<Device> {
    let settings = connection_settings(args)?;
    let endpoint = settings.endpoint().map_err(anyhow::Error::msg)?;
    let explicit = option(args, "--device")?;
    let by_name = option(args, "--keyboard")?;
    anyhow::ensure!(
        !(explicit.is_some() && by_name.is_some()),
        "choose --device or --keyboard"
    );
    let mut opts = scan_options(&settings);
    if let Some(id) = explicit {
        if !id.starts_with("openrgb:") {
            let id = id.parse::<DeviceId>()?;
            if !opts.allow.contains(&id) {
                opts.allow.push(id);
            }
        }
    }
    let found = if option(args, "--openrgb")?.is_some() {
        backend::discover_openrgb(endpoint.unwrap())
    } else {
        backend::discover(&opts, endpoint)
    };
    let pin = explicit
        .map(|id| {
            id.parse::<DeviceId>()
                .map(|id| id.to_string())
                .unwrap_or_else(|_| id.to_string())
        })
        .or_else(|| settings.target());
    let index = if let Some(name) = by_name {
        let matches: Vec<_> = found
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| match c {
                backend::Candidate::OpenRgb(c) => c.name.eq_ignore_ascii_case(name),
                backend::Candidate::Native(c) => c.label().eq_ignore_ascii_case(name),
            })
            .map(|(i, _)| i)
            .collect();
        anyhow::ensure!(
            matches.len() == 1,
            "--keyboard must match exactly one connected keyboard; run `keylux devices`"
        );
        Some(matches[0])
    } else {
        backend::choose(&found.candidates, pin.as_deref())
    };
    let index = index.ok_or_else(|| {
        anyhow::anyhow!(
            "Selected keyboard not found. Run `keylux devices`. {}",
            found.warnings.join("; ")
        )
    })?;
    Device::open(&found.candidates[index], endpoint)
}

fn option<'a>(args: &'a [String], flag: &str) -> anyhow::Result<Option<&'a str>> {
    let positions: Vec<_> = args
        .iter()
        .enumerate()
        .filter(|(_, s)| s.as_str() == flag)
        .map(|(i, _)| i)
        .collect();
    anyhow::ensure!(positions.len() <= 1, "{flag} specified more than once");
    let Some(&i) = positions.first() else {
        return Ok(None);
    };
    let value = args
        .get(i + 1)
        .filter(|v| !v.starts_with("--"))
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))?;
    Ok(Some(value))
}

fn discover(args: &[String], settings: &Settings) -> anyhow::Result<backend::Discovery> {
    let endpoint = settings.endpoint().map_err(anyhow::Error::msg)?;
    Ok(if option(args, "--openrgb")?.is_some() {
        backend::discover_openrgb(endpoint.unwrap())
    } else {
        backend::discover(&scan_options(settings), endpoint)
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
        if key == "native" {
            i += 1;
            continue;
        }
        let Some(raw) = args.get(i + 1) else { break };
        if matches!(key, "device" | "keyboard" | "openrgb" | "fps") {
            i += 2;
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_options_reject_missing_and_duplicate_values() {
        for values in [
            vec!["--device"],
            vec!["--device", "--fps", "10"],
            vec!["--device", "258a:010c", "--device", "258a:010c"],
        ] {
            let args: Vec<_> = values.into_iter().map(str::to_string).collect();
            assert!(option(&args, "--device").is_err());
        }
    }

    #[test]
    fn connection_flags_do_not_consume_effect_parameters() {
        let args = [
            "solid",
            "1",
            "--native",
            "--color",
            "#123456",
            "--fps",
            "10",
            "--keyboard",
            "Some keyboard",
        ]
        .map(str::to_string);
        let effect = aula_effects::builtin::all()
            .into_iter()
            .find(|e| e.meta().id == "solid")
            .unwrap();
        let meta = effect.meta();
        let mut params = Params::from_specs(&meta.params);
        apply_cli_overrides(&args, &meta, &mut params);
        assert_eq!(
            params.color("color", aula_protocol::Rgb::BLACK),
            aula_protocol::Rgb::new(0x12, 0x34, 0x56)
        );
    }
}
