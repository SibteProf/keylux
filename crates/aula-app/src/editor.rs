//! Composition editor: build effects by stacking layers, no code required.
//!
//! An effect is a [`Composition`] — a stack of layers composited by blend mode.
//! A layer is either painted **keyframes** (the timeline/brush tools below) or
//! a procedural **generator** (Sweep, Wave, …). This is the merge of the old
//! "animation" and "script" ideas into one surface: paint a base, drop a moving
//! rainbow on top, composite, done. The "Send to keyboard" toggle streams the
//! composited preview to real hardware while editing.
//!
//! Layer settings are baked in at authoring time; playback exposes only master
//! speed and brightness. Saving writes a `.klx` JSON file into the animations
//! directory, where the registry picks it up like any other effect.

use eframe::egui;

use aula_effects::animation::{Animation, Ease, HexColor, Keyframe};
use aula_effects::composition::{Composition, Layer, LayerContent};
use aula_effects::generators::{Direction, GenParams, Generator};
use aula_effects::Blend;
use aula_protocol::{Frame, KeyPos, Rgb};

use crate::board::{self, BoardGeom};
use crate::theme;

/// How a click or drag on the board paints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BrushMode {
    Key,
    Row,
    Column,
    Region,
    Pick,
}

impl BrushMode {
    fn label(self) -> &'static str {
        match self {
            BrushMode::Key => "Key",
            BrushMode::Row => "Row",
            BrushMode::Column => "Column",
            BrushMode::Region => "Region",
            BrushMode::Pick => "Eyedropper",
        }
    }
    const ALL: [BrushMode; 5] = [
        BrushMode::Key,
        BrushMode::Row,
        BrushMode::Column,
        BrushMode::Region,
        BrushMode::Pick,
    ];
}

const UNDO_LIMIT: usize = 80;

pub struct Editor {
    pub comp: Composition,
    /// The layer being edited.
    pub active: usize,
    /// The selected keyframe within the active keyframe layer.
    pub selected: usize,
    pub brush: Rgb,
    pub brush_b: Rgb,
    pub mode: BrushMode,
    pub playhead: f32,
    pub playing: bool,
    pub dirty: bool,
    pub file_name: String,
    pub status: Option<String>,
    /// Stream the composited preview to the physical keyboard while editing.
    pub live: bool,

    /// Layer-stack snapshots for undo (newest last) and redo.
    undo: Vec<Vec<Layer>>,
    redo: Vec<Vec<Layer>>,
    clip: Option<Vec<HexColor>>,
    region_start: Option<egui::Pos2>,
    stroke_open: bool,
}

impl Editor {
    pub fn new(leds: usize) -> Self {
        Self::from_parts(
            Composition::new("My effect", leds.max(1)),
            "my-effect".into(),
        )
    }

    /// Open an existing composition for editing.
    pub fn from_composition(comp: Composition, file_name: String) -> Self {
        Self::from_parts(comp, file_name)
    }

    /// Open a legacy `.json` animation, wrapped as a single keyframe layer.
    pub fn from_animation(anim: Animation, file_name: String) -> Self {
        Self::from_parts(Composition::from_animation(anim), file_name)
    }

    fn from_parts(comp: Composition, file_name: String) -> Self {
        Self {
            comp,
            active: 0,
            selected: 0,
            brush: Rgb::new(255, 40, 120),
            brush_b: Rgb::new(40, 120, 255),
            mode: BrushMode::Key,
            playhead: 0.0,
            playing: false,
            dirty: false,
            file_name,
            status: None,
            live: false,
            undo: Vec::new(),
            redo: Vec::new(),
            clip: None,
            region_start: None,
            stroke_open: false,
        }
    }

    // ---- layer access ----

    fn active_is_keyframes(&self) -> bool {
        matches!(
            self.comp.layers.get(self.active).map(|l| &l.content),
            Some(LayerContent::Keyframes(_))
        )
    }

