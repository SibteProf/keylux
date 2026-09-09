//! Built-in effects, ported from the TypeScript prototype.

mod bloom;
mod solid;
mod sweep;
mod text;
mod wave;

pub use bloom::Bloom;
pub use solid::Solid;
pub use sweep::Sweep;
pub use text::Text;
pub use wave::Wave;

use crate::Effect;

/// Every built-in, freshly constructed.
pub fn all() -> Vec<Box<dyn Effect>> {
    vec![
        Box::new(Solid),
        Box::new(Wave),
        Box::new(Sweep),
        Box::new(Bloom::default()),
        Box::new(Text),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Ids address effects from the CLI and from saved selections, so a
    /// duplicate would silently shadow one of them.
    #[test]
    fn ids_are_unique() {
        let ids: Vec<String> = all().iter().map(|e| e.meta().id).collect();
        let unique: HashSet<&String> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "duplicate built-in id in {ids:?}");
    }
}
