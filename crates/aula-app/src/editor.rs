//! Timeline editor: build animations by painting keys, no code required.
//!
//! The editor owns an [`Animation`] and a selected keyframe. Clicking a key on
//! the preview paints it with the current colour; the timeline strip below
//! adds, moves and deletes keyframes. Saving writes JSON into the animations
//! directory, where the registry picks it up like any other effect.

use eframe::egui;

use aula_effects::animation::{Animation, HexColor, Keyframe};
use aula_protocol::{Frame, KeyPos, Rgb};

pub struct Editor {
    pub anim: Animation,
    pub selected: usize,
    pub brush: Rgb,
    /// Preview scrub position, in seconds.
    pub playhead: f32,
    pub playing: bool,
    pub dirty: bool,
    pub file_name: String,
    pub status: Option<String>,
}

impl Editor {
    pub fn new(leds: usize) -> Self {
        Self {
            anim: Animation::new("My animation", leds),
            selected: 0,
            brush: Rgb::new(255, 40, 120),
            playhead: 0.0,
            playing: false,
            dirty: false,
            file_name: "my-animation".into(),
            status: None,
        }
    }

    /// Open an existing animation for editing.
    #[allow(dead_code)] // used once the "edit this animation" button lands
    pub fn from_animation(anim: Animation, file_name: String) -> Self {
        Self {
            selected: 0,
            file_name,
            anim,
            brush: Rgb::new(255, 40, 120),
            playhead: 0.0,
            playing: false,
            dirty: false,
            status: None,
        }
    }

    fn kf(&mut self) -> &mut Keyframe {
        if self.selected >= self.anim.keyframes.len() {
            self.selected = self.anim.keyframes.len().saturating_sub(1);
        }
        &mut self.anim.keyframes[self.selected]
    }

    /// The frame the board should show while editing: the selected keyframe,
    /// or the interpolated animation when previewing.
    pub fn preview_frame(&self, leds: usize) -> Frame {
        let mut f = Frame::black(leds);
        if self.playing {
            self.anim.sample(self.playhead, &mut f);
        } else if let Some(k) = self.anim.keyframes.get(self.selected) {
            for (led, c) in k.colors.iter().enumerate() {
                f.set(led, c.to_rgb());
            }
        }
        f
    }

    pub fn paint(&mut self, led: usize, color: Rgb) {
        let kf = self.kf();
        if led < kf.colors.len() {
            kf.colors[led] = HexColor::from(color);
            self.dirty = true;
        }
    }

    pub fn fill_all(&mut self, color: Rgb) {
        let kf = self.kf();
        for c in kf.colors.iter_mut() {
            *c = HexColor::from(color);
        }
        self.dirty = true;
    }

    pub fn add_keyframe(&mut self) {
        let leds = self.anim.leds;
        // Insert after the selected one, halfway to the next (or +0.5s at the end).
        let cur_t = self
            .anim
            .keyframes
            .get(self.selected)
            .map(|k| k.t)
            .unwrap_or(0.0);
        let next_t = self
            .anim
            .keyframes
            .get(self.selected + 1)
            .map(|k| k.t)
            .unwrap_or(cur_t + 1.0);
        let t = (cur_t + next_t) / 2.0;

        // Copy the current keyframe so the user edits a duplicate rather than
        // starting from black — far more useful in practice.
        let mut new = self
            .anim
            .keyframes
            .get(self.selected)
            .cloned()
            .unwrap_or_else(|| Keyframe::black(t, leds));
        new.t = t;

        self.anim.keyframes.insert(self.selected + 1, new);
        self.selected += 1;
        self.anim.normalise();
        self.dirty = true;
    }

    pub fn delete_keyframe(&mut self) {
        if self.anim.keyframes.len() <= 1 {
            self.status = Some("An animation needs at least one keyframe".into());
            return;
        }
        self.anim.keyframes.remove(self.selected);
        self.selected = self.selected.min(self.anim.keyframes.len() - 1);
        self.anim.normalise();
        self.dirty = true;
    }

    pub fn save(&mut self, dir: &std::path::Path) -> Option<std::path::PathBuf> {
        let name = sanitise(&self.file_name);
        let path = dir.join(format!("{name}.json"));
        self.anim.normalise();
        match self.anim.save(&path) {
            Ok(()) => {
                self.dirty = false;
                self.status = Some(format!("Saved to {}", path.display()));
                Some(path)
            }
            Err(e) => {
                self.status = Some(format!("Save failed: {e}"));
                None
            }
        }
    }