    fn anim(&self) -> Option<&Animation> {
        match &self.comp.layers.get(self.active)?.content {
            LayerContent::Keyframes(a) => Some(a),
            _ => None,
        }
    }

    fn anim_mut(&mut self) -> Option<&mut Animation> {
        match &mut self.comp.layers.get_mut(self.active)?.content {
            LayerContent::Keyframes(a) => Some(a),
            _ => None,
        }
    }

    fn gen_mut(&mut self) -> Option<&mut GenParams> {
        match &mut self.comp.layers.get_mut(self.active)?.content {
            LayerContent::Generator(p) => Some(p),
            _ => None,
        }
    }

    fn cur_kf_mut(&mut self) -> Option<&mut Keyframe> {
        let sel = self.selected;
        let a = self.anim_mut()?;
        if a.keyframes.is_empty() {
            return None;
        }
        let idx = sel.min(a.keyframes.len() - 1);
        a.keyframes.get_mut(idx)
    }

    fn selected_time(&self) -> f32 {
        self.anim()
            .and_then(|a| a.keyframes.get(self.selected))
            .map(|k| k.t)
            .unwrap_or(0.0)
    }

    /// The composited frame the board shows: the whole stack at the playhead
    /// when previewing, or frozen at the selected keyframe's time when paused.
    pub fn preview_frame(&self, leds: usize, layout: &[KeyPos]) -> Frame {
        let mut f = Frame::black(leds);
        let t = if self.playing {
            self.playhead
        } else {
            self.selected_time()
        };
        self.comp.sample(t, layout, &mut f);
        f
    }

    // ---- history ----

