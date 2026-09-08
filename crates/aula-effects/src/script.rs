//! Rhai scripting host.
//!
//! A script is an effect. It implements the same contract as a built-in, so
//! anyone can add one by dropping a `.rhai` file into the effects directory —
//! no Rust toolchain, no rebuild.
//!
//! # Contract
//!
//! ```rhai
//! fn meta() {
//!     #{
//!         name: "Spiral",
//!         description: "A rotating arm",
//!         params: [
//!             #{ id: "speed", label: "Speed", kind: "float",
//!                min: 0.1, max: 5.0, default: 1.0 },
//!             #{ id: "tint",  label: "Tint",  kind: "color", default: "#ff00aa" },
//!         ],
//!     }
//! }
//!
//! // t     seconds since the effect started
//! // keys  array of #{ x, y, row, w, led, nx, ny } — one per physical key
//! // p     map of current parameter values
//! //
//! // return one colour per key, in the same order as `keys`
//! fn render(t, keys, p) {
//!     let out = [];
//!     for k in keys {
//!         out.push(hsv(k.nx + t * p.speed, 1.0, 1.0));
//!     }
//!     out
//! }
//! ```
//!
//! Colours may be returned as `hsv(..)`/`rgb(..)`, a `#rrggbb` string, or an
//! `[r, g, b]` array.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rhai::{Array, Dynamic, Engine, Map, Scope, AST};

use aula_protocol::{Frame, KeyPos, Rgb};

use crate::{Effect, EffectMeta, ParamKind, ParamSpec, Params, RenderCtx};

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("{path}: {source}")]
    Compile {
        path: String,
        #[source]
        source: Box<rhai::EvalAltResult>,
    },
    #[error("{path}: {source}")]
    Runtime {
        path: String,
        #[source]
        source: Box<rhai::EvalAltResult>,
    },
    #[error("{path}: {msg}")]
    Contract { path: String, msg: String },
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// An effect backed by a Rhai script.
pub struct ScriptEffect {
    path: PathBuf,
    engine: Engine,
    ast: AST,
    meta: EffectMeta,
    /// Rebuilt only when the layout changes, not per frame.
    keys_cache: Option<(usize, Array)>,
    mtime: Option<SystemTime>,
    /// Last runtime error, so the UI can show it without spamming the console.
    pub last_error: Option<String>,
}

impl ScriptEffect {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ScriptError> {
        let path = path.as_ref().to_path_buf();
        let src = std::fs::read_to_string(&path)?;
        let engine = build_engine();

        let ast = engine.compile(&src).map_err(|e| ScriptError::Compile {
            path: display(&path),
            source: Box::new(e.into()),
        })?;

        let meta = read_meta(&engine, &ast, &path)?;
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

