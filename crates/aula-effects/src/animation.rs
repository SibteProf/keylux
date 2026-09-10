//! Keyframe animations.
//!
//! An animation is a list of keyframes: a time, and a colour for every LED.
//! Playback interpolates between them.
//!
//! This is the shared foundation for two features that look different in the
//! UI but are the same data underneath:
//!
//! * the **timeline editor** — the user paints keys and sets keyframes
//! * **imported frame sequences** — a GIF or image folder becomes one keyframe
//!   per source frame
//!
//! Animations are plain JSON in the `animations/` directory, so they can be
//! shared, diffed and hand-edited.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use aula_protocol::{Frame, Rgb};

use crate::{Effect, EffectMeta, ParamSpec, RenderCtx};

/// A colour stored as `#rrggbb`, so animation files stay readable and
/// hand-editable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HexColor(pub String);

impl From<Rgb> for HexColor {
    fn from(c: Rgb) -> Self {
        HexColor(format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b))
    }
}

impl HexColor {
    pub fn to_rgb(&self) -> Rgb {
        Rgb::from_hex(&self.0).unwrap_or(Rgb::BLACK)
    }
}

/// How the transition *out of* a keyframe is shaped over time.
///
/// The ease is applied to the interpolation fraction between this keyframe and
/// the next, so each keyframe owns the curve of the segment that begins at it.
/// [`Ease::Hold`] makes just that one segment a hard cut — something the global
/// [`Animation::interpolate`] switch cannot express, since it is all-or-nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ease {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Hold this keyframe until the next, then cut. A per-keyframe hard cut.
    Hold,
}

impl Ease {
    /// Map a linear 0.0..=1.0 fraction through the easing curve.
    pub fn apply(self, f: f32) -> f32 {
        let f = f.clamp(0.0, 1.0);
        match self {
            Ease::Linear => f,
            Ease::EaseIn => f * f,
            Ease::EaseOut => f * (2.0 - f),
            Ease::EaseInOut => {
                if f < 0.5 {
                    2.0 * f * f
                } else {
                    -1.0 + (4.0 - 2.0 * f) * f
                }
            }
            Ease::Hold => 0.0,
        }
    }

    /// Short label for the editor's dropdown.
    pub fn label(self) -> &'static str {
        match self {
            Ease::Linear => "Linear",
            Ease::EaseIn => "Ease in",
            Ease::EaseOut => "Ease out",
            Ease::EaseInOut => "Ease in-out",
            Ease::Hold => "Hold (cut)",
        }
    }

    pub const ALL: [Ease; 5] = [
        Ease::Linear,
        Ease::EaseIn,
        Ease::EaseOut,
        Ease::EaseInOut,
        Ease::Hold,
    ];
}

/// One moment in an animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Seconds from the start of the animation.
    pub t: f32,
    /// One colour per LED index.
    pub colors: Vec<HexColor>,
    /// Easing for the transition out of this keyframe. Defaults to linear so
    /// animations authored before easing existed load unchanged.
    #[serde(default)]
    pub ease: Ease,
}

impl Keyframe {
    pub fn black(t: f32, leds: usize) -> Self {
        Self {
            t,
            colors: vec![HexColor::from(Rgb::BLACK); leds],
            ease: Ease::default(),
        }
    }

    pub fn from_frame(t: f32, frame: &Frame) -> Self {
        Self {
            t,
            colors: frame.iter().map(|c| HexColor::from(*c)).collect(),
            ease: Ease::default(),
        }
    }

    pub fn color(&self, led: usize) -> Rgb {
        self.colors
            .get(led)
            .map(|c| c.to_rgb())
            .unwrap_or(Rgb::BLACK)
    }
}

/// A complete animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Animation {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Which device this was authored for, as a sanity check.
    #[serde(default = "default_device")]
    pub device: String,
    /// Number of LED slots the keyframes cover.
    pub leds: usize,
    /// Total length in seconds. Playback wraps at this point.
    pub duration: f32,
    #[serde(default = "yes")]
    pub loops: bool,
    /// Blend between keyframes, or cut hard between them.
    #[serde(default = "yes")]
    pub interpolate: bool,
    pub keyframes: Vec<Keyframe>,
}

fn default_device() -> String {
    "aula-f75".into()
}
fn yes() -> bool {
    true
}

