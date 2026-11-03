//! Output-only colour correction; source effects and editor files stay intact.
use aula_protocol::{Frame, Rgb};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calibration {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            red: 100,
            green: 100,
            blue: 100,
        }
    }
}

impl Calibration {
    pub fn apply(self, source: &Frame) -> Frame {
        let scale = |value: u8, percent: u8| {
            ((u16::from(value) * u16::from(percent.min(100)) + 50) / 100) as u8
        };
        let mut output = Frame::black(source.len());
        for (i, c) in source.as_slice().iter().enumerate() {
            output.set(
                i,
                Rgb::new(
                    scale(c.r, self.red),
                    scale(c.g, self.green),
                    scale(c.b, self.blue),
                ),
            );
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_preserves_sources_black_and_identity() {
        let mut source = Frame::solid(3, Rgb::new(255, 127, 1));
        source.set(1, Rgb::BLACK);
        assert_eq!(Calibration::default().apply(&source), source);
        let corrected = Calibration {
            red: 50,
            green: 0,
            blue: 255,
        }
        .apply(&source);
        assert_eq!(corrected.get(0), Rgb::new(128, 0, 1));
        assert_eq!(corrected.get(1), Rgb::BLACK);
        assert_eq!(source.get(0), Rgb::new(255, 127, 1));
        assert_eq!(corrected.len(), source.len());
    }
}