    fn snapshot(&mut self) {
        self.undo.push(self.comp.layers.clone());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.comp.layers.clone());
            self.comp.layers = prev;
            self.clamp();
            self.dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.comp.layers.clone());
            self.comp.layers = next;
            self.clamp();
            self.dirty = true;
        }
    }

    fn clamp(&mut self) {
        if self.comp.layers.is_empty() {
            self.comp.layers.push(Layer::keyframes(
                "Base",
                Animation::new("Base", self.comp.leds),
            ));
        }
        self.active = self.active.min(self.comp.layers.len() - 1);
        let kfs = self.anim().map(|a| a.keyframes.len()).unwrap_or(1);
        self.selected = self.selected.min(kfs.saturating_sub(1));
    }

    /// Keep every keyframe layer on the composition's master timeline.
    fn sync_timeline(&mut self) {
        let (dur, loops) = (self.comp.duration, self.comp.loops);
        for layer in &mut self.comp.layers {
            if let LayerContent::Keyframes(a) = &mut layer.content {
                a.duration = dur;
                a.loops = loops;
            }
        }
    }

    // ---- painting (acts on the active keyframe layer) ----

    pub fn paint(&mut self, led: usize, color: Rgb) {
        if let Some(k) = self.cur_kf_mut() {
            if led < k.colors.len() {
                k.colors[led] = HexColor::from(color);
            }
        }
        self.dirty = true;
    }

    pub fn fill_all(&mut self, color: Rgb) {
        if let Some(k) = self.cur_kf_mut() {
            for c in k.colors.iter_mut() {
                *c = HexColor::from(color);
            }
        }
        self.dirty = true;
    }

    fn apply_gradient(&mut self, layout: &[KeyPos]) {
        let max_x = layout.iter().map(|k| k.x).fold(1.0_f32, f32::max);
        let (a, b) = (self.brush, self.brush_b);
        for k in layout {
            let f = if max_x > 0.0 { k.x / max_x } else { 0.0 };
            self.paint(k.led, lerp_rgb(a, b, f));
        }
    }

    fn brush_at(&mut self, led: usize, color: Rgb, layout: &[KeyPos]) {
        match self.mode {
            BrushMode::Key => self.paint(led, color),
            BrushMode::Row => {
                if let Some(row) = layout.iter().find(|k| k.led == led).map(|k| k.row) {
                    let leds: Vec<usize> = layout
                        .iter()
                        .filter(|k| k.row == row)
                        .map(|k| k.led)
                        .collect();
                    for l in leds {
                        self.paint(l, color);
                    }
                }
            }
            BrushMode::Column => {
                if let Some(x0) = layout.iter().find(|k| k.led == led).map(|k| k.x) {
                    let leds: Vec<usize> = layout
                        .iter()
                        .filter(|k| (k.x - x0).abs() < 0.6)
                        .map(|k| k.led)
                        .collect();
                    for l in leds {
                        self.paint(l, color);
                    }
                }
            }
            BrushMode::Region | BrushMode::Pick => {}
        }
    }

    pub fn add_keyframe(&mut self) {
        self.snapshot();
        let sel = self.selected;
        let leds = self.comp.leds;
        let Some(a) = self.anim_mut() else { return };
        let cur_t = a.keyframes.get(sel).map(|k| k.t).unwrap_or(0.0);
        let next_t = a.keyframes.get(sel + 1).map(|k| k.t).unwrap_or(cur_t + 1.0);
        let t = (cur_t + next_t) / 2.0;
        let mut new = a
            .keyframes
            .get(sel)
            .cloned()
            .unwrap_or_else(|| Keyframe::black(t, leds));
        new.t = t;
        a.keyframes.insert(sel + 1, new);
        a.normalise();
        self.selected = sel + 1;
        self.dirty = true;
    }

    pub fn delete_keyframe(&mut self) {
        let sel = self.selected;
        if self.anim().map(|a| a.keyframes.len()).unwrap_or(0) <= 1 {
            self.status = Some("A layer needs at least one keyframe".into());
            return;
        }
        self.snapshot();
        if let Some(a) = self.anim_mut() {
            a.keyframes.remove(sel);
            a.normalise();
        }
        self.clamp();
        self.dirty = true;
    }

    pub fn save(&mut self, dir: &std::path::Path) -> Option<std::path::PathBuf> {
        let name = sanitise(&self.file_name);
        let path = dir.join(format!("{name}.klx"));
        self.sync_timeline();
        for layer in &mut self.comp.layers {
            if let LayerContent::Keyframes(a) = &mut layer.content {
                a.normalise();
            }
        }
        match self.comp.save(&path) {
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

    pub fn tick(&mut self, dt: f32) {
        if self.playing {
            self.playhead = (self.playhead + dt) % self.comp.duration.max(0.001);
        }
    }
}

/// Draw the editor.
pub fn show(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos], anim_dir: &std::path::Path) {
    handle_shortcuts(ui, ed);
    top_bar(ui, ed, anim_dir);
    ui.separator();
    layer_panel(ui, ed);
    ui.separator();

    if ed.active_is_keyframes() {
        brush_bar(ui, ed);
        ui.add_space(4.0);
        ui.weak("Click to paint · right-click to erase · drag to paint several.");
        board_area(ui, ed, layout, true);
        ui.add_space(6.0);
        timeline_controls(ui, ed);
        ui.add_space(4.0);
        timeline_strip(ui, ed);
        keyframe_controls(ui, ed);
    } else {
        ui.weak("Generator layer — it animates procedurally. Tune it below; the board shows the composite.");
        board_area(ui, ed, layout, false);
        ui.add_space(6.0);
        generator_controls(ui, ed);
    }

    if let Some(s) = &ed.status {
        ui.add_space(4.0);
        ui.weak(s);
    }
}

fn handle_shortcuts(ui: &egui::Ui, ed: &mut Editor) {
    let (undo, redo) = ui.input(|i| {
        let cmd = i.modifiers.command;
        let undo = cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z);
        let redo = cmd
            && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z)));
        (undo, redo)
    });
    if undo {
        ed.undo();
    }
    if redo {
        ed.redo();
    }
}

