//! Layered compositions: the merge of painted animations and procedural
//! effects into one thing.
//!
//! A [`Composition`] stacks [`Layer`]s — each a painted keyframe [`Animation`]
//! or a procedural [`GenParams`] generator — composited bottom-to-top by a
//! [`Blend`] mode and opacity. It plays back as a [`CompositeEffect`], which is
//! an ordinary [`Effect`]: the render engine drives it exactly like a built-in,
//! a script, or a plain animation, so nothing downstream needs to know layers
//! exist.
//!
//! Compositions are JSON in `.klx` files next to animations, so they stay
//! shareable and hand-editable. A layer's settings are baked in at authoring
//! time (matching plain animations); the only runtime knobs are master speed
//! and brightness.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use aula_protocol::{Frame, KeyPos, Rgb};

use crate::animation::{Animation, AnimationError};
use crate::generators::{self, GenParams};
use crate::{Effect, EffectMeta, ParamSpec, RenderCtx};

/// How a layer combines with everything beneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blend {
    /// `src` over `dst`, weighted by opacity. A dark layer dims what is below.
    Normal,
    /// Add the channels, saturating. The natural choice for stacking lights.
    #[default]
    Add,
    /// Inverse-multiply: lightens without clipping as hard as Add.
    Screen,
    /// Multiply channels: darkens, useful as a mask.
    Multiply,
    /// Keep the brighter channel of the two.
    Max,
}

/// Composite one pixel of `src` onto `dst`. `opacity` scales `src` first.
pub fn blend_px(dst: Rgb, src: Rgb, mode: Blend, opacity: f32) -> Rgb {
    let src = src.scale(opacity.clamp(0.0, 1.0));
    let ch = |d: u8, s: u8| -> u8 {
        let (d, s) = (d as f32, s as f32);
        let out = match mode {
            // `src` already carries opacity, so Normal mixes by that weight.
            Blend::Normal => d * (1.0 - opacity.clamp(0.0, 1.0)) + s,
            Blend::Add => d + s,
            Blend::Screen => 255.0 - (255.0 - d) * (255.0 - s) / 255.0,
            Blend::Multiply => d * s / 255.0,
            Blend::Max => d.max(s),
        };
        out.round().clamp(0.0, 255.0) as u8
    };
    Rgb::new(ch(dst.r, src.r), ch(dst.g, src.g), ch(dst.b, src.b))
}

/// What a layer draws.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LayerContent {
    /// Hand-painted or imported keyframes.
    Keyframes(Animation),
    /// A procedural motion baked to keyframes at play time.
    Generator(GenParams),
    // A `Script { path, params }` variant is planned; the tag-based format
    // leaves room for it without breaking existing files.
}

/// One layer in a composition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub content: LayerContent,
    #[serde(default)]
    pub blend: Blend,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

impl Layer {
    pub fn keyframes(name: impl Into<String>, anim: Animation) -> Self {
        Self {
            name: name.into(),
            content: LayerContent::Keyframes(anim),
            blend: Blend::default(),
            opacity: 1.0,
            enabled: true,
        }
    }

    pub fn generator(name: impl Into<String>, params: GenParams) -> Self {
        Self {
            name: name.into(),
            content: LayerContent::Generator(params),
            blend: Blend::default(),
            opacity: 1.0,
            enabled: true,
        }
    }
}

/// A stack of layers that play back as one effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// LED slots the layers cover.
    pub leds: usize,
    pub duration: f32,
    #[serde(default = "yes")]
    pub loops: bool,
    pub layers: Vec<Layer>,
}

