//! Effect engine.
//!
//! An effect is a pure function of time: given `t` and the keyboard's physical
//! layout, produce a [`Frame`]. That purity is what makes effects trivially
//! scriptable — a Rhai script implements the same contract as a built-in.
//!
//! Effects **declare** their parameters via [`ParamSpec`], and the GUI builds
//! controls from that declaration. A newly dropped-in script therefore gets a
//! full set of sliders and colour pickers without any UI code.

pub mod animation;
pub mod builtin;
pub mod import;
pub mod params;
pub mod registry;
pub mod script;

pub use animation::{Animation, AnimationEffect, Keyframe};
pub use params::{ParamKind, ParamSpec, Params, Value};
pub use registry::Registry;
pub use script::{ScriptEffect, ScriptError};

use aula_protocol::{Frame, KeyPos};

/// What an effect is called and what knobs it exposes.
#[derive(Clone, Debug)]
pub struct EffectMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    pub params: Vec<ParamSpec>,
}

/// Everything an effect needs to render one frame.
pub struct RenderCtx<'a> {
    /// Seconds since the effect started. Animation phase should come from this
    /// rather than a frame counter, so effects run at the same real-world speed
    /// regardless of frame rate.
    pub t: f32,
    /// Physical key geometry: position, size, and LED index.
    pub layout: &'a [KeyPos],
    /// Widest key centre, for normalising across the board.
    pub max_x: f32,
    /// Bottom row index.
    pub max_row: f32,
    /// Current parameter values.
    pub params: &'a Params,
}

impl RenderCtx<'_> {
    /// Normalised horizontal position, 0.0 at the left edge, 1.0 at the right.
    pub fn nx(&self, k: &KeyPos) -> f32 {
        if self.max_x > 0.0 {
            k.x / self.max_x
        } else {
            0.0
        }
    }

    /// Normalised vertical position, 0.0 top row, 1.0 bottom row.
    pub fn ny(&self, k: &KeyPos) -> f32 {
        if self.max_row > 0.0 {
            f32::from(k.row) / self.max_row
        } else {
            0.0
        }
    }
}

/// A renderable effect.
pub trait Effect: Send {
    fn meta(&self) -> EffectMeta;

    /// Draw one frame. `out` arrives cleared to black.
    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame);
}

/// Keys wide enough that lighting them as a single "pixel" swamps a pattern.
///
/// The F75's spacebar is one LED spanning about six columns. Text and other
/// structured effects skip these; smooth washes like the wave do not care.
pub const WIDE_KEY_THRESHOLD: f32 = 3.0;

pub fn is_wide(k: &KeyPos) -> bool {
    k.w >= WIDE_KEY_THRESHOLD
}