fn top_bar(ui: &mut egui::Ui, ed: &mut Editor, anim_dir: &std::path::Path) {
    let pal = theme::current();
    // Name/file/save on one row, the live toggle on its own, so a narrow window
    // never overlaps the "Send to keyboard" button onto the Save controls.
    ui.horizontal(|ui| {
        ui.label("Name");
        if ui
            .add(egui::TextEdit::singleline(&mut ed.comp.name).desired_width(160.0))
            .changed()
        {
            ed.dirty = true;
        }
        ui.label("File");
        ui.add(egui::TextEdit::singleline(&mut ed.file_name).desired_width(110.0));
        if ui.button("💾 Save").clicked() {
            ed.save(anim_dir);
        }
        if ed.dirty {
            ui.colored_label(pal.warning, "unsaved");
        }
    });
    ui.horizontal(|ui| {
        ui.toggle_value(&mut ed.live, "📡 Send to keyboard")
            .on_hover_text(
                "Stream the composited preview to the real keyboard as you edit.\n\
                 Smoothest over the cable; the 2.4 GHz link updates slowly.",
            );
    });
}

/// The layer stack: add / reorder / blend / opacity / enable, and pick active.
fn layer_panel(ui: &mut egui::Ui, ed: &mut Editor) {
    ui.horizontal(|ui| {
        ui.strong("Layers");
        if ui
            .button("➕ Paint")
            .on_hover_text("Add a keyframe layer")
            .clicked()
        {
            ed.snapshot();
            let n = ed.comp.layers.len() + 1;
            let mut a = Animation::new("Layer", ed.comp.leds);
            a.duration = ed.comp.duration;
            a.loops = ed.comp.loops;
            ed.comp
                .layers
                .push(Layer::keyframes(format!("Layer {n}"), a));
            ed.active = ed.comp.layers.len() - 1;
            ed.selected = 0;
            ed.dirty = true;
        }
        if ui
            .button("➕ Generator")
            .on_hover_text("Add a procedural layer")
            .clicked()
        {
            ed.snapshot();
            let n = ed.comp.layers.len() + 1;
            ed.comp
                .layers
                .push(Layer::generator(format!("Gen {n}"), GenParams::default()));
            ed.active = ed.comp.layers.len() - 1;
            ed.dirty = true;
        }
    });

    // Layers are drawn top-first so the list reads like the visual stack.
    let count = ed.comp.layers.len();
    let mut to_select = None;
    let mut reorder: Option<(usize, isize)> = None;
    let mut toggle: Option<usize> = None;
    for i in (0..count).rev() {
        let layer = &ed.comp.layers[i];
        let icon = match layer.content {
            LayerContent::Keyframes(_) => "🎞",
            LayerContent::Generator(_) => "✨",
        };
        let name = layer.name.clone();
        let enabled = layer.enabled;
        let is_active = i == ed.active;
        ui.horizontal(|ui| {
            let mut en = enabled;
            if ui.checkbox(&mut en, "").changed() {
                toggle = Some(i);
            }
            if ui
                .selectable_label(is_active, format!("{icon} {name}"))
                .clicked()
            {
                to_select = Some(i);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(i + 1 < count, egui::Button::new("⬆").small())
                    .clicked()
                {
                    reorder = Some((i, 1)); // toward the top of the stack
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("⬇").small())
                    .clicked()
                {
                    reorder = Some((i, -1));
                }
            });
        });
    }

    if let Some(i) = to_select {
        ed.active = i;
        ed.selected = 0;
    }
    if let Some(i) = toggle {
        ed.snapshot();
        ed.comp.layers[i].enabled = !ed.comp.layers[i].enabled;
        ed.dirty = true;
    }
    if let Some((i, dir)) = reorder {
        let j = (i as isize + dir) as usize;
        if j < count {
            ed.snapshot();
            ed.comp.layers.swap(i, j);
            if ed.active == i {
                ed.active = j;
            } else if ed.active == j {
                ed.active = i;
            }
            ed.dirty = true;
        }
    }

    // Controls for the active layer: blend, opacity, rename, delete.
    if let Some(layer) = ed.comp.layers.get_mut(ed.active) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Blend");
            egui::ComboBox::from_id_salt("layer-blend")
                .selected_text(blend_label(layer.blend))
                .show_ui(ui, |ui| {
                    for b in [
                        Blend::Normal,
                        Blend::Add,
                        Blend::Screen,
                        Blend::Multiply,
                        Blend::Max,
                    ] {
                        if ui
                            .selectable_value(&mut layer.blend, b, blend_label(b))
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });
            ui.label("Opacity");
            if ui
                .add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0))
                .changed()
            {
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Layer name");
            if ui
                .add(egui::TextEdit::singleline(&mut layer.name).desired_width(160.0))
                .changed()
            {
                changed = true;
            }
        });
        if changed {
            ed.dirty = true;
        }
    }

    if count > 1 && ui.button("🗑 Delete layer").clicked() {
        ed.snapshot();
        ed.comp.layers.remove(ed.active);
        ed.clamp();
        ed.dirty = true;
    }
}