    /// Advance the preview playhead.
    pub fn tick(&mut self, dt: f32) {
        if self.playing {
            self.playhead = (self.playhead + dt) % self.anim.duration.max(0.001);
        }
    }
}

/// Draw the editor. Returns true if the animation changed this frame.
pub fn show(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos], anim_dir: &std::path::Path) {
    ui.horizontal(|ui| {
        ui.label("Name");
        if ui.text_edit_singleline(&mut ed.anim.name).changed() {
            ed.dirty = true;
        }
        ui.label("File");
        ui.add(egui::TextEdit::singleline(&mut ed.file_name).desired_width(120.0));
        if ui.button("💾 Save").clicked() {
            ed.save(anim_dir);
        }
        if ed.dirty {
            ui.colored_label(egui::Color32::from_rgb(230, 180, 60), "● unsaved");
        }
    });

    ui.separator();

    // ---- painting ----
    ui.horizontal(|ui| {
        ui.label("Brush");
        let mut rgb = [ed.brush.r, ed.brush.g, ed.brush.b];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            ed.brush = Rgb::new(rgb[0], rgb[1], rgb[2]);
        }
        if ui.button("Fill all").clicked() {
            let b = ed.brush;
            ed.fill_all(b);
        }
        if ui.button("Clear").clicked() {
            ed.fill_all(Rgb::BLACK);
        }
        ui.separator();
        ui.checkbox(&mut ed.anim.interpolate, "Blend between keyframes")
            .on_hover_text("Off = hard cuts, like a flipbook");
        ui.checkbox(&mut ed.anim.loops, "Loop");
    });

    ui.add_space(4.0);
    ui.weak("Click a key to paint it. Right-click to erase. Drag to paint several.");

    // ---- the paintable board ----
    let desired = egui::vec2(ui.available_width(), 200.0);
    let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());
    let frame = ed.preview_frame(ed.anim.leds);
    let hit = draw_editable_board(ui, rect, layout, &frame);

    if let Some(pos) = resp.interact_pointer_pos() {
        if let Some(led) = hit(pos) {
            if resp.secondary_clicked() {
                ed.paint(led, Rgb::BLACK);
            } else if resp.clicked() || resp.dragged() {
                let b = ed.brush;
                ed.paint(led, b);
            }
        }
    }

    ui.add_space(6.0);

    // ---- timeline ----
    ui.horizontal(|ui| {
        if ui.button(if ed.playing { "⏸" } else { "▶" }).clicked() {
            ed.playing = !ed.playing;
        }
        ui.label("Duration");
        if ui
            .add(
                egui::DragValue::new(&mut ed.anim.duration)
                    .range(0.1..=60.0)
                    .suffix(" s"),
            )
            .changed()
        {
            ed.dirty = true;
        }
        ui.separator();
        if ui.button("➕ Keyframe").clicked() {
            ed.add_keyframe();
        }
        if ui.button("🗑 Delete").clicked() {
            ed.delete_keyframe();
        }
        ui.label(format!(
            "{} of {}",
            ed.selected + 1,
            ed.anim.keyframes.len()
        ));
    });

    ui.add_space(4.0);
    timeline_strip(ui, ed);

    if let Some(k) = ed.anim.keyframes.get_mut(ed.selected) {
        ui.horizontal(|ui| {
            ui.label("Keyframe time");
            if ui
                .add(
                    egui::DragValue::new(&mut k.t)
                        .range(0.0..=60.0)
                        .speed(0.02)
                        .suffix(" s"),
                )
                .changed()
            {
                ed.dirty = true;
            }
        });
    }

    if let Some(s) = &ed.status {
        ui.add_space(4.0);
        ui.weak(s);
    }
}