        Ok(Self {
            path,
            engine,
            ast,
            meta,
            keys_cache: None,
            mtime,
            last_error: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Has the file changed on disk since it was loaded?
    pub fn is_stale(&self) -> bool {
        match std::fs::metadata(&self.path).and_then(|m| m.modified()) {
            Ok(now) => Some(now) != self.mtime,
            Err(_) => false,
        }
    }

    /// Recompile in place. On failure the previous version keeps running, so a
    /// typo mid-edit does not kill the animation.
    pub fn reload(&mut self) -> Result<(), ScriptError> {
        let fresh = Self::load(&self.path)?;
        self.engine = fresh.engine;
        self.ast = fresh.ast;
        self.meta = fresh.meta;
        self.mtime = fresh.mtime;
        self.keys_cache = None;
        self.last_error = None;
        Ok(())
    }

    fn keys_array(&mut self, ctx: &RenderCtx) -> Array {
        if let Some((len, cached)) = &self.keys_cache {
            if *len == ctx.layout.len() {
                return cached.clone();
            }
        }
        let arr: Array = ctx
            .layout
            .iter()
            .map(|k| Dynamic::from_map(key_map(k, ctx)))
            .collect();
        self.keys_cache = Some((ctx.layout.len(), arr.clone()));
        arr
    }
}

impl Effect for ScriptEffect {
    fn meta(&self) -> EffectMeta {
        self.meta.clone()
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        let keys = self.keys_array(ctx);
        let params = params_map(&self.meta.params, ctx.params);

        let mut scope = Scope::new();
        let result: Result<Dynamic, _> = self.engine.call_fn(
            &mut scope,
            &self.ast,
            "render",
            (ctx.t as f64, keys, params),
        );

        match result {
            Ok(v) => {
                self.last_error = None;
                apply(v, ctx.layout, out);
            }
            Err(e) => {
                // Leave the previous frame in place rather than flashing black,
                // and record the error once for the UI.
                let msg = e.to_string();
                if self.last_error.as_deref() != Some(msg.as_str()) {
                    self.last_error = Some(msg);
                }
            }
        }
    }
}

/// Engine with the helpers scripts are expected to have.
fn build_engine() -> Engine {
    let mut engine = Engine::new();

    // Scripts are local files the user chose to run, but they should not be
    // able to hang the render thread by accident.
    engine.set_max_operations(2_000_000);
    engine.set_max_call_levels(32);
    engine.set_max_string_size(8 * 1024);

    engine.register_fn("rgb", |r: f64, g: f64, b: f64| -> Map {
        color_map(Rgb::from_f32(
            (r / 255.0) as f32,
            (g / 255.0) as f32,
            (b / 255.0) as f32,
        ))
    });
    engine.register_fn("hsv", |h: f64, s: f64, v: f64| -> Map {
        color_map(Rgb::from_hsv(h as f32, s as f32, v as f32))
    });
    engine.register_fn("clamp", |v: f64, lo: f64, hi: f64| v.clamp(lo, hi));
    engine.register_fn("lerp", |a: f64, b: f64, t: f64| a + (b - a) * t);
    engine.register_fn("dist", |x0: f64, y0: f64, x1: f64, y1: f64| {
        ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt()
    });
    engine.register_fn("frac", |v: f64| v - v.floor());
    engine.register_fn("smoothstep", |e0: f64, e1: f64, x: f64| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    });

    engine
}

fn read_meta(engine: &Engine, ast: &AST, path: &Path) -> Result<EffectMeta, ScriptError> {
    let mut scope = Scope::new();
    let m: Map = engine
        .call_fn(&mut scope, ast, "meta", ())
        .map_err(|e| ScriptError::Runtime {
            path: display(path),
            source: e,
        })?;

    let contract = |msg: &str| ScriptError::Contract {
        path: display(path),
        msg: msg.to_string(),
    };

    let name = m
        .get("name")
        .and_then(|v| v.clone().into_string().ok())
        .ok_or_else(|| contract("meta() must return a map with a string `name`"))?;

    let description = m
        .get("description")
        .and_then(|v| v.clone().into_string().ok())
        .unwrap_or_default();

    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());

    let mut params = Vec::new();
    if let Some(list) = m.get("params").and_then(|v| v.clone().try_cast::<Array>()) {
        for entry in list {
            let pm = entry
                .try_cast::<Map>()
                .ok_or_else(|| contract("each entry of `params` must be a map"))?;
            params.push(param_from_map(&pm, path)?);
        }
    }

