//! Scrolling text.
//!
//! The F75's grid is roughly 16 columns by 6 rows and the rows are physically
//! *staggered*, so a lot of column/row positions have no key under them. Two
//! consequences shape this effect:
//!
//! - The glyphs are 5 wide. A 3-wide font is unreadable here: letters like B
//!   and E have a full-height first column, which just reads as a moving bar.
//! - The glyphs are 4 tall, not 5. Below the F-row the board only has four
//!   full-width rows (number, Tab, Caps, Shift); row 5 is the spacebar and a
//!   few modifiers, so a 5-row glyph loses its bottom bar and E renders as F.
//!
//! The spacebar and other wide keys are skipped entirely — one LED spanning
//! six columns cannot carry a pixel without swamping the word.

use aula_protocol::{Frame, Rgb};

use crate::{is_wide, Effect, EffectMeta, ParamSpec, RenderCtx};

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 4;

/// 5 wide, 4 tall, row-major, `1` = lit.
const FONT: &[(char, [&str; GLYPH_H])] = &[
    ('A', ["01110", "10001", "11111", "10001"]),
    ('B', ["11110", "11110", "10001", "11110"]),
    ('C', ["01111", "10000", "10000", "01111"]),
    ('D', ["11110", "10001", "10001", "11110"]),
    ('E', ["11111", "11100", "10000", "11111"]),
    ('F', ["11111", "11100", "10000", "10000"]),
    ('G', ["01111", "10000", "10011", "01111"]),
    ('H', ["10001", "11111", "10001", "10001"]),
    ('I', ["11111", "00100", "00100", "11111"]),
    ('J', ["00111", "00010", "10010", "01100"]),
    ('K', ["10010", "11100", "10010", "10001"]),
    ('L', ["10000", "10000", "10000", "11111"]),
    ('M', ["10001", "11011", "10101", "10001"]),
    ('N', ["10001", "11001", "10011", "10001"]),
    ('O', ["01110", "10001", "10001", "01110"]),
    ('P', ["11110", "10001", "11110", "10000"]),
    ('Q', ["01110", "10001", "10011", "01111"]),
    ('R', ["11110", "11110", "10010", "10001"]),
    ('S', ["01111", "01110", "00001", "11110"]),
    ('T', ["11111", "00100", "00100", "00100"]),
    ('U', ["10001", "10001", "10001", "01110"]),
    ('V', ["10001", "10001", "01010", "00100"]),
    ('W', ["10001", "10101", "11011", "10001"]),
    ('X', ["10001", "01010", "01010", "10001"]),
    ('Y', ["10001", "01010", "00100", "00100"]),
    ('Z', ["11111", "00110", "01100", "11111"]),
    ('0', ["01110", "10011", "11001", "01110"]),
    ('1', ["00100", "01100", "00100", "01110"]),
    ('2', ["11110", "00110", "01100", "11111"]),
    ('3', ["11110", "00110", "00001", "11110"]),
    ('4', ["10010", "10010", "11111", "00010"]),
    ('5', ["11111", "11110", "00001", "11110"]),
    ('6', ["01110", "10000", "11110", "01110"]),
    ('7', ["11111", "00010", "00100", "01000"]),
    ('8', ["01110", "01110", "10001", "01110"]),
    ('9', ["01110", "01111", "00001", "01110"]),
    ('.', ["00000", "00000", "00000", "00100"]),
    (',', ["00000", "00000", "00100", "01000"]),
    ('-', ["00000", "01110", "00000", "00000"]),
    ('!', ["00100", "00100", "00000", "00100"]),
    ('?', ["11110", "00011", "00000", "00100"]),
    (' ', ["00000", "00000", "00000", "00000"]),
];

fn glyph(ch: char) -> [&'static str; GLYPH_H] {
    let up = ch.to_ascii_uppercase();
    FONT.iter()
        .find(|(c, _)| *c == up)
        .or_else(|| FONT.iter().find(|(c, _)| *c == '?'))
        .map(|(_, g)| *g)
        .unwrap_or(["00000"; GLYPH_H])
}

/// Column-major bitmap of the whole message, one blank column between letters.
fn bitmap(text: &str) -> Vec<[bool; GLYPH_H]> {
    let mut cols = Vec::new();
    for ch in text.chars() {
        let g = glyph(ch);
        for c in 0..GLYPH_W {
            let mut col = [false; GLYPH_H];
            for (r, row) in g.iter().enumerate() {
                col[r] = row.as_bytes().get(c) == Some(&b'1');
            }
            cols.push(col);
        }
        cols.push([false; GLYPH_H]);
    }
    cols
}

#[derive(Default)]
pub struct Text;

