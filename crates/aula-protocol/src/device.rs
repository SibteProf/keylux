//! Device-agnostic types: physical layout, frames, and the `RgbDevice` trait.

use crate::color::Rgb;
use crate::Result;

/// One physical key: where it sits on the board, and which LED index drives it.
///
/// `x` is the key centre in 1u units from the left edge, `row` is the row from
/// the top. Effects sample these so animations travel across the board in real
/// space rather than in wiring order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyPos {
    pub name: &'static str,
    pub row: u8,
    pub x: f32,
    /// Key width in 1u units. Effects use this to skip oversized keys — a
    /// spacebar is a single LED spanning ~6 columns, and lighting it as one
    /// "pixel" swamps anything drawn around it.
    pub w: f32,
    pub led: usize,
}

/// A full frame of colours, indexed by LED slot.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    leds: Vec<Rgb>,
}

impl Frame {
    pub fn black(len: usize) -> Self {
        Self {
            leds: vec![Rgb::BLACK; len],
        }
    }

    pub fn solid(len: usize, c: Rgb) -> Self {
        Self { leds: vec![c; len] }
    }

    pub fn len(&self) -> usize {
        self.leds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.leds.is_empty()
    }

    pub fn as_slice(&self) -> &[Rgb] {
        &self.leds
    }

    /// Out-of-range writes are ignored rather than panicking: effects are often
    /// user scripts, and a bad index should not take the app down.
    pub fn set(&mut self, led: usize, c: Rgb) {
        if let Some(slot) = self.leds.get_mut(led) {
            *slot = c;
        }
    }

    pub fn get(&self, led: usize) -> Rgb {
        self.leds.get(led).copied().unwrap_or(Rgb::BLACK)
    }

    /// Additive blend into a slot, for overlapping effects.
    pub fn add(&mut self, led: usize, c: Rgb) {
        if let Some(slot) = self.leds.get_mut(led) {
            *slot = slot.blend_add(c);
        }
    }

    pub fn fill(&mut self, c: Rgb) {
        self.leds.fill(c);
    }

    pub fn scale(&mut self, f: f32) {
        for c in &mut self.leds {
            *c = c.scale(f);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Rgb> {
        self.leds.iter()
    }
}

/// An RGB keyboard this app can drive.
///
/// Static and streaming are deliberately separate methods rather than one
/// `write(frame)`. On the F75 they are genuinely different commands with
/// different payload layouts, and using the static path for animation blanks
/// the whole board — so the type system should not let them be confused.
pub trait RgbDevice {
    /// Human-readable device name, for the UI.
    fn name(&self) -> &str;

    /// Number of addressable LED slots. Not all map to physical keys.
    fn led_count(&self) -> usize;

    /// Physical geometry, one entry per real key.
    fn layout(&self) -> &[KeyPos];

    /// Highest frame rate that is safe to sustain.
    ///
    /// This is a hardware limit, not a preference: writing faster starves the
    /// keyboard's key-scanning loop and it stops responding to keypresses.
    fn max_fps(&self) -> u32;

    /// Put the board into the mode where host per-key colours are rendered.
    ///
    /// Returns `true` if a change was actually written. This may touch
    /// non-volatile config, so call it once before streaming — never per frame.
    fn ensure_per_key_mode(&mut self) -> Result<bool>;

    /// Write a single frame that persists. Not for animation.
    fn set_static(&mut self, frame: &Frame) -> Result<()>;

    /// Push one animation frame. Safe to call repeatedly at up to `max_fps`.
    ///
    /// Implementations may skip a frame identical to the one already on the
    /// device, so calling this in a tight loop with a static image is cheap.
    fn stream(&mut self, frame: &Frame) -> Result<()>;

    /// Ask for a slower frame rate than the hardware ceiling.
    ///
    /// Only downward: `max_fps` is a hardware limit. Useful when a user would
    /// rather have less bus traffic than smoother animation. Devices with no
    /// rate control ignore it.
    fn set_max_fps(&mut self, fps: u32) {
        let _ = fps;
    }
}

/// Convenience: a frame sized for a device.
pub fn frame_for(dev: &dyn RgbDevice) -> Frame {
    Frame::black(dev.led_count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_set_is_ignored() {
        let mut f = Frame::black(4);
        f.set(99, Rgb::WHITE);
        assert!(f.iter().all(|c| c.is_black()));
    }

    #[test]
    fn additive_blend_saturates() {
        let mut f = Frame::black(1);
        f.add(0, Rgb::new(200, 0, 0));
        f.add(0, Rgb::new(100, 0, 0));
        assert_eq!(f.get(0), Rgb::new(255, 0, 0));
    }
}
