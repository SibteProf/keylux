//! Motion primitives that **bake into keyframes**.
//!
//! The timeline editor can paint flat keyframes, but hand-painting a travelling
//! sweep or a rainbow is tedious and looks mechanical. A generator samples a
//! parametric motion over the animation's duration and writes the result as
//! ordinary keyframes — so the output is the same plain JSON everything else
//! consumes, with no new playback path and no wireless-streaming implications.
//!
//! Generators are pure: `bake(params, leds, layout, duration)` in, a
//! `Vec<Keyframe>` out. That keeps them trivially testable.

use serde::{Deserialize, Serialize};

use aula_protocol::{KeyPos, Rgb};

use crate::animation::{Ease, HexColor, Keyframe};

/// Serialize an [`Rgb`] as a readable `#rrggbb` string, so generator settings
/// saved inside a composition stay hand-editable — and so `aula-protocol` need
/// not depend on serde.
mod serde_rgb {
    use super::*;
    use serde::Deserializer;

    pub fn serialize<S: serde::Serializer>(c: &Rgb, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&HexColor::from(*c).0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Rgb, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::from_hex(&s).ok_or_else(|| serde::de::Error::custom("expected #rrggbb"))
    }
}

/// Which motion to bake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Generator {
    /// A bar of colour A sliding across a colour-B background.
    Sweep,
    /// A sinusoidal brightness wash of colour A over colour B.
    Wave,
    /// A colour-A→B gradient scrolling across the board.
    GradientScroll,
    /// Concentric rings pulsing out from the board centre.
    Ripple,
    /// A full-spectrum hue that travels across the board.
    Rainbow,
}

impl Generator {
    pub fn label(self) -> &'static str {
        match self {
            Generator::Sweep => "Sweep",
            Generator::Wave => "Wave",
            Generator::GradientScroll => "Gradient scroll",
            Generator::Ripple => "Ripple",
            Generator::Rainbow => "Rainbow",
        }
    }

    /// Does this generator use colour A / colour B? The editor hides the
    /// pickers for generators that ignore them (Rainbow).
    pub fn uses_colors(self) -> bool {
        !matches!(self, Generator::Rainbow)
    }

    /// Does direction apply? Ripple is radial, so it ignores it.
    pub fn uses_direction(self) -> bool {
        !matches!(self, Generator::Ripple)
    }

    pub const ALL: [Generator; 5] = [
        Generator::Sweep,
        Generator::Wave,
        Generator::GradientScroll,
        Generator::Ripple,
        Generator::Rainbow,
    ];
}

/// The axis and sense a linear generator travels along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Direction::LeftToRight => "→ Left to right",
            Direction::RightToLeft => "← Right to left",
            Direction::TopToBottom => "↓ Top to bottom",
            Direction::BottomToTop => "↑ Bottom to top",
        }
    }

    pub const ALL: [Direction; 4] = [
        Direction::LeftToRight,
        Direction::RightToLeft,
        Direction::TopToBottom,
        Direction::BottomToTop,
    ];
}

/// Everything a generator needs besides the layout and duration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenParams {
    pub kind: Generator,
    #[serde(with = "serde_rgb")]
    pub color_a: Rgb,
    #[serde(with = "serde_rgb")]
    pub color_b: Rgb,
    pub direction: Direction,
    /// Feature width as a fraction of the board (sweep bar, ripple ring).
    pub width: f32,
    /// How many full passes across the whole duration.
    pub cycles: f32,
    /// Keyframes to produce. More = smoother but larger files.
    pub frames: usize,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            kind: Generator::Sweep,
            color_a: Rgb::new(255, 40, 120),
            color_b: Rgb::BLACK,
            direction: Direction::LeftToRight,
            width: 0.25,
            cycles: 1.0,
            frames: 24,
        }
    }
}

/// Board extents in the normalised coordinates effects already use elsewhere.
struct Extent {
    max_x: f32,
    max_row: f32,
}

impl Extent {
    fn of(layout: &[KeyPos]) -> Self {
        Self {
            max_x: layout.iter().map(|k| k.x).fold(1.0_f32, f32::max),
            max_row: layout
                .iter()
                .map(|k| f32::from(k.row))
                .fold(1.0_f32, f32::max),
        }
    }

    /// The key's position along the generator's axis, 0.0..=1.0.
    fn along(&self, k: &KeyPos, dir: Direction) -> f32 {
        match dir {
            Direction::LeftToRight => k.x / self.max_x,
            Direction::RightToLeft => 1.0 - k.x / self.max_x,
            Direction::TopToBottom => f32::from(k.row) / self.max_row,
            Direction::BottomToTop => 1.0 - f32::from(k.row) / self.max_row,
        }
    }
}

