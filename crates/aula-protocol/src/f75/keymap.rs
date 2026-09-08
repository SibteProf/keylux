//! AULA F75 LED index map and physical geometry.
//!
//! The LED order is the keyboard's own key-matrix order, which the firmware
//! exposes via command 0x83 (4 bytes per entry, HID usage code in byte 3).
//! Decoding that table gives `LED_ORDER` below.
//!
//! It is column-major: down the leftmost physical column (Esc, `, Tab, Caps,
//! LShift, LCtrl), then the next column, and so on. 90 of the 128 indices are
//! real keys; the rest are matrix positions with no key fitted.
//!
//! VERIFIED on hardware: writing a single 0xFF byte at buffer offset 1 lit the
//! backtick key red, matching index 1 here.

use crate::device::KeyPos;

/// LED index -> key name, straight from the firmware's matrix table.
#[rustfmt::skip]
pub const LED_ORDER: [Option<&str>; 90] = [
    Some("Esc"), Some("`"), Some("Tab"), Some("Caps"), Some("LShift"), Some("LCtrl"), None, Some("1"),
    Some("Q"), Some("A"), Some("Z"), Some("LWin"), Some("F1"), Some("2"), Some("W"), Some("S"),
    Some("X"), Some("LAlt"), Some("F2"), Some("3"), Some("E"), Some("D"), Some("C"), None,
    Some("F3"), Some("4"), Some("R"), Some("F"), Some("V"), None, Some("F4"), Some("5"),
    Some("T"), Some("G"), Some("B"), Some("Space"), Some("F5"), Some("6"), Some("Y"), Some("H"),
    Some("N"), None, Some("F6"), Some("7"), Some("U"), Some("J"), Some("M"), None,
    Some("F7"), Some("8"), Some("I"), Some("K"), Some(","), Some("Fn"), Some("F8"), Some("9"),
    Some("O"), Some("L"), Some("."), Some("RCtrl"), Some("F9"), Some("0"), Some("P"), Some(";"),
    Some("/"), None, Some("F10"), None, Some("["), Some("'"), Some("RShift"), None,
    Some("F11"), Some("="), Some("]"), None, None, Some("Left"), Some("F12"), Some("Backspace"),
    Some("\\"), Some("Enter"), Some("Up"), Some("Down"), Some("Knob"), Some("Del"), Some("PgUp"), Some("PgDn"),
    Some("End"), Some("Right"),
];

/// Physical rows: (key name, width in 1u units), left to right.
#[rustfmt::skip]
const ROWS: &[&[(&str, f32)]] = &[
    &[("Esc", 1.0), ("F1", 1.0), ("F2", 1.0), ("F3", 1.0), ("F4", 1.0), ("F5", 1.0), ("F6", 1.0),
      ("F7", 1.0), ("F8", 1.0), ("F9", 1.0), ("F10", 1.0), ("F11", 1.0), ("F12", 1.0),
      ("Del", 1.0), ("Knob", 1.0)],
    &[("`", 1.0), ("1", 1.0), ("2", 1.0), ("3", 1.0), ("4", 1.0), ("5", 1.0), ("6", 1.0), ("7", 1.0),
      ("8", 1.0), ("9", 1.0), ("0", 1.0), ("-", 1.0), ("=", 1.0), ("Backspace", 2.0), ("PgUp", 1.0)],
    &[("Tab", 1.5), ("Q", 1.0), ("W", 1.0), ("E", 1.0), ("R", 1.0), ("T", 1.0), ("Y", 1.0), ("U", 1.0),
      ("I", 1.0), ("O", 1.0), ("P", 1.0), ("[", 1.0), ("]", 1.0), ("\\", 1.5), ("PgDn", 1.0)],
    &[("Caps", 1.75), ("A", 1.0), ("S", 1.0), ("D", 1.0), ("F", 1.0), ("G", 1.0), ("H", 1.0), ("J", 1.0),
      ("K", 1.0), ("L", 1.0), (";", 1.0), ("'", 1.0), ("Enter", 2.25), ("Home", 1.0)],
    &[("LShift", 2.25), ("Z", 1.0), ("X", 1.0), ("C", 1.0), ("V", 1.0), ("B", 1.0), ("N", 1.0), ("M", 1.0),
      (",", 1.0), (".", 1.0), ("/", 1.0), ("RShift", 1.75), ("Up", 1.0), ("End", 1.0)],
    &[("LCtrl", 1.25), ("LWin", 1.25), ("LAlt", 1.25), ("Space", 6.25), ("Fn", 1.0),
      ("RCtrl", 1.0), ("Left", 1.0), ("Down", 1.0), ("Right", 1.0)],
];

/// Build the layout: every key that has both a physical position and an LED.
pub fn layout() -> Vec<KeyPos> {
    let mut keys = Vec::new();
    for (row_idx, row) in ROWS.iter().enumerate() {
        let mut x = 0.0f32;
        for (name, w) in row.iter() {
            if let Some(led) = led_for(name) {
                keys.push(KeyPos {
                    name: static_name(name),
                    row: row_idx as u8,
                    x: x + w / 2.0,
                    w: *w,
                    led,
                });
            }
            x += w;
        }
    }
    keys
}

fn led_for(name: &str) -> Option<usize> {
    LED_ORDER
        .iter()
        .position(|slot| matches!(slot, Some(n) if *n == name))
}

/// `KeyPos::name` is `&'static str`; ROWS entries already are.
fn static_name(name: &str) -> &'static str {
    ROWS.iter()
        .flat_map(|r| r.iter())
        .find(|(n, _)| *n == name)
        .map(|(n, _)| *n)
        .unwrap_or("?")
}

/// Rightmost key centre, in 1u units. Effects use it to normalise across the board.
pub fn max_x(keys: &[KeyPos]) -> f32 {
    keys.iter().map(|k| k.x).fold(0.0, f32::max)
}

/// Bottom row index.
pub fn max_row(keys: &[KeyPos]) -> u8 {
    keys.iter().map(|k| k.row).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_is_led_zero_and_backtick_is_one() {
        // The single-byte hardware probe that proved the ordering.
        assert_eq!(led_for("Esc"), Some(0));
        assert_eq!(led_for("`"), Some(1));
    }

    #[test]
    fn column_major_stride_of_six() {
        // The leftmost column is Esc, `, Tab, Caps, LShift, LCtrl; the next
        // column starts six later, which is why F1 is 12 and F2 is 18.
        assert_eq!(led_for("F1"), Some(12));
        assert_eq!(led_for("F2"), Some(18));
        assert_eq!(led_for("F3"), Some(24));
    }

    #[test]
    fn layout_is_populated_and_unique() {
        let keys = layout();
        assert!(
            keys.len() > 70,
            "expected most keys mapped, got {}",
            keys.len()
        );
        let mut leds: Vec<_> = keys.iter().map(|k| k.led).collect();
        leds.sort_unstable();
        let before = leds.len();
        leds.dedup();
        assert_eq!(before, leds.len(), "LED indices must be unique");
    }

    #[test]
    fn geometry_is_sane() {
        let keys = layout();
        assert!(max_row(&keys) == 5, "F75 has six rows");
        let mx = max_x(&keys);
        assert!((14.0..18.0).contains(&mx), "board is ~16u wide, got {mx}");
    }

    #[test]
    fn spacebar_is_wide_enough_to_exclude() {
        let keys = layout();
        let space = keys.iter().find(|k| k.name == "Space").unwrap();
        assert!(space.w >= 3.0, "effects rely on w to skip oversized keys");
    }
}