/// The timeline: one marker per keyframe, click to select, drag to move.
fn timeline_strip(ui: &mut egui::Ui, ed: &mut Editor) {
    let h = 34.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), h),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_gray(28));

    let dur = ed.anim.duration.max(0.001);
    let x_of = |t: f32| rect.left() + 8.0 + (t / dur) * (rect.width() - 16.0);

    // playhead
    if ed.playing {
        let x = x_of(ed.playhead);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.5_f32, egui::Color32::from_gray(140)),
        );
    }

    let mut clicked_idx = None;
    for (i, k) in ed.anim.keyframes.iter().enumerate() {
        let x = x_of(k.t);
        let c = egui::pos2(x, rect.center().y);
        let selected = i == ed.selected;
        let r = if selected { 8.0 } else { 6.0 };
        let col = if selected {
            egui::Color32::from_rgb(120, 190, 255)
        } else {
            egui::Color32::from_gray(120)
        };
        painter.circle_filled(c, r, col);

        if let Some(p) = resp.interact_pointer_pos() {
            if (p.x - x).abs() < 10.0 {
                clicked_idx = Some(i);
            }
        }
    }

    if let Some(i) = clicked_idx {
        if resp.clicked() {
            ed.selected = i;
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = ((p.x - rect.left() - 8.0) / (rect.width() - 16.0) * dur).clamp(0.0, dur);
                ed.anim.keyframes[i].t = t;
                ed.selected = i;
                ed.dirty = true;
            }
        }
    }
}

/// Draw the board and return a hit-test closure mapping a screen point to an LED.
fn draw_editable_board(
    ui: &egui::Ui,
    rect: egui::Rect,
    layout: &[KeyPos],
    frame: &Frame,
) -> impl Fn(egui::Pos2) -> Option<usize> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, egui::Color32::from_gray(18));

    let cols = layout
        .iter()
        .map(|k| k.x + k.w / 2.0)
        .fold(1.0f32, f32::max);
    let rows = layout.iter().map(|k| k.row).max().unwrap_or(0) as f32 + 1.0;

    let pad = 10.0;
    let unit = ((rect.width() - pad * 2.0) / cols).min((rect.height() - pad * 2.0) / rows);
    let origin = egui::pos2(
        rect.left() + pad + ((rect.width() - pad * 2.0) - unit * cols) / 2.0,
        rect.top() + pad + ((rect.height() - pad * 2.0) - unit * rows) / 2.0,
    );

    let mut boxes: Vec<(egui::Rect, usize)> = Vec::with_capacity(layout.len());
    for k in layout {
        let pos = egui::pos2(
            origin.x + (k.x - k.w / 2.0) * unit + 1.0,
            origin.y + f32::from(k.row) * unit + 1.0,
        );
        let size = egui::vec2((unit * k.w - 2.0).max(1.0), (unit - 2.0).max(1.0));
        let key_rect = egui::Rect::from_min_size(pos, size);

        let c = frame.get(k.led);
        let fill = if c.is_black() {
            egui::Color32::from_gray(34)
        } else {
            egui::Color32::from_rgb(c.r, c.g, c.b)
        };
        painter.rect_filled(key_rect, 2.0, fill);
        painter.rect_stroke(
            key_rect,
            2.0,
            egui::Stroke::new(0.5_f32, egui::Color32::from_gray(60)),
        );
        boxes.push((key_rect, k.led));
    }

    move |p: egui::Pos2| {
        boxes
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, led)| *led)
    }
}

fn sanitise(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if s.is_empty() {
        "animation".into()
    } else {
        s.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painting_marks_dirty_and_sets_the_colour() {
        let mut ed = Editor::new(10);
        ed.paint(3, Rgb::new(1, 2, 3));
        assert!(ed.dirty);
        assert_eq!(ed.anim.keyframes[0].color(3), Rgb::new(1, 2, 3));
    }

    #[test]
    fn adding_a_keyframe_duplicates_the_current_one() {
        let mut ed = Editor::new(4);
        ed.paint(0, Rgb::new(9, 9, 9));
        let before = ed.anim.keyframes.len();
        ed.add_keyframe();
        assert_eq!(ed.anim.keyframes.len(), before + 1);
        // the new keyframe inherits the painted pixel rather than starting black
        assert_eq!(ed.anim.keyframes[ed.selected].color(0), Rgb::new(9, 9, 9));
    }

    #[test]
    fn cannot_delete_the_last_keyframe() {
        let mut ed = Editor::new(2);
        ed.anim.keyframes.truncate(1);
        ed.delete_keyframe();
        assert_eq!(ed.anim.keyframes.len(), 1);
        assert!(ed.status.is_some());
    }

    #[test]
    fn file_names_are_sanitised() {
        assert_eq!(sanitise("My Cool Anim!"), "my-cool-anim-");
        assert_eq!(sanitise("   "), "animation");
    }
}
