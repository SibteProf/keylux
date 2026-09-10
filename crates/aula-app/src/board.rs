//! The keyboard preview, drawn from real layout geometry.
//!
//! A single draw used by both the Play tab (read-only) and the timeline editor
//! (paintable). It returns a [`BoardGeom`] mapping screen space back to LED
//! indices, which the editor queries for click, drag and rectangle painting;
//! the read-only caller simply ignores it.
//!
//! Drawing the board this way doubles as an LED-map check: a wrong map shows up
//! immediately as an effect that looks scrambled here but fine on the board, or
//! the reverse.

use eframe::egui;

use aula_protocol::{Frame, KeyPos};

use crate::theme;

/// Where each LED landed on screen, so input can be mapped back to LEDs.
pub struct BoardGeom {
    boxes: Vec<(egui::Rect, usize)>,
}

impl BoardGeom {
    /// The LED whose key contains `p`, if any.
    pub fn led_at(&self, p: egui::Pos2) -> Option<usize> {
        self.boxes
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, led)| *led)
    }

    /// Every LED whose key intersects `sel` — for rectangle (region) painting.
    pub fn leds_in(&self, sel: egui::Rect) -> Vec<usize> {
        self.boxes
            .iter()
            .filter(|(r, _)| sel.intersects(*r))
            .map(|(_, led)| *led)
            .collect()
    }
}

/// Draw `frame` across `layout` inside `rect`. Returns the hit geometry.
pub fn draw_board(ui: &egui::Ui, rect: egui::Rect, layout: &[KeyPos], frame: &Frame) -> BoardGeom {
    let pal = theme::current();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, pal.board_bg);

    let mut boxes = Vec::with_capacity(layout.len());
    if layout.is_empty() {
        return BoardGeom { boxes };
    }

    let cols = layout
        .iter()
        .map(|k| k.x + k.w / 2.0)
        .fold(1.0_f32, f32::max);
    let rows = layout.iter().map(|k| k.row).max().unwrap_or(0) as f32 + 1.0;

    let pad = 10.0;
    let unit = ((rect.width() - pad * 2.0) / cols).min((rect.height() - pad * 2.0) / rows);
    let origin = egui::pos2(
        rect.left() + pad + ((rect.width() - pad * 2.0) - unit * cols) / 2.0,
        rect.top() + pad + ((rect.height() - pad * 2.0) - unit * rows) / 2.0,
    );

    for k in layout {
        let pos = egui::pos2(
            origin.x + (k.x - k.w / 2.0) * unit + 1.0,
            origin.y + f32::from(k.row) * unit + 1.0,
        );
        let size = egui::vec2((unit * k.w - 2.0).max(1.0), (unit - 2.0).max(1.0));
        let key_rect = egui::Rect::from_min_size(pos, size);

        // Unlit keys stay visible as dark tiles so the board reads as a keyboard
        // even when an effect is mostly black.
        let c = frame.get(k.led);
        let fill = if c.is_black() {
            pal.key_unlit
        } else {
            egui::Color32::from_rgb(c.r, c.g, c.b)
        };
        painter.rect_filled(key_rect, 2.0, fill);
        painter.rect_stroke(key_rect, 2.0, egui::Stroke::new(0.5_f32, pal.key_stroke));
        boxes.push((key_rect, k.led));
    }

    BoardGeom { boxes }
}