/// Sample `p` generators over `duration` into keyframes.
///
/// The result is always at least two keyframes so playback has something to
/// interpolate, is sorted by time, and covers `[0, duration)` evenly.
pub fn bake(params: &GenParams, leds: usize, layout: &[KeyPos], duration: f32) -> Vec<Keyframe> {
    let frames = params.frames.clamp(2, 240);
    let duration = duration.max(0.1);
    let ext = Extent::of(layout);

    (0..frames)
        .map(|i| {
            // Phase 0..cycles across the sequence. The last frame lands just
            // before `duration` so the loop wraps cleanly back to frame 0.
            let phase = (i as f32 / frames as f32) * params.cycles;
            let t = (i as f32 / frames as f32) * duration;
            let mut colors = vec![HexColor::from(Rgb::BLACK); leds];
            for k in layout {
                if k.led >= leds {
                    continue;
                }
                let u = ext.along(k, params.direction);
                colors[k.led] = HexColor::from(sample_key(params, &ext, k, u, phase));
            }
            Keyframe {
                t,
                colors,
                ease: Ease::Linear,
            }
        })
        .collect()
}

fn sample_key(p: &GenParams, ext: &Extent, k: &KeyPos, u: f32, phase: f32) -> Rgb {
    match p.kind {
        Generator::Sweep => {
            // A bar centred at `phase` (wrapping) of half-width w/2, with a soft
            // edge so it reads as motion rather than a flashing block.
            let center = phase.rem_euclid(1.0);
            let d = wrapped_dist(u, center);
            let half = (p.width * 0.5).max(0.01);
            let edge = (half * 0.5).max(0.001);
            let on = 1.0 - ((d - half) / edge).clamp(0.0, 1.0);
            lerp_rgb(p.color_b, p.color_a, on)
        }
        Generator::Wave => {
            let s = 0.5 + 0.5 * (std::f32::consts::TAU * (u - phase)).sin();
            lerp_rgb(p.color_b, p.color_a, s)
        }
        Generator::GradientScroll => {
            // Triangle wave so the gradient is seamless as it scrolls.
            let f = triangle((u + phase).rem_euclid(1.0));
            lerp_rgb(p.color_a, p.color_b, f)
        }
        Generator::Rainbow => {
            let hue = (u - phase).rem_euclid(1.0);
            Rgb::from_hsv(hue, 1.0, 1.0)
        }
        Generator::Ripple => {
            // Radial distance from the board centre, normalised so corners sit
            // near 1.0, with rings travelling outward over time.
            let cx = 0.5;
            let cy = 0.5;
            let nx = k.x / ext.max_x;
            let ny = f32::from(k.row) / ext.max_row;
            let r = (((nx - cx).powi(2) + (ny - cy).powi(2)).sqrt() / 0.707).clamp(0.0, 1.0);
            let rings = (1.0 / p.width.max(0.05)).max(1.0);
            let s = 0.5 + 0.5 * (std::f32::consts::TAU * (r * rings - phase)).sin();
            lerp_rgb(p.color_b, p.color_a, s)
        }
    }
}

/// Shortest distance between two points on a 0..1 ring.
fn wrapped_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).abs().rem_euclid(1.0);
    d.min(1.0 - d)
}

/// 0→1→0 over the 0..1 interval.
fn triangle(f: f32) -> f32 {
    if f < 0.5 {
        f * 2.0
    } else {
        (1.0 - f) * 2.0
    }
}

fn lerp_rgb(a: Rgb, b: Rgb, f: f32) -> Rgb {
    let f = f.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * f)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aula_protocol::f75::keymap;

    fn params(kind: Generator) -> GenParams {
        GenParams {
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn bakes_the_requested_number_of_keyframes() {
        let layout = keymap::layout();
        let p = GenParams {
            frames: 16,
            ..params(Generator::Sweep)
        };
        let kfs = bake(&p, 126, &layout, 4.0);
        assert_eq!(kfs.len(), 16);
        // first keyframe at t=0, times strictly increasing and under duration
        assert_eq!(kfs[0].t, 0.0);
        assert!(kfs.windows(2).all(|w| w[0].t < w[1].t));
        assert!(kfs.last().unwrap().t < 4.0);
        assert!(kfs.iter().all(|k| k.colors.len() == 126));
    }

    #[test]
    fn frame_count_is_clamped_to_something_playable() {
        let layout = keymap::layout();
        let p = GenParams {
            frames: 0,
            ..params(Generator::Wave)
        };
        assert!(bake(&p, 126, &layout, 2.0).len() >= 2);
    }

    #[test]
    fn every_generator_actually_moves() {
        let layout = keymap::layout();
        for kind in Generator::ALL {
            let kfs = bake(&params(kind), 126, &layout, 2.0);
            let first = &kfs[0].colors;
            // at least one later keyframe differs from the first
            assert!(
                kfs.iter().skip(1).any(|k| &k.colors != first),
                "{kind:?} produced a static sequence"
            );
        }
    }

    #[test]
    fn rainbow_lights_the_board() {
        let layout = keymap::layout();
        let kfs = bake(&params(Generator::Rainbow), 126, &layout, 2.0);
        assert!(
            kfs[0].colors.iter().any(|c| !c.to_rgb().is_black()),
            "rainbow should light keys"
        );
    }
}
