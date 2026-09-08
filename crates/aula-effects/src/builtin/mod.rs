//! Built-in effects, ported from the TypeScript prototype.

mod solid;
mod wave;

pub use solid::Solid;
pub use wave::Wave;

use crate::Effect;

/// Every built-in, freshly constructed.
pub fn all() -> Vec<Box<dyn Effect>> {
    vec![Box::new(Solid), Box::new(Wave)]
}