fn blend_label(b: Blend) -> &'static str {
    match b {
        Blend::Normal => "Normal",
        Blend::Add => "Add",
        Blend::Screen => "Screen",
        Blend::Multiply => "Multiply",
        Blend::Max => "Max",
    }
}

fn brush_bar(ui: &mut egui::Ui, ed: &mut Editor) {
    ui.horizontal(|ui| {
        ui.label("Brush");
        let mut rgb = [ed.brush.r, ed.brush.g, ed.brush.b];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            ed.brush = Rgb::new(rgb[0], rgb[1], rgb[2]);
        }
        egui::ComboBox::from_id_salt("brush-mode")
            .selected_text(ed.mode.label())
            .show_ui(ui, |ui| {
                for m in BrushMode::ALL {
                    ui.selectable_value(&mut ed.mode, m, m.label());
                }
            });
        if ui.button("Fill all").clicked() {
            ed.snapshot();
            let b = ed.brush;
            ed.fill_all(b);
        }
        if ui.button("Clear").clicked() {
            ed.snapshot();
            ed.fill_all(Rgb::BLACK);
        }
        ui.separator();
        let can_undo = !ed.undo.is_empty();
        let can_redo = !ed.redo.is_empty();
        if ui
            .add_enabled(can_undo, egui::Button::new("↶ Undo"))
            .clicked()
        {
            ed.undo();
        }
        if ui
            .add_enabled(can_redo, egui::Button::new("↷ Redo"))
            .clicked()
        {
            ed.redo();
        }
    });

    ui.horizontal(|ui| {
        ui.label("Gradient 2nd colour");
        let mut b = [ed.brush_b.r, ed.brush_b.g, ed.brush_b.b];
        if ui.color_edit_button_srgb(&mut b).changed() {
            ed.brush_b = Rgb::new(b[0], b[1], b[2]);
        }
        ui.separator();
        if ui
            .button("📋 Copy")
            .on_hover_text("Copy this keyframe's colours")
            .clicked()
        {
            ed.clip = ed
                .anim()
                .and_then(|a| a.keyframes.get(ed.selected))
                .map(|k| k.colors.clone());
        }
        let can_paste = ed.clip.is_some();
        if ui
            .add_enabled(can_paste, egui::Button::new("Paste"))
            .clicked()
        {
            if let Some(colors) = ed.clip.clone() {
                ed.snapshot();
                if let Some(k) = ed.cur_kf_mut() {
                    k.colors = colors;
                }
                ed.dirty = true;
            }
        }
    });
}

