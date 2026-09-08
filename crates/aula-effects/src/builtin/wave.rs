//! Travelling rainbow wave.
//!
//! Sampled from each key's physical position, so it sweeps smoothly across the
//! board rather than jumping around in wiring order. Phase comes from wall
//! clock time, so the wave travels at the same real-world speed at any frame
//! rate.

use aula_protocol::{Frame, Rgb};

use crate::{Effect, EffectMeta, ParamSpec, RenderCtx};

#[derive(Default)]
pub struct Wave;

impl Effect for Wave {
    fn meta(&self) -> EffectMeta {
        EffectMeta {
            id: "wave".into(),
            name: "Wave".into(),
            description: "A rainbow travelling across the keys.".into(),
            params: vec![
                ParamSpec::float("speed", "Speed", 0.0, 2.0, 0.35),
                ParamSpec::float("cycles", "Colour cycles", 0.25, 4.0, 1.2),
                ParamSpec::float("tilt", "Diagonal tilt", -1.0, 1.0, 0.25),
                ParamSpec::float("brightness", "Brightness", 0.0, 1.0, 1.0),
            ],
        }
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        let speed = ctx.params.float("speed", 0.35);
        let cycles = ctx.params.float("cycles", 1.2);
        let tilt = ctx.params.float("tilt", 0.25);
        let brightness = ctx.params.float("brightness", 1.0);

        for k in ctx.layout {
            let phase = ctx.nx(k) * cycles + ctx.ny(k) * tilt - ctx.t * speed;
            out.set(k.led, Rgb::from_hsv(phase, 1.0, brightness));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Params;
    use aula_protocol::f75::keymap;

    #[test]
    fn wave_lights_every_mapped_key_and_moves() {
        let layout = keymap::layout();
        let params = Params::from_specs(&Wave.meta().params);
        let mut w = Wave;

        let render_at = |w: &mut Wave, t: f32| {
            let ctx = RenderCtx {
                t,
                layout: &layout,
                max_x: keymap::max_x(&layout),
                max_row: f32::from(keymap::max_row(&layout)),
                params: &params,
            };
            let mut f = Frame::black(126);
            w.render(&ctx, &mut f);
            f
        };

        let a = render_at(&mut w, 0.0);
        let b = render_at(&mut w, 1.0);

        assert!(layout.iter().all(|k| !a.get(k.led).is_black()));
        assert_ne!(a, b, "the wave must actually animate");
    }
}
