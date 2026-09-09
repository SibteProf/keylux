//! Flowers opening and fading across the board.
//!
//! Each flower grows from a point: petals push outward, hold, then fade. The
//! petal silhouette comes from modulating the radius by angle, so it reads as
//! a flower rather than an expanding disc. Blooms overlap and blend additively.
//!
//! Births are spread across one cycle and every flower's age is taken modulo
//! that cycle, so the whole garden loops seamlessly with no visible seam.

use aula_protocol::{Frame, Rgb};

use crate::{Effect, EffectMeta, ParamSpec, RenderCtx};

struct Flower {
    x: f32,
    y: f32,
    /// Seconds into the cycle at which this flower starts opening.
    birth: f32,
    hue: f32,
    petals: f32,
    rot: f32,
}

/// Soft pastel palette: pinks, violets, coral, gold.
const HUES: [f32; 7] = [0.95, 0.88, 0.78, 0.62, 0.45, 0.13, 0.05];

/// Small deterministic RNG, so the arrangement is stable between runs and the
/// garden does not rearrange itself every time a slider moves.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0 as f32 / u32::MAX as f32
    }
}

#[derive(Default)]
pub struct Bloom {
    garden: Vec<Flower>,
    /// What the cached garden was grown for; regrown when this changes.
    grown_for: Option<(usize, u32, u32, u32)>,
}

fn grow(count: usize, cycle: f32, max_x: f32, max_row: f32, seed: u32) -> Vec<Flower> {
    let mut r = Rng(seed);
    (0..count)
        .map(|i| Flower {
            x: 0.6 + r.next() * (max_x - 1.2).max(0.1),
            y: 0.4 + r.next() * (max_row - 0.8).max(0.1),
            // Evenly spread but jittered, so blooms do not pulse in lockstep.
            birth: ((i as f32 + r.next() * 0.7) / count as f32) * cycle,
            hue: HUES[(r.next() * HUES.len() as f32) as usize % HUES.len()],
            petals: 4.0 + (r.next() * 2.0).floor(),
            rot: r.next() * std::f32::consts::TAU,
        })
        .collect()
}

impl Effect for Bloom {
    fn meta(&self) -> EffectMeta {
        EffectMeta {
            id: "bloom".into(),
            name: "Bloom".into(),
            description: "Flowers opening and fading across the keys.".into(),
            params: vec![
                ParamSpec::int("count", "Flowers per loop", 3, 40, 14),
                ParamSpec::float("cycle", "Loop length (s)", 2.0, 30.0, 9.0),
                ParamSpec::float("life", "Flower life (s)", 0.5, 8.0, 3.2),
                ParamSpec::float("size", "Petal radius", 0.5, 6.0, 2.4),
                ParamSpec::float("wash", "Background glow", 0.0, 60.0, 0.0),
                ParamSpec::float("brightness", "Brightness", 0.0, 1.0, 1.0),
                ParamSpec::int("seed", "Arrangement", 0, 999, 908),
            ],
        }
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        let count = ctx.params.int("count", 14).clamp(1, 200) as usize;
        let cycle = ctx.params.float("cycle", 9.0).max(0.5);
        let life = ctx.params.float("life", 3.2).max(0.1);
        let size = ctx.params.float("size", 2.4).max(0.1);
        let wash = ctx.params.float("wash", 0.0);
        let brightness = ctx.params.float("brightness", 1.0);
        let seed = ctx.params.int("seed", 908) as u32;

        // Regrow only when the arrangement actually depends on something that
        // changed — otherwise dragging the brightness slider would reshuffle
        // every flower mid-bloom.
        let key = (count, cycle.to_bits(), seed, ctx.max_x.to_bits());
        if self.grown_for != Some(key) {
            self.garden = grow(count, cycle, ctx.max_x, ctx.max_row, seed.wrapping_add(1));
            self.grown_for = Some(key);
        }

        let mut acc = vec![(0.0f32, 0.0f32, 0.0f32); ctx.layout.len()];

        for f in &self.garden {
            let age = (ctx.t - f.birth).rem_euclid(cycle);
            if age > life {
                continue;
            }
            let p = age / life;
            // Radius eases out: fast open, gentle settle.
            let radius = size * (1.0 - (1.0 - p).powf(2.2));
            // Fade in quickly, hold, fade out slowly.
            let alpha = if p < 0.18 {
                p / 0.18
            } else if p > 0.55 {
                (1.0 - (p - 0.55) / 0.45).powf(1.5)
            } else {
                1.0
            };
            if alpha <= 0.0 {
                continue;
            }

            for (i, k) in ctx.layout.iter().enumerate() {
                let dx = k.x - f.x;
                let dy = f32::from(k.row) - f.y;
                let d = dx.hypot(dy);
                if d > radius * 1.05 {
                    continue;
                }
                let theta = dy.atan2(dx);
                let petal_r = radius * (0.66 + 0.34 * (f.petals * theta + f.rot).cos());
                if d > petal_r {
                    continue;
                }

                let falloff = (1.0 - d / petal_r.max(0.001)).powf(0.85);
                let v = falloff * alpha * brightness;

                // Warm pale centre grading out into the petal hue.
                let centre = (1.0 - d / (radius * 0.42).max(0.001)).max(0.0).powf(1.6);
                let sat = 0.15 + 0.85 * (1.0 - centre);
                let hue = f.hue * (1.0 - centre * 0.55) + 0.12 * (centre * 0.55);
                let c = Rgb::from_hsv(hue, sat, v);

                acc[i].0 += f32::from(c.r);
                acc[i].1 += f32::from(c.g);
                acc[i].2 += f32::from(c.b);
            }
        }

        // These LEDs are bright even at very low values, so the wash defaults
        // to zero: even 10/255 on every key drowns the flowers out.
        for (i, k) in ctx.layout.iter().enumerate() {
            let (r, g, b) = acc[i];
            out.set(
                k.led,
                Rgb::new(
                    (r + wash * 0.2).min(255.0) as u8,
                    (g + wash * 0.6).min(255.0) as u8,
                    (b + wash).min(255.0) as u8,
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Params;
    use aula_protocol::f75::keymap;

    fn render_at(b: &mut Bloom, t: f32, params: &Params) -> Frame {
        let layout = keymap::layout();
        let ctx = RenderCtx {
            t,
            layout: &layout,
            max_x: keymap::max_x(&layout),
            max_row: f32::from(keymap::max_row(&layout)),
            params,
        };
        let mut f = Frame::black(126);
        b.render(&ctx, &mut f);
        f
    }

    #[test]
    fn flowers_appear_and_change() {
        let params = Params::from_specs(&Bloom::default().meta().params);
        let mut b = Bloom::default();
        let a = render_at(&mut b, 1.0, &params);
        let c = render_at(&mut b, 2.5, &params);
        assert!(a.iter().any(|c| !c.is_black()), "something must bloom");
        assert_ne!(a, c, "the garden must animate");
    }

    /// One full cycle later the garden must look the same, or the loop shows a
    /// visible jump every time it wraps.
    #[test]
    fn the_loop_is_seamless() {
        let params = Params::from_specs(&Bloom::default().meta().params);
        let cycle = params.float("cycle", 9.0);
        let mut b = Bloom::default();
        let a = render_at(&mut b, 2.0, &params);
        let c = render_at(&mut b, 2.0 + cycle, &params);
        assert_eq!(a, c);
    }
}
