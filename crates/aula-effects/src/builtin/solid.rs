//! A single colour across the whole board.

use aula_protocol::{Frame, Rgb};

use crate::{Effect, EffectMeta, ParamSpec, RenderCtx};

#[derive(Default)]
pub struct Solid;

impl Effect for Solid {
    fn meta(&self) -> EffectMeta {
        EffectMeta {
            id: "solid".into(),
            name: "Solid".into(),
            description: "One colour across every key.".into(),
            params: vec![
                ParamSpec::color("color", "Colour", Rgb::new(255, 0, 0)),
                ParamSpec::float("brightness", "Brightness", 0.0, 1.0, 1.0),
            ],
        }
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        let c = ctx
            .params
            .color("color", Rgb::new(255, 0, 0))
            .scale(ctx.params.float("brightness", 1.0));
        // Every mapped key, so unmapped slots stay black.
        for k in ctx.layout {
            out.set(k.led, c);
        }
    }
}