fn board_area(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos], editable: bool) {
    if editable {
        ui.horizontal(|ui| {
            if ui
                .button("🎨 Apply gradient")
                .on_hover_text("Fill the active layer with brush → second colour")
                .clicked()
            {
                ed.snapshot();
                ed.apply_gradient(layout);
            }
        });
    }

    let sense = if editable {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let desired = egui::vec2(ui.available_width(), 200.0);
    let (rect, resp) = ui.allocate_exact_size(desired, sense);
    let frame = ed.preview_frame(ed.comp.leds, layout);
    let geom: BoardGeom = board::draw_board(ui, rect, layout, &frame);

    if !editable {
        return;
    }

    let secondary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
    let erase = secondary_down || resp.secondary_clicked();
    let color = if erase { Rgb::BLACK } else { ed.brush };

    match ed.mode {
        BrushMode::Region => region_interaction(ui, ed, &resp, &geom, rect, color),
        BrushMode::Pick => {
            if resp.clicked() {
                if let Some(led) = resp.interact_pointer_pos().and_then(|p| geom.led_at(p)) {
                    if let Some(c) = ed
                        .anim()
                        .and_then(|a| a.keyframes.get(ed.selected))
                        .map(|k| k.color(led))
                    {
                        ed.brush = c;
                    }
                }
            }
        }
        _ => {
            if resp.drag_started() || (resp.clicked() && !ed.stroke_open) {
                ed.snapshot();
                ed.stroke_open = true;
            }
            if let Some(led) = resp.interact_pointer_pos().and_then(|p| geom.led_at(p)) {
                if resp.clicked() || resp.dragged() || resp.secondary_clicked() {
                    ed.brush_at(led, color, layout);
                }
            }
            if resp.drag_stopped() || resp.clicked() {
                ed.stroke_open = false;
            }
        }
    }
}

fn region_interaction(
    ui: &egui::Ui,
    ed: &mut Editor,
    resp: &egui::Response,
    geom: &BoardGeom,
    rect: egui::Rect,
    color: Rgb,
) {
    if resp.drag_started() {
        ed.region_start = resp.interact_pointer_pos();
    }
    if let (Some(start), Some(cur)) = (ed.region_start, resp.interact_pointer_pos()) {
        let sel = egui::Rect::from_two_pos(start, cur).intersect(rect);
        ui.painter_at(rect).rect_stroke(
            sel,
            2.0,
            egui::Stroke::new(1.5_f32, theme::current().accent),
        );
    }
    if resp.drag_stopped() {
        if let (Some(start), Some(end)) = (ed.region_start.take(), resp.interact_pointer_pos()) {
            ed.snapshot();
            let sel = egui::Rect::from_two_pos(start, end);
            for led in geom.leds_in(sel) {
                ed.paint(led, color);
            }
        }
    }
}

fn timeline_controls(ui: &mut egui::Ui, ed: &mut Editor) {
    ui.horizontal(|ui| {
        if ui.button(if ed.playing { "⏸" } else { "▶" }).clicked() {
            ed.playing = !ed.playing;
        }
        ui.label("Duration");
        if ui
            .add(
                egui::DragValue::new(&mut ed.comp.duration)
                    .range(0.1..=60.0)
                    .suffix(" s"),
            )
            .changed()
        {
            ed.sync_timeline();
            ed.dirty = true;
        }
        ui.checkbox(&mut ed.comp.loops, "Loop");
        ui.separator();
        if ui.button("➕ Keyframe").clicked() {
            ed.add_keyframe();
        }
        if ui.button("🗑 Delete").clicked() {
            ed.delete_keyframe();
        }
        let total = ed.anim().map(|a| a.keyframes.len()).unwrap_or(0);
        ui.label(format!("{} of {}", ed.selected + 1, total));
    });

    if let Some(a) = ed.anim_mut() {
        ui.horizontal(|ui| {
            let mut interp = a.interpolate;
            if ui
                .checkbox(&mut interp, "Blend between keyframes")
                .on_hover_text("Off = hard cuts, like a flipbook")
                .changed()
            {
                a.interpolate = interp;
            }
        });
    }
}

fn keyframe_controls(ui: &mut egui::Ui, ed: &mut Editor) {
    let mut t = ed
        .anim()
        .and_then(|a| a.keyframes.get(ed.selected))
        .map(|k| k.t);
    let mut e = ed
        .anim()
        .and_then(|a| a.keyframes.get(ed.selected))
        .map(|k| k.ease);
    let (Some(t), Some(e)) = (t.as_mut(), e.as_mut()) else {
        return;
    };

    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Keyframe time");
        if ui
            .add(
                egui::DragValue::new(t)
                    .range(0.0..=60.0)
                    .speed(0.02)
                    .suffix(" s"),
            )
            .changed()
        {
            changed = true;
        }
        ui.label("Ease out");
        egui::ComboBox::from_id_salt("kf-ease")
            .selected_text(e.label())
            .show_ui(ui, |ui| {
                for opt in Ease::ALL {
                    if ui.selectable_value(e, opt, opt.label()).changed() {
                        changed = true;
                    }
                }
            });
    });

    if changed {
        let sel = ed.selected;
        if let Some(a) = ed.anim_mut() {
            if let Some(k) = a.keyframes.get_mut(sel) {
                k.t = *t;
                k.ease = *e;
            }
        }
        ed.dirty = true;
    }
}