    Ok(EffectMeta {
        id,
        name,
        description,
        params,
    })
}

fn param_from_map(m: &Map, path: &Path) -> Result<ParamSpec, ScriptError> {
    let contract = |msg: String| ScriptError::Contract {
        path: display(path),
        msg,
    };

    let id = m
        .get("id")
        .and_then(|v| v.clone().into_string().ok())
        .ok_or_else(|| contract("a param needs a string `id`".into()))?;
    let label = m
        .get("label")
        .and_then(|v| v.clone().into_string().ok())
        .unwrap_or_else(|| id.clone());
    let kind_s = m
        .get("kind")
        .and_then(|v| v.clone().into_string().ok())
        .unwrap_or_else(|| "float".into());

    // `default` is a reserved word in Rhai, so it must be quoted in a map
    // literal. `def` is accepted as an unquoted alias for convenience.
    let field = |key: &str| -> Option<&rhai::Dynamic> {
        if key == "default" {
            m.get("default").or_else(|| m.get("def"))
        } else {
            m.get(key)
        }
    };
    let f = |key: &str, fallback: f64| -> f64 { field(key).and_then(num).unwrap_or(fallback) };

    let kind = match kind_s.as_str() {
        "float" => ParamKind::Float {
            min: f("min", 0.0) as f32,
            max: f("max", 1.0) as f32,
            default: f("default", 0.0) as f32,
        },
        "int" => ParamKind::Int {
            min: f("min", 0.0) as i64,
            max: f("max", 100.0) as i64,
            default: f("default", 0.0) as i64,
        },
        "bool" => ParamKind::Bool {
            default: field("default")
                .and_then(|v| v.clone().as_bool().ok())
                .unwrap_or(false),
        },
        "color" => ParamKind::Color {
            default: field("default")
                .and_then(dyn_to_color)
                .unwrap_or(Rgb::WHITE),
        },
        "text" => ParamKind::Text {
            default: field("default")
                .and_then(|v| v.clone().into_string().ok())
                .unwrap_or_default(),
        },
        other => {
            return Err(contract(format!(
                "unknown param kind {other:?}; use float, int, bool, color or text"
            )))
        }
    };

    Ok(ParamSpec { id, label, kind })
}

fn num(v: &Dynamic) -> Option<f64> {
    if let Ok(f) = v.as_float() {
        Some(f)
    } else {
        v.as_int().ok().map(|i| i as f64)
    }
}

fn key_map(k: &KeyPos, ctx: &RenderCtx) -> Map {
    let mut m = Map::new();
    m.insert("x".into(), Dynamic::from_float(k.x as f64));
    m.insert("y".into(), Dynamic::from_float(f64::from(k.row)));
    m.insert("row".into(), Dynamic::from_int(i64::from(k.row)));
    m.insert("w".into(), Dynamic::from_float(k.w as f64));
    m.insert("led".into(), Dynamic::from_int(k.led as i64));
    m.insert("nx".into(), Dynamic::from_float(ctx.nx(k) as f64));
    m.insert("ny".into(), Dynamic::from_float(ctx.ny(k) as f64));
    m.insert("name".into(), Dynamic::from(k.name.to_string()));
    m
}

fn params_map(specs: &[ParamSpec], values: &Params) -> Map {
    let mut m = Map::new();
    for s in specs {
        let v = match &s.kind {
            ParamKind::Float { default, .. } => {
                Dynamic::from_float(values.float(&s.id, *default) as f64)
            }
            ParamKind::Int { default, .. } => Dynamic::from_int(values.int(&s.id, *default)),
            ParamKind::Bool { default } => Dynamic::from_bool(values.bool(&s.id, *default)),
            ParamKind::Color { default } => {
                Dynamic::from_map(color_map(values.color(&s.id, *default)))
            }
            ParamKind::Text { default } => Dynamic::from(values.text(&s.id, default)),
            ParamKind::Choice { default, .. } => {
                Dynamic::from_int(values.int(&s.id, *default as i64))
            }
        };
        m.insert(s.id.clone().into(), v);
    }
    m
}

fn color_map(c: Rgb) -> Map {
    let mut m = Map::new();
    m.insert("r".into(), Dynamic::from_int(i64::from(c.r)));
    m.insert("g".into(), Dynamic::from_int(i64::from(c.g)));
    m.insert("b".into(), Dynamic::from_int(i64::from(c.b)));
    m
}

/// Accept a colour as a map, an `[r, g, b]` array, or a `#rrggbb` string.
fn dyn_to_color(v: &Dynamic) -> Option<Rgb> {
    if let Some(m) = v.clone().try_cast::<Map>() {
        let ch = |k: &str| m.get(k).and_then(num).unwrap_or(0.0);
        return Some(Rgb::new(
            ch("r").clamp(0.0, 255.0) as u8,
            ch("g").clamp(0.0, 255.0) as u8,
            ch("b").clamp(0.0, 255.0) as u8,
        ));
    }
    if let Some(a) = v.clone().try_cast::<Array>() {
        if a.len() >= 3 {
            let ch = |i: usize| num(&a[i]).unwrap_or(0.0).clamp(0.0, 255.0) as u8;
            return Some(Rgb::new(ch(0), ch(1), ch(2)));
        }
    }
    if let Ok(s) = v.clone().into_string() {
        return Rgb::from_hex(&s);
    }
    None
}

/// Map the script's return value onto the frame.
fn apply(v: Dynamic, layout: &[KeyPos], out: &mut Frame) {
    let Some(arr) = v.try_cast::<Array>() else {
        return;
    };
    for (k, item) in layout.iter().zip(arr.iter()) {
        if let Some(c) = dyn_to_color(item) {
            out.set(k.led, c);
        }
    }
}

fn display(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aula_protocol::f75::keymap;

    fn render_once(path: &str, t: f32) -> (ScriptEffect, Frame) {
        let mut fx = ScriptEffect::load(path).expect("script should load");
        let layout = keymap::layout();
        let params = Params::from_specs(&fx.meta().params);
        let ctx = RenderCtx {
            t,
            layout: &layout,
            max_x: keymap::max_x(&layout),
            max_row: f32::from(keymap::max_row(&layout)),
            params: &params,
        };
        let mut frame = Frame::black(126);
        fx.render(&ctx, &mut frame);
        (fx, frame)
    }

    #[test]
    fn shipped_scripts_load_and_declare_params() {
        for name in ["spiral", "breathe", "ripple"] {
            let path = format!("../../effects/{name}.rhai");
            let fx = ScriptEffect::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            let m = fx.meta();
            assert!(!m.name.is_empty(), "{name} needs a name");
            assert!(!m.params.is_empty(), "{name} should declare params");
        }
    }

    #[test]
    fn scripts_light_the_board_and_animate() {
        for name in ["spiral", "breathe", "ripple"] {
            let path = format!("../../effects/{name}.rhai");
            let (fx, a) = render_once(&path, 0.30);
            assert!(fx.last_error.is_none(), "{name}: {:?}", fx.last_error);
            let (_, b) = render_once(&path, 1.70);
            assert!(
                a.iter().any(|c| !c.is_black()),
                "{name} produced an entirely black frame"
            );
            assert_ne!(a, b, "{name} does not animate");
        }
    }

    #[test]
    fn param_changes_reach_the_script() {
        let mut fx = ScriptEffect::load("../../effects/breathe.rhai").unwrap();
        let layout = keymap::layout();
        let mut params = Params::from_specs(&fx.meta().params);
        params.set("color", crate::Value::Color(Rgb::new(0, 255, 0)));

        let ctx = RenderCtx {
            t: 2.0, // near the peak of the breathe curve
            layout: &layout,
            max_x: keymap::max_x(&layout),
            max_row: f32::from(keymap::max_row(&layout)),
            params: &params,
        };
        let mut frame = Frame::black(126);
        fx.render(&ctx, &mut frame);

        let lit = frame.get(layout[0].led);
        assert!(lit.g > lit.r, "green param should dominate, got {lit:?}");
    }

    #[test]
    fn a_broken_script_reports_instead_of_panicking() {
        let dir = std::env::temp_dir().join("aula-script-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.rhai");
        // meta() is fine so it loads; render() explodes at runtime.
        std::fs::write(
            &path,
            r#"
            fn meta() { #{ name: "Broken", params: [] } }
            fn render(t, keys, p) { this_function_does_not_exist(); }
            "#,
        )
        .unwrap();

        let (fx, frame) = render_once(path.to_str().unwrap(), 0.5);
        assert!(fx.last_error.is_some(), "runtime error should be captured");
        assert!(frame.iter().all(|c| c.is_black()), "frame left untouched");
    }

    #[test]
    fn compile_errors_surface_as_errors() {
        let dir = std::env::temp_dir().join("aula-script-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("syntax.rhai");
        std::fs::write(&path, "fn meta( { this is not rhai").unwrap();
        assert!(ScriptEffect::load(&path).is_err());
    }
}
