//! A bright bar sweeping left to right over a dark background.
//!
//! Deliberately unambiguous: a bar only *looks* like a bar if the LED map is
//! spatially correct, so this doubles as a map check — on a scrambled map it
//! renders as flicker rather than a moving edge.

use aula_protocol::{Frame, Rgb};

use crate::{Effect, EffectMeta, ParamSpec, RenderCtx};

#[derive(Default)]
pub struct Sweep;

impl Effect for Sweep {
    fn meta(&self) -> EffectMeta {
        EffectMeta {
            id: "sweep".into(),
            name: "Sweep".into(),
            description: "A bar of light crossing the board. Good for checking the LED map.".into(),
            params: vec![
                ParamSpec::float("seconds", "Seconds per pass", 0.3, 6.0, 1.6),
                ParamSpec::float("width", "Bar width", 0.04, 0.5, 0.16),
                ParamSpec::color("color", "Colour", Rgb::new(255, 255, 230)),
                ParamSpec::color("background", "Background", Rgb::new(0, 0, 40)),
                ParamSpec::float("brightness", "Brightness", 0.0, 1.0, 1.0),
            ],
        }
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        let seconds = ctx.params.float("seconds", 1.6).max(0.05);
        let width = ctx.params.float("width", 0.16).max(0.01);
        let color = ctx.params.color("color", Rgb::new(255, 255, 230));
        let background = ctx.params.color("background", Rgb::new(0, 0, 40));
        let brightness = ctx.params.float("brightness", 1.0);

        // Start and finish fully off the board, so the bar enters and leaves
        // rather than popping into existence at the edges.
        let head = (ctx.t / seconds).rem_euclid(1.0) * (1.0 + 2.0 * width) - width;

        for k in ctx.layout {
            let d = (ctx.nx(k) - head).abs();
            // Squared falloff: a soft edge still reads as one bar, where a
            // linear ramp smears into a general glow.
            let v = if d < width {
                (1.0 - d / width).powi(2)
            } else {
                0.0
            };
            let lit = color.scale(v * brightness);
            let bg = background.scale((1.0 - v) * brightness);
            out.set(k.led, lit.blend_add(bg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Params;
    use aula_protocol::f75::keymap;

    fn frame_at(t: f32) -> (Frame, Vec<aula_protocol::KeyPos>) {
        let layout = keymap::layout();
        let params = Params::from_specs(&Sweep.meta().params);
        let ctx = RenderCtx {
            t,
            layout: &layout,
            max_x: keymap::max_x(&layout),
            max_row: f32::from(keymap::max_row(&layout)),
            params: &params,
        };
        let mut f = Frame::black(126);
        Sweep.render(&ctx, &mut f);
        (f, layout)
    }

    /// The point of this effect is that the bright region is *localised*. If
    /// it ever lit the whole board it would stop being a map check.
    #[test]
    fn the_bar_is_narrow_and_moves() {
        // Mid-pass on both counts: the bar deliberately starts and ends fully
        // off the board, so t = 0 lights nothing at all.
        let (a, layout) = frame_at(0.8);
        let (b, _) = frame_at(0.4);

        let bright = |f: &Frame| {
            layout
                .iter()
                .filter(|k| f.get(k.led).r > 200)
                .map(|k| k.led)
                .collect::<Vec<_>>()
        };
        let ba = bright(&a);
        assert!(!ba.is_empty(), "something must be lit");
        assert!(
            ba.len() < layout.len() / 2,
            "the bar must not cover the board"
        );
        assert_ne!(ba, bright(&b), "the bar must travel");
    }
}