fn generator_controls(ui: &mut egui::Ui, ed: &mut Editor) {
    // Play/pause touches `ed.playing`, so handle it before borrowing the
    // generator params mutably.
    if ui
        .button(if ed.playing {
            "⏸ Pause"
        } else {
            "▶ Preview"
        })
        .clicked()
    {
        ed.playing = !ed.playing;
    }
    let Some(g) = ed.gen_mut() else { return };
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Motion");
        egui::ComboBox::from_id_salt("gen-kind")
            .selected_text(g.kind.label())
            .show_ui(ui, |ui| {
                for k in Generator::ALL {
                    if ui.selectable_value(&mut g.kind, k, k.label()).changed() {
                        changed = true;
                    }
                }
            });
        if g.kind.uses_direction() {
            egui::ComboBox::from_id_salt("gen-dir")
                .selected_text(g.direction.label())
                .show_ui(ui, |ui| {
                    for d in Direction::ALL {
                        if ui
                            .selectable_value(&mut g.direction, d, d.label())
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });
        }
    });

    if g.kind.uses_colors() {
        ui.horizontal(|ui| {
            ui.label("Colours");
            let mut a = [g.color_a.r, g.color_a.g, g.color_a.b];
            if ui.color_edit_button_srgb(&mut a).changed() {
                g.color_a = Rgb::new(a[0], a[1], a[2]);
                changed = true;
            }
            let mut b = [g.color_b.r, g.color_b.g, g.color_b.b];
            if ui.color_edit_button_srgb(&mut b).changed() {
                g.color_b = Rgb::new(b[0], b[1], b[2]);
                changed = true;
            }
            ui.weak("foreground / background");
        });
    }

    ui.horizontal(|ui| {
        ui.label("Width");
        changed |= ui
            .add(egui::Slider::new(&mut g.width, 0.05..=1.0))
            .changed();
        ui.label("Passes");
        changed |= ui
            .add(egui::Slider::new(&mut g.cycles, 0.25..=8.0))
            .changed();
        ui.label("Smoothness");
        changed |= ui.add(egui::Slider::new(&mut g.frames, 2..=120)).changed();
    });

    if changed {
        ed.dirty = true;
    }
}

/// The timeline: one marker per keyframe of the active layer.
fn timeline_strip(ui: &mut egui::Ui, ed: &mut Editor) {
    let pal = theme::current();
    let h = 34.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), h),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, pal.surface_alt);

    let dur = ed.comp.duration.max(0.001);
    let x_of = |t: f32| rect.left() + 8.0 + (t / dur) * (rect.width() - 16.0);

    if ed.playing {
        let x = x_of(ed.playhead);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.5_f32, pal.text_muted),
        );
    }

    let Some(anim) = ed.anim() else { return };
    let mut clicked_idx = None;
    for (i, k) in anim.keyframes.iter().enumerate() {
        let x = x_of(k.t);
        let c = egui::pos2(x, rect.center().y);
        let selected = i == ed.selected;
        let r = if selected { 8.0 } else { 6.0 };
        let col = if selected { pal.accent } else { pal.text_muted };
        painter.circle_filled(c, r, col);
        if let Some(tint) = dominant(k) {
            painter.circle_filled(c, r - 3.0, tint);
        }
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
                if let Some(a) = ed.anim_mut() {
                    a.keyframes[i].t = t;
                }
                ed.selected = i;
                ed.dirty = true;
            }
        }
    }
}

