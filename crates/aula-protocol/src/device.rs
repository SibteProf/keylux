//! Device-agnostic types: physical layout, frames, and the `RgbDevice` trait.

use crate::color::Rgb;
use crate::Result;

/// A USB device identity: what `hidapi` matches on.
///
/// Kept as a pair rather than two loose `u16`s because a keyboard and its
/// receiver are two different USB devices, and once there is more than one of
/// them, passing `vid` and `pid` separately is an argument-order bug waiting to
/// happen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeviceId {
    pub vid: u16,
    pub pid: u16,
}

impl DeviceId {
    pub const fn new(vid: u16, pid: u16) -> Self {
        Self { vid, pid }
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vid, self.pid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected a device id like \"258a:010c\"")]
pub struct ParseDeviceIdError;

impl std::str::FromStr for DeviceId {
    type Err = ParseDeviceIdError;

    /// Accepts `258a:010c` and `0x258A:0x010C`. This parses user-editable
    /// settings and command-line arguments, so it is deliberately forgiving
    /// about case and the `0x` prefix and strict about everything else.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (v, p) = s.trim().split_once(':').ok_or(ParseDeviceIdError)?;
        let hex = |t: &str| {
            let t = t.trim();
            let t = t
                .strip_prefix("0x")
                .or_else(|| t.strip_prefix("0X"))
                .unwrap_or(t);
            if t.is_empty() {
                return Err(ParseDeviceIdError);
            }
            u16::from_str_radix(t, 16).map_err(|_| ParseDeviceIdError)
        };
        Ok(Self::new(hex(v)?, hex(p)?))
    }
}

/// How the keyboard is attached.
///
/// This is not cosmetic: it selects the write pacing. The wired floor is
/// measured, the wireless one is not, so the two must stay distinguishable all
/// the way down to the frame gap.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Link {
    #[default]
    Wired,
    Dongle,
    /// Found by a scan of hardware that is not in any known-device list.
    Unknown,
}

impl Link {
    /// The parenthesised half of a device name, e.g. `AULA F75 (wired)`.
    pub const fn suffix(self) -> &'static str {
        match self {
            Link::Wired => "wired",
            Link::Dongle => "2.4 GHz dongle",
            Link::Unknown => "unrecognised link",
        }
    }
}

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

    #[test]
    fn device_id_round_trips_through_text() {
        let id = DeviceId::new(0x258a, 0x010c);
        assert_eq!(id.to_string(), "258a:010c");
        assert_eq!("258a:010c".parse::<DeviceId>().unwrap(), id);
        // Settings files are hand-editable, so tolerate the obvious variants.
        assert_eq!("0x258A:0x010C".parse::<DeviceId>().unwrap(), id);
        assert_eq!(" 258a:010c ".parse::<DeviceId>().unwrap(), id);
    }

    #[test]
    fn malformed_device_ids_are_rejected() {
        for bad in [
            "nonsense",
            "258a",
            "258a:",
            ":010c",
            "258a:010c:1",
            "",
            "zzzz:010c",
        ] {
            assert!(
                bad.parse::<DeviceId>().is_err(),
                "{bad:?} should not parse as a device id"
            );
        }
    }
}