impl Composition {
    /// A new composition with a single black keyframe layer to paint on.
    pub fn new(name: &str, leds: usize) -> Self {
        let mut base = Animation::new("Base", leds);
        base.interpolate = true;
        Self {
            name: name.to_string(),
            description: String::new(),
            leds,
            duration: base.duration,
            loops: true,
            layers: vec![Layer::keyframes("Base", base)],
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, AnimationError> {
        let text = std::fs::read_to_string(path.as_ref())?;
        let comp: Composition = serde_json::from_str(&text)?;
        Ok(comp)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), AnimationError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path.as_ref(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Composite every enabled layer at time `t` into `out`.
    ///
    /// Uncached — it bakes generator layers on each call, so it is for previews
    /// (the editor) rather than the hot playback path, which uses the cached
    /// [`CompositeEffect`].
    pub fn sample(&self, t: f32, layout: &[KeyPos], out: &mut Frame) {
        let leds = out.len();
        let mut scratch = Frame::black(leds);
        for layer in &self.layers {
            if !layer.enabled {
                continue;
            }
            scratch.fill(Rgb::BLACK);
            match &layer.content {
                LayerContent::Keyframes(a) => a.sample(t, &mut scratch),
                LayerContent::Generator(p) => {
                    let mut a = Animation::new(&layer.name, self.leds);
                    a.duration = self.duration;
                    a.loops = self.loops;
                    a.interpolate = true;
                    a.keyframes = generators::bake(p, self.leds, layout, self.duration);
                    a.normalise();
                    a.sample(t, &mut scratch);
                }
            }
            for led in 0..leds {
                let mixed = blend_px(out.get(led), scratch.get(led), layer.blend, layer.opacity);
                out.set(led, mixed);
            }
        }
    }

    /// Wrap a plain animation as a one-layer composition, so the editor can open
    /// legacy `.json` files in the same surface.
    pub fn from_animation(anim: Animation) -> Self {
        Self {
            name: anim.name.clone(),
            description: anim.description.clone(),
            leds: anim.leds,
            duration: anim.duration,
            loops: anim.loops,
            layers: vec![Layer::keyframes("Base", anim)],
        }
    }
}

/// Plays a [`Composition`] as an effect.
pub struct CompositeEffect {
    comp: Composition,
    id: String,
    path: Option<PathBuf>,
    /// Baked keyframes for generator layers, parallel to `comp.layers`; `None`
    /// for keyframe layers. Rebuilt when the layout width changes.
    baked: Vec<Option<Animation>>,
    baked_for: usize,
}

impl CompositeEffect {
    pub fn new(comp: Composition, id: impl Into<String>) -> Self {
        let baked = vec![None; comp.layers.len()];
        Self {
            comp,
            id: id.into(),
            path: None,
            baked,
            baked_for: 0,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, AnimationError> {
        let path = path.as_ref().to_path_buf();
        let comp = Composition::load(&path)?;
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| comp.name.clone());
        let mut fx = Self::new(comp, id);
        fx.path = Some(path);
        Ok(fx)
    }

    pub fn composition(&self) -> &Composition {
        &self.comp
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// (Re)bake generator layers for the current layout if needed.
    fn ensure_baked(&mut self, ctx: &RenderCtx) {
        let len = ctx.layout.len();
        if self.baked_for == len && self.baked.len() == self.comp.layers.len() {
            return;
        }
        self.baked = self
            .comp
            .layers
            .iter()
            .map(|layer| match &layer.content {
                LayerContent::Generator(params) => {
                    let kfs =
                        generators::bake(params, self.comp.leds, ctx.layout, self.comp.duration);
                    let mut a = Animation::new(&layer.name, self.comp.leds);
                    a.duration = self.comp.duration;
                    a.loops = self.comp.loops;
                    a.interpolate = true;
                    a.keyframes = kfs;
                    a.normalise();
                    Some(a)
                }
                LayerContent::Keyframes(_) => None,
            })
            .collect();
        self.baked_for = len;
    }
}

impl Effect for CompositeEffect {
    fn meta(&self) -> EffectMeta {
        EffectMeta {
            id: self.id.clone(),
            name: self.comp.name.clone(),
            description: if self.comp.description.is_empty() {
                format!("{} layers", self.comp.layers.len())
            } else {
                self.comp.description.clone()
            },
            params: vec![
                ParamSpec::float("speed", "Speed", 0.1, 4.0, 1.0),
                ParamSpec::float("brightness", "Brightness", 0.0, 1.0, 1.0),
            ],
        }
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        self.ensure_baked(ctx);

        let speed = ctx.params.float("speed", 1.0);
        let brightness = ctx.params.float("brightness", 1.0);
        let t = ctx.t * speed;
        let leds = out.len();
        let mut scratch = Frame::black(leds);

        for (i, layer) in self.comp.layers.iter().enumerate() {
            if !layer.enabled {
                continue;
            }
            // Draw the layer in isolation, then composite onto `out`.
            scratch.fill(Rgb::BLACK);
            match &layer.content {
                LayerContent::Keyframes(a) => a.sample(t, &mut scratch),
                LayerContent::Generator(_) => {
                    if let Some(a) = self.baked.get(i).and_then(|o| o.as_ref()) {
                        a.sample(t, &mut scratch);
                    }
                }
            }
            for led in 0..leds {
                let mixed = blend_px(out.get(led), scratch.get(led), layer.blend, layer.opacity);
                out.set(led, mixed);
            }
        }

        if brightness < 1.0 {
            out.scale(brightness);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{HexColor, Keyframe};
    use crate::generators::Generator;
    use crate::Params;
    use aula_protocol::f75::keymap;

    fn solid_anim(leds: usize, c: Rgb) -> Animation {
        let mut a = Animation::new("s", leds);
        a.interpolate = false;
        a.keyframes = vec![Keyframe {
            t: 0.0,
            colors: vec![HexColor::from(c); leds],
            ease: Default::default(),
        }];
        a.normalise();
        a
    }

    fn render_at(fx: &mut CompositeEffect, t: f32) -> Frame {
        let layout = keymap::layout();
        let params = Params::from_specs(&fx.meta().params);
        let ctx = RenderCtx {
            t,
            layout: &layout,
            max_x: keymap::max_x(&layout),
            max_row: f32::from(keymap::max_row(&layout)),
            params: &params,
        };
        let mut f = Frame::black(layout.iter().map(|k| k.led + 1).max().unwrap_or(0));
        fx.render(&ctx, &mut f);
        f
    }

    #[test]
    fn add_blend_sums_channels() {
        let r = blend_px(Rgb::new(10, 0, 0), Rgb::new(0, 20, 0), Blend::Add, 1.0);
        assert_eq!(r, Rgb::new(10, 20, 0));
    }

    #[test]
    fn opacity_scales_the_source() {
        let r = blend_px(Rgb::BLACK, Rgb::new(100, 0, 0), Blend::Add, 0.5);
        assert_eq!(r, Rgb::new(50, 0, 0));
    }

    #[test]
    fn max_blend_keeps_the_brighter_channel() {
        let r = blend_px(Rgb::new(200, 10, 0), Rgb::new(50, 90, 0), Blend::Max, 1.0);
        assert_eq!(r, Rgb::new(200, 90, 0));
    }

    #[test]
    fn layers_stack_bottom_to_top() {
        let leds = 126;
        let comp = Composition {
            name: "two".into(),
            description: String::new(),
            leds,
            duration: 1.0,
            loops: true,
            layers: vec![
                Layer::keyframes("base", solid_anim(leds, Rgb::new(20, 0, 0))),
                Layer::keyframes("top", solid_anim(leds, Rgb::new(0, 0, 40))),
            ],
        };
        let mut fx = CompositeEffect::new(comp, "two");
        let f = render_at(&mut fx, 0.0);
        // Add blend: red base + blue top.
        assert_eq!(f.get(0), Rgb::new(20, 0, 40));
    }

    #[test]
    fn a_disabled_layer_is_skipped() {
        let leds = 126;
        let mut top = Layer::keyframes("top", solid_anim(leds, Rgb::new(0, 0, 40)));
        top.enabled = false;
        let comp = Composition {
            name: "d".into(),
            description: String::new(),
            leds,
            duration: 1.0,
            loops: true,
            layers: vec![
                Layer::keyframes("base", solid_anim(leds, Rgb::new(20, 0, 0))),
                top,
            ],
        };
        let mut fx = CompositeEffect::new(comp, "d");
        assert_eq!(render_at(&mut fx, 0.0).get(0), Rgb::new(20, 0, 0));
    }

    #[test]
    fn a_generator_layer_bakes_and_animates() {
        let leds = 126;
        let comp = Composition {
            name: "g".into(),
            description: String::new(),
            leds,
            duration: 2.0,
            loops: true,
            layers: vec![Layer::generator(
                "rainbow",
                GenParams {
                    kind: Generator::Rainbow,
                    ..Default::default()
                },
            )],
        };
        let mut fx = CompositeEffect::new(comp, "g");
        let a = render_at(&mut fx, 0.0);
        let b = render_at(&mut fx, 1.0);
        assert!(
            a.iter().any(|c| !c.is_black()),
            "generator lights the board"
        );
        assert_ne!(a, b, "generator animates");
    }

    #[test]
    fn round_trips_through_klx_json() {
        let dir = std::env::temp_dir().join("aula-comp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.klx");
        let comp = Composition::new("demo", 4);
        comp.save(&path).unwrap();
        let back = Composition::load(&path).unwrap();
        assert_eq!(comp, back);
    }
}