fn dominant(k: &Keyframe) -> Option<egui::Color32> {
    k.colors
        .iter()
        .map(|c| c.to_rgb())
        .filter(|c| !c.is_black())
        .max_by_key(|c| c.r as u32 + c.g as u32 + c.b as u32)
        .map(|c| egui::Color32::from_rgb(c.r, c.g, c.b))
}

fn lerp_rgb(a: Rgb, b: Rgb, f: f32) -> Rgb {
    let f = f.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * f)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
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
        "effect".into()
    } else {
        s.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_anim(ed: &Editor) -> &Animation {
        match &ed.comp.layers[0].content {
            LayerContent::Keyframes(a) => a,
            _ => panic!("layer 0 should be keyframes"),
        }
    }

    #[test]
    fn painting_marks_dirty_and_sets_the_colour() {
        let mut ed = Editor::new(10);
        ed.paint(3, Rgb::new(1, 2, 3));
        assert!(ed.dirty);
        assert_eq!(base_anim(&ed).keyframes[0].color(3), Rgb::new(1, 2, 3));
    }

    #[test]
    fn adding_a_keyframe_duplicates_the_current_one() {
        let mut ed = Editor::new(4);
        ed.paint(0, Rgb::new(9, 9, 9));
        let before = base_anim(&ed).keyframes.len();
        ed.add_keyframe();
        assert_eq!(base_anim(&ed).keyframes.len(), before + 1);
        assert_eq!(
            base_anim(&ed).keyframes[ed.selected].color(0),
            Rgb::new(9, 9, 9)
        );
    }

    #[test]
    fn cannot_delete_the_last_keyframe() {
        let mut ed = Editor::new(2);
        // A fresh layer has two keyframes; delete down to one, then it refuses.
        while base_anim(&ed).keyframes.len() > 1 {
            ed.delete_keyframe();
        }
        assert_eq!(base_anim(&ed).keyframes.len(), 1);
        ed.delete_keyframe();
        assert_eq!(base_anim(&ed).keyframes.len(), 1);
        assert!(ed.status.is_some());
    }

    #[test]
    fn undo_restores_the_previous_colours() {
        let mut ed = Editor::new(4);
        ed.snapshot();
        ed.paint(1, Rgb::new(50, 60, 70));
        assert_eq!(base_anim(&ed).keyframes[0].color(1), Rgb::new(50, 60, 70));
        ed.undo();
        assert_eq!(base_anim(&ed).keyframes[0].color(1), Rgb::BLACK);
        ed.redo();
        assert_eq!(base_anim(&ed).keyframes[0].color(1), Rgb::new(50, 60, 70));
    }

    #[test]
    fn adding_a_generator_layer_makes_it_active_and_non_paintable() {
        let mut ed = Editor::new(4);
        ed.comp
            .layers
            .push(Layer::generator("g", GenParams::default()));
        ed.active = ed.comp.layers.len() - 1;
        assert!(!ed.active_is_keyframes());
        // Painting a generator layer is a no-op on keyframes (there are none).
        ed.paint(0, Rgb::WHITE);
        assert_eq!(base_anim(&ed).keyframes[0].color(0), Rgb::BLACK);
    }

    #[test]
    fn file_names_are_sanitised() {
        assert_eq!(sanitise("My Cool FX!"), "my-cool-fx-");
        assert_eq!(sanitise("   "), "effect");
    }
}