impl Effect for Text {
    fn meta(&self) -> EffectMeta {
        EffectMeta {
            id: "text".into(),
            name: "Scrolling text".into(),
            description: "A word sweeping across the board, one column per key.".into(),
            params: vec![
                ParamSpec::text("text", "Message", "HELLO"),
                ParamSpec::float("speed", "Columns per second", 0.5, 20.0, 6.0),
                ParamSpec::float("hue", "Hue", 0.0, 1.0, 0.55),
                ParamSpec::boolean("cycle_hue", "Tint each letter", true),
                ParamSpec::int("top_row", "Top row", 0, 3, 1),
                ParamSpec::float("brightness", "Brightness", 0.0, 1.0, 1.0),
            ],
        }
    }

    fn render(&mut self, ctx: &RenderCtx, out: &mut Frame) {
        let text = ctx.params.text("text", "HELLO");
        let speed = ctx.params.float("speed", 6.0);
        let base_hue = ctx.params.float("hue", 0.55);
        let cycle_hue = ctx.params.bool("cycle_hue", true);
        let top_row = ctx.params.int("top_row", 1) as i32;
        let brightness = ctx.params.float("brightness", 1.0);

        let cols = bitmap(&text);
        if cols.is_empty() {
            return;
        }

        let board_cols = ctx.max_x.round() + 1.0;
        let travel = cols.len() as f32 + board_cols;
        // Where bitmap column 0 sits on the board. It increases with time,
        // which moves the word to the right.
        let left_edge = -(cols.len() as f32) + (ctx.t * speed).rem_euclid(travel);

        for k in ctx.layout {
            // One LED under six columns cannot carry a pixel.
            if is_wide(k) {
                out.set(k.led, Rgb::BLACK);
                continue;
            }

            let col = (k.x - 0.5).round() as i32;
            let row = i32::from(k.row) - top_row;
            let mut lit = false;
            let mut hue = base_hue;

            if (0..GLYPH_H as i32).contains(&row) {
                let src = col - left_edge.floor() as i32;
                if src >= 0 && (src as usize) < cols.len() {
                    lit = cols[src as usize][row as usize];
                    // Tint by which letter this column belongs to, so the word
                    // reads as separate letters rather than one blob.
                    if cycle_hue {
                        let letter = src as usize / (GLYPH_W + 1);
                        hue = (base_hue + letter as f32 * 0.18).rem_euclid(1.0);
                    }
                }
            }

            out.set(
                k.led,
                if lit {
                    Rgb::from_hsv(hue, 1.0, brightness)
                } else {
                    Rgb::BLACK
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Params, Value};
    use aula_protocol::f75::keymap;

    fn render_at(t: f32, params: &Params) -> (Frame, Vec<aula_protocol::KeyPos>) {
        let layout = keymap::layout();
        let ctx = RenderCtx {
            t,
            layout: &layout,
            max_x: keymap::max_x(&layout),
            max_row: f32::from(keymap::max_row(&layout)),
            params,
        };
        let mut f = Frame::black(126);
        Text.render(&ctx, &mut f);
        (f, layout)
    }

    #[test]
    fn a_letter_is_five_columns_wide() {
        let b = bitmap("A");
        assert_eq!(b.len(), GLYPH_W + 1, "five columns plus a gap");
        assert!(b[GLYPH_W].iter().all(|x| !x), "the gap is blank");
    }

    #[test]
    fn unknown_characters_fall_back_rather_than_vanish() {
        assert_eq!(bitmap("\u{263a}"), bitmap("?"));
    }

    #[test]
    fn the_message_scrolls_and_stays_off_the_f_row() {
        let mut params = Params::from_specs(&Text.meta().params);
        params.set("text", Value::Text("SIBTE".into()));

        let (a, layout) = render_at(0.4, &params);
        let (b, _) = render_at(1.2, &params);
        assert!(layout.iter().any(|k| !a.get(k.led).is_black()));
        assert_ne!(a, b, "the text must travel");

        // top_row defaults to 1, so row 0 (Esc and the F-keys) stays dark.
        for k in layout.iter().filter(|k| k.row == 0) {
            assert!(a.get(k.led).is_black(), "{} should stay dark", k.name);
        }
    }

    /// The spacebar is a single LED spanning about six columns; lighting it
    /// from one bitmap column would drown the word.
    #[test]
    fn wide_keys_are_skipped() {
        let params = Params::from_specs(&Text.meta().params);
        let (f, layout) = render_at(0.4, &params);
        for k in layout.iter().filter(|k| is_wide(k)) {
            assert!(f.get(k.led).is_black(), "{} should stay dark", k.name);
        }
    }
}
