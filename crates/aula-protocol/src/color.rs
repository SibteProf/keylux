//! Colour type and helpers.

/// An 8-bit-per-channel colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
    pub const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// From floats in 0.0..=1.0, clamped.
    pub fn from_f32(r: f32, g: f32, b: f32) -> Self {
        Self {
            r: clamp_u8(r * 255.0),
            g: clamp_u8(g * 255.0),
            b: clamp_u8(b * 255.0),
        }
    }

    /// HSV to RGB. `h` wraps, `s` and `v` are clamped to 0.0..=1.0.
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(1.0);
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);

        let i = (h * 6.0).floor();
        let f = h * 6.0 - i;
        let p = v * (1.0 - s);
        let q = v * (1.0 - f * s);
        let t = v * (1.0 - (1.0 - f) * s);

        let (r, g, b) = match (i as i32).rem_euclid(6) {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Self::from_f32(r, g, b)
    }

    /// RGB to HSV: hue in 0.0..1.0 (wrapping), saturation and value 0.0..=1.0.
    ///
    /// The inverse of [`Rgb::from_hsv`]. Grey has no meaningful hue, so it
    /// reports 0.
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let r = f32::from(self.r) / 255.0;
        let g = f32::from(self.g) / 255.0;
        let b = f32::from(self.b) / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;

        let h = if d == 0.0 {
            0.0
        } else if max == r {
            (((g - b) / d) % 6.0) / 6.0
        } else if max == g {
            (((b - r) / d) + 2.0) / 6.0
        } else {
            (((r - g) / d) + 4.0) / 6.0
        };
        let s = if max == 0.0 { 0.0 } else { d / max };
        (h.rem_euclid(1.0), s, max)
    }

    /// Parse `#rrggbb` or `rrggbb`.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        Some(Self {
            r: u8::from_str_radix(&s[0..2], 16).ok()?,
            g: u8::from_str_radix(&s[2..4], 16).ok()?,
            b: u8::from_str_radix(&s[4..6], 16).ok()?,
        })
    }

    /// Multiply every channel by `f`.
    pub fn scale(self, f: f32) -> Self {
        Self {
            r: clamp_u8(self.r as f32 * f),
            g: clamp_u8(self.g as f32 * f),
            b: clamp_u8(self.b as f32 * f),
        }
    }

    /// Additive blend, saturating at 255. Used where effects overlap.
    ///
    /// Named `blend_add` rather than `add` so it is not mistaken for
    /// `std::ops::Add`, which would imply wrapping arithmetic.
    pub fn blend_add(self, o: Rgb) -> Self {
        Self {
            r: self.r.saturating_add(o.r),
            g: self.g.saturating_add(o.g),
            b: self.b.saturating_add(o.b),
        }
    }

    pub fn is_black(self) -> bool {
        self.r == 0 && self.g == 0 && self.b == 0
    }
}

fn clamp_u8(v: f32) -> u8 {
    if v.is_nan() {
        0
    } else {
        v.round().clamp(0.0, 255.0) as u8
    }
}

/// Physical order of the three colour bytes for a device.
///
/// The F75 is plain RGB — proven by decoding the vendor driver's own payload,
/// which yields exact primaries and secondaries at stride 3. Other boards in
/// this family may differ, so it stays configurable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelOrder {
    #[default]
    Rgb,
    Grb,
    Brg,
    Rbg,
    Gbr,
    Bgr,
}

impl ChannelOrder {
    /// Reorder a colour into wire order.
    pub fn apply(self, c: Rgb) -> [u8; 3] {
        match self {
            ChannelOrder::Rgb => [c.r, c.g, c.b],
            ChannelOrder::Grb => [c.g, c.r, c.b],
            ChannelOrder::Brg => [c.b, c.r, c.g],
            ChannelOrder::Rbg => [c.r, c.b, c.g],
            ChannelOrder::Gbr => [c.g, c.b, c.r],
            ChannelOrder::Bgr => [c.b, c.g, c.r],
        }
    }
}

impl std::str::FromStr for ChannelOrder {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "rgb" => Ok(Self::Rgb),
            "grb" => Ok(Self::Grb),
            "brg" => Ok(Self::Brg),
            "rbg" => Ok(Self::Rbg),
            "gbr" => Ok(Self::Gbr),
            "bgr" => Ok(Self::Bgr),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primaries() {
        assert_eq!(Rgb::from_hsv(0.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::from_hsv(1.0 / 3.0, 1.0, 1.0), Rgb::new(0, 255, 0));
        assert_eq!(Rgb::from_hsv(2.0 / 3.0, 1.0, 1.0), Rgb::new(0, 0, 255));
    }

    #[test]
    fn hsv_hue_wraps() {
        assert_eq!(Rgb::from_hsv(1.0, 1.0, 1.0), Rgb::from_hsv(0.0, 1.0, 1.0));
        assert_eq!(Rgb::from_hsv(-0.5, 1.0, 1.0), Rgb::from_hsv(0.5, 1.0, 1.0));
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Rgb::from_hex("#ff00aa"), Some(Rgb::new(255, 0, 170)));
        assert_eq!(Rgb::from_hex("00ff00"), Some(Rgb::new(0, 255, 0)));
        assert_eq!(Rgb::from_hex("nope"), None);
    }

    #[test]
    fn channel_order_reorders() {
        let c = Rgb::new(1, 2, 3);
        assert_eq!(ChannelOrder::Rgb.apply(c), [1, 2, 3]);
        assert_eq!(ChannelOrder::Grb.apply(c), [2, 1, 3]);
    }
}
