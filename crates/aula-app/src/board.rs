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

use aula_protocol::{Frame, KeyPos, Rgb};

use crate::theme;

/// Where each LED landed on screen, so input can be mapped back to LEDs.
pub struct BoardGeom {
    boxes: Vec<(egui::Rect, usize)>,
}

impl BoardGeom {
    pub fn outline_keys(&self, ui: &egui::Ui, rect: egui::Rect, selected: impl Fn(usize) -> bool) {
        let painter = ui.painter_at(rect);
        for (key_rect, led) in &self.boxes {
            if selected(*led) {
                painter.rect_stroke(
                    *key_rect,
                    2.0,
                    egui::Stroke::new(2.0_f32, theme::current().accent),
                );
            } else {
                painter.rect_filled(*key_rect, 2.0, egui::Color32::from_black_alpha(150));
            }
        }
    }

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
    painter.rect_filled(rect, 8.0, pal.board_bg);

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
        painter.rect_filled(key_rect, 4.0, fill);
        painter.rect_stroke(key_rect, 4.0, egui::Stroke::new(0.75_f32, pal.key_stroke));

        // SDK labels are often "Key: A". Keep the full label in the layout
        // for scripts, and clip long legends so they cannot cover nearby keys.
        let label = k.name.strip_prefix("Key: ").unwrap_or(&k.name);
        painter.with_clip_rect(key_rect.shrink(1.0)).text(
            key_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional((unit * 0.28).clamp(10.0, 13.0)),
            key_label_color(c, pal.text_muted),
        );

        boxes.push((key_rect, k.led));
    }

    BoardGeom { boxes }
}

fn key_label_color(c: Rgb, muted: egui::Color32) -> egui::Color32 {
    if c.is_black() {
        return muted;
    }

    let linear = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b);
    if luminance > 0.179 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_blue_keys_get_light_labels() {
        assert_eq!(
            key_label_color(Rgb::new(0, 0, 180), egui::Color32::GRAY),
            egui::Color32::WHITE
        );
    }

    #[test]
    fn bright_keys_get_dark_labels() {
        assert_eq!(
            key_label_color(Rgb::new(245, 245, 245), egui::Color32::GRAY),
            egui::Color32::BLACK
        );
    }

    #[test]
    fn unlit_keys_use_muted_labels() {
        let muted = egui::Color32::from_rgb(90, 90, 90);

        assert_eq!(key_label_color(Rgb::BLACK, muted), muted);
    }

    #[test]
    fn red_keys_use_black_labels_for_readable_contrast() {
        assert_eq!(
            key_label_color(Rgb::new(255, 0, 0), egui::Color32::GRAY),
            egui::Color32::BLACK
        );
    }
}