impl Animation {
    /// A new, empty animation: two keyframes so there is something to edit.
    pub fn new(name: &str, leds: usize) -> Self {
        Self {
            name: name.to_string(),
            description: String::new(),
            device: default_device(),
            leds,
            duration: 2.0,
            loops: true,
            interpolate: true,
            keyframes: vec![Keyframe::black(0.0, leds), Keyframe::black(2.0, leds)],
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, AnimationError> {
        let text = std::fs::read_to_string(path.as_ref())?;
        let mut anim: Animation = serde_json::from_str(&text)?;
        anim.normalise();
        Ok(anim)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), AnimationError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path.as_ref(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Keep the data sane after loading or editing: sorted keyframes, sensible
    /// duration, no zero-length animation.
    pub fn normalise(&mut self) {
        self.keyframes
            .sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        if self.keyframes.is_empty() {
            self.keyframes.push(Keyframe::black(0.0, self.leds));
        }
        let last = self.keyframes.last().map(|k| k.t).unwrap_or(0.0);
        if self.duration <= 0.0 || self.duration < last {
            self.duration = (last + 0.5).max(0.1);
        }
    }

    /// Sample the animation at `t` seconds.
    pub fn sample(&self, t: f32, out: &mut Frame) {
        if self.keyframes.is_empty() {
            return;
        }
        let t = if self.loops {
            t.rem_euclid(self.duration.max(0.001))
        } else {
            t.min(self.duration)
        };

        // Last keyframe at or before t.
        let idx = self.keyframes.iter().rposition(|k| k.t <= t).unwrap_or(0);
        let a = &self.keyframes[idx];

        if !self.interpolate {
            write_kf(a, out);
            return;
        }

        // Next keyframe, wrapping to the first if we are past the end.
        let next = self.keyframes.get(idx + 1);
        let (b, span) = match next {
            Some(b) => (b, b.t - a.t),
            None if self.loops => (&self.keyframes[0], self.duration - a.t),
            None => {
                write_kf(a, out);
                return;
            }
        };

        if span <= 0.0 {
            write_kf(a, out);
            return;
        }

        // The segment's curve is owned by the keyframe it starts from. `Hold`
        // collapses to 0, holding `a` until the cut at `b`.
        let f = a.ease.apply(((t - a.t) / span).clamp(0.0, 1.0));
        for led in 0..self.leds {
            out.set(led, lerp_rgb(a.color(led), b.color(led), f));
        }
    }
}

fn write_kf(k: &Keyframe, out: &mut Frame) {
    for (led, c) in k.colors.iter().enumerate() {
        out.set(led, c.to_rgb());
    }
}

fn lerp_rgb(a: Rgb, b: Rgb, f: f32) -> Rgb {
    let m = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * f)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// Plays an [`Animation`] as an effect.
pub struct AnimationEffect {
    anim: Animation,
    id: String,
    path: Option<PathBuf>,
}

impl AnimationEffect {
    pub fn new(anim: Animation, id: impl Into<String>) -> Self {
        Self {
            anim,
            id: id.into(),
            path: None,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, AnimationError> {
        let path = path.as_ref().to_path_buf();
        let anim = Animation::load(&path)?;
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| anim.name.clone());
        Ok(Self {
            anim,
            id,
            path: Some(path),
        })
    }

    pub fn animation(&self) -> &Animation {
        &self.anim
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl Effect for AnimationEffect {
    fn meta(&self) -> EffectMeta {
        EffectMeta {
            id: self.id.clone(),
            name: self.anim.name.clone(),
            description: if self.anim.description.is_empty() {
                format!(
                    "{} keyframes over {:.1}s",
                    self.anim.keyframes.len(),
                    self.anim.duration
                )
            } else {
                self.anim.description.clone()
            },
            params: vec![
                ParamSpec::float("speed", "Speed", 0.1, 4.0, 1.0),
                ParamSpec::float("brightness", "Brightness", 0.0, 1.0, 1.0),
            ],
        }
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        let speed = ctx.params.float("speed", 1.0);
        let brightness = ctx.params.float("brightness", 1.0);
        self.anim.sample(ctx.t * speed, out);
        if brightness < 1.0 {
            out.scale(brightness);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AnimationError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid animation file: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_frame(leds: usize) -> Animation {
        let mut a = Animation::new("Test", leds);
        a.duration = 2.0;
        a.keyframes = vec![
            Keyframe {
                t: 0.0,
                colors: vec![HexColor::from(Rgb::new(0, 0, 0)); leds],
                ease: Ease::Linear,
            },
            Keyframe {
                t: 1.0,
                colors: vec![HexColor::from(Rgb::new(255, 0, 0)); leds],
                ease: Ease::Linear,
            },
        ];
        a
    }

    #[test]
    fn interpolates_between_keyframes() {
        let a = two_frame(4);
        let mut f = Frame::black(4);

        a.sample(0.0, &mut f);
        assert_eq!(f.get(0), Rgb::new(0, 0, 0));

        a.sample(0.5, &mut f);
        let mid = f.get(0);
        assert!(
            mid.r > 100 && mid.r < 160,
            "halfway should be mid-red, got {mid:?}"
        );

        a.sample(1.0, &mut f);
        assert_eq!(f.get(0), Rgb::new(255, 0, 0));
    }

    #[test]
    fn hard_cut_when_interpolation_is_off() {
        let mut a = two_frame(2);
        a.interpolate = false;
        let mut f = Frame::black(2);
        a.sample(0.5, &mut f);
        assert_eq!(
            f.get(0),
            Rgb::new(0, 0, 0),
            "should hold the earlier keyframe"
        );
    }

    #[test]
    fn loops_back_to_the_first_keyframe() {
        let a = two_frame(2);
        let mut f = Frame::black(2);
        // 1.0 -> 2.0 interpolates from red back to the first keyframe (black)
        a.sample(1.5, &mut f);
        let mid = f.get(0);
        assert!(
            mid.r > 100 && mid.r < 160,
            "should be fading back, got {mid:?}"
        );
        // wrapping past the end returns to the start
        a.sample(2.0, &mut f);
        assert_eq!(f.get(0), Rgb::new(0, 0, 0));
    }

    #[test]
    fn round_trips_through_json() {
        let dir = std::env::temp_dir().join("aula-anim-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.json");

        let a = two_frame(3);
        a.save(&path).unwrap();
        let b = Animation::load(&path).unwrap();
        assert_eq!(a.keyframes, b.keyframes);
        assert_eq!(a.duration, b.duration);

        // stored in a readable form
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("#ff0000"), "colours should be hex strings");
    }

    #[test]
    fn normalise_sorts_and_fixes_duration() {
        let mut a = Animation::new("x", 2);
        a.duration = 0.0;
        a.keyframes = vec![Keyframe::black(3.0, 2), Keyframe::black(1.0, 2)];
        a.normalise();
        assert!(a.keyframes[0].t < a.keyframes[1].t, "keyframes get sorted");
        assert!(a.duration >= 3.0, "duration covers the last keyframe");
    }

    #[test]
    fn ease_curves_map_endpoints_and_bend_the_middle() {
        for e in Ease::ALL {
            assert_eq!(e.apply(0.0), 0.0, "{e:?} starts at 0");
            if e != Ease::Hold {
                assert!((e.apply(1.0) - 1.0).abs() < 1e-6, "{e:?} ends at 1");
            }
        }
        // ease-in is slower than linear at the midpoint, ease-out faster.
        assert!(Ease::EaseIn.apply(0.5) < 0.5);
        assert!(Ease::EaseOut.apply(0.5) > 0.5);
        // hold stays at the source keyframe for the whole segment.
        assert_eq!(Ease::Hold.apply(0.9), 0.0);
    }

    #[test]
    fn per_keyframe_hold_cuts_while_neighbours_blend() {
        let mut a = two_frame(1);
        a.keyframes[0].ease = Ease::Hold;
        let mut f = Frame::black(1);
        // With Hold on the first keyframe, halfway still shows the source colour
        // even though the global interpolate switch is on.
        a.sample(0.5, &mut f);
        assert_eq!(f.get(0), Rgb::new(0, 0, 0), "held until the cut");
    }

    #[test]
    fn loads_json_without_an_ease_field() {
        // A file written before easing existed has no `ease` key; it must load
        // and default to linear rather than failing to parse.
        let json = r##"{
            "name": "legacy",
            "leds": 2,
            "duration": 1.0,
            "keyframes": [
                { "t": 0.0, "colors": ["#000000", "#000000"] },
                { "t": 1.0, "colors": ["#ffffff", "#ffffff"] }
            ]
        }"##;
        let a: Animation = serde_json::from_str(json).unwrap();
        assert_eq!(a.keyframes[0].ease, Ease::Linear);
    }

    #[test]
    fn effect_playback_animates() {
        use crate::Params;
        use aula_protocol::f75::keymap;

        let layout = keymap::layout();
        let mut fx = AnimationEffect::new(two_frame(126), "test");
        let params = Params::from_specs(&fx.meta().params);

        let render = |fx: &mut AnimationEffect, t: f32| {
            let ctx = RenderCtx {
                t,
                layout: &layout,
                max_x: keymap::max_x(&layout),
                max_row: f32::from(keymap::max_row(&layout)),
                params: &params,
            };
            let mut f = Frame::black(126);
            fx.render(&ctx, &mut f);
            f
        };

        assert_ne!(render(&mut fx, 0.0), render(&mut fx, 1.0));
    }
}
