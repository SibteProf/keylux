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
    pub editing_keys: bool,
    pub playhead: f32,
    pub playing: bool,
    pub dirty: bool,
    pub file_name: String,
    pub status: Option<String>,
    /// Stream the composited preview to the physical keyboard while editing.
    pub live: bool,
    pub frame_requested: bool,
    pub nvram_result: Option<Result<(), String>>,

    /// Layer-stack snapshots for undo (newest last) and redo.
    undo: Vec<Vec<Layer>>,
    redo: Vec<Vec<Layer>>,
    clip: Option<Vec<HexColor>>,
    region_start: Option<egui::Pos2>,
    stroke_open: bool,
    keyboard_key: usize,
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
            editing_keys: false,
            playhead: 0.0,
            playing: false,
            dirty: false,
            frame_requested: false,
            nvram_result: None,
            file_name,
            status: None,
            live: false,
            undo: Vec::new(),
            redo: Vec::new(),
            clip: None,
            region_start: None,
            stroke_open: false,
            keyboard_key: 0,
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
        if led >= self.comp.leds {
            return;
        }
        if self.editing_keys {
            self.set_key_membership(led, !color.is_black());
            return;
        }
        if !self.comp.layers[self.active].affects(led) {
            return;
        }
        if let Some(k) = self.cur_kf_mut() {
            if led < k.colors.len() {
                k.colors[led] = HexColor::from(color);
            }
        }
        self.dirty = true;
    }

    pub fn fill_all(&mut self, color: Rgb) {
        let keys = self.comp.layers[self.active].keys.clone();
        if let Some(k) = self.cur_kf_mut() {
            for (led, c) in k.colors.iter_mut().enumerate() {
                if keys.as_ref().map_or(true, |keys| keys.contains(&led)) {
                    *c = HexColor::from(color);
                }
            }
        }
        self.dirty = true;
    }

    fn set_key_membership(&mut self, led: usize, included: bool) {
        if led >= self.comp.leds {
            return;
        }
        let layer = &mut self.comp.layers[self.active];
        if layer.affects(led) == included {
            return;
        }
        let keys = layer
            .keys
            .get_or_insert_with(|| (0..self.comp.leds).collect());
        if included {
            keys.push(led);
            keys.sort_unstable();
        } else {
            keys.retain(|&key| key != led);
        }
        self.dirty = true;
    }

    fn add_static_layer(&mut self) {
        self.snapshot();
        let mut anim = Animation::new("Static", self.comp.leds);
        anim.keyframes = vec![Keyframe {
            t: 0.0,
            colors: vec![HexColor::from(self.brush); self.comp.leds],
            ease: Ease::default(),
        }];
        anim.duration = self.comp.duration;
        anim.loops = self.comp.loops;
        let mut layer = Layer::keyframes("Static", anim);
        layer.blend = Blend::Normal;
        self.comp.layers.push(layer);
        self.active = self.comp.layers.len() - 1;
        self.selected = 0;
        self.editing_keys = true;
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
pub fn show(
    ui: &mut egui::Ui,
    ed: &mut Editor,
    layout: &[KeyPos],
    anim_dir: &std::path::Path,
    can_save: bool,
    can_stream: bool,
) {
    handle_shortcuts(ui, ed);
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
        ed.save(anim_dir);
    }
    egui::ScrollArea::vertical()
        .id_salt("composition-editor")
        .show(ui, |ui| {
            show_contents(ui, ed, layout, anim_dir, can_save, can_stream);
        });
}

fn show_contents(
    ui: &mut egui::Ui,
    ed: &mut Editor,
    layout: &[KeyPos],
    anim_dir: &std::path::Path,
    can_save: bool,
    can_stream: bool,
) {
    top_bar(ui, ed, anim_dir, can_save);
    ui.add_space(12.0);
    if ui.available_width() >= 860.0 {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(232.0, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::surface(ui, |ui| {
                        layer_panel(ui, ed);
                    });
                },
            );
            ui.add_space(6.0);
            ui.vertical(|ui| {
                canvas_and_tools(ui, ed, layout, can_stream);
            });
        });
    } else {
        canvas(ui, ed, layout, can_stream);
        ui.add_space(14.0);
        theme::surface(ui, |ui| {
            egui::CollapsingHeader::new(format!("Layers · {}", ed.comp.layers.len()))
                .default_open(false)
                .show(ui, |ui| {
                    layer_panel(ui, ed);
                });
        });
        ui.add_space(14.0);
        layer_tools(ui, ed, layout);
    }
    if let Some(status) = &ed.status {
        ui.add_space(12.0);
        theme::muted(ui, status);
    }
}

fn canvas_and_tools(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos], can_stream: bool) {
    canvas(ui, ed, layout, can_stream);
    ui.add_space(14.0);
    layer_tools(ui, ed, layout);
}

fn canvas(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos], can_stream: bool) {
    theme::surface(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Keyboard canvas").size(17.0).strong());
            theme::muted(ui, &ed.comp.layers[ed.active].name);
            ui.add_space(6.0);
            ui.add_enabled(
                can_stream,
                egui::Checkbox::new(&mut ed.live, "Live preview"),
            )
            .on_hover_text("Show canvas changes on the connected keyboard.")
            .on_disabled_hover_text("Connect a keyboard with the same LED count to preview live.");
        });
        key_mask_controls(ui, ed, layout);
        if !ed.editing_keys && ed.active_is_keyframes() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Brush");
                let mut rgb = [ed.brush.r, ed.brush.g, ed.brush.b];
                if theme::color_edit(ui, "Brush colour", &mut rgb).changed() {
                    ed.brush = Rgb::new(rgb[0], rgb[1], rgb[2]);
                }
                egui::ComboBox::from_id_salt("brush-mode")
                    .width(120.0)
                    .selected_text(ed.mode.label())
                    .show_ui(ui, |ui| {
                        for mode in BrushMode::ALL {
                            ui.selectable_value(&mut ed.mode, mode, mode.label());
                        }
                    });
            });
        }
        ui.add_space(6.0);
        board_area(ui, ed, layout, ed.editing_keys || ed.active_is_keyframes());
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(if ed.editing_keys {
                "Click to include · Right-click to exclude · Drag to select several"
            } else if ed.active_is_keyframes() {
                "Click to paint · Right-click to erase · Drag to paint several"
            } else {
                "Previewing all layers · Choose Select keys to limit this motion"
            })
            .small()
            .color(theme::current().text_muted),
        );
        egui::CollapsingHeader::new("Keyboard controls").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let label = layout
                    .iter()
                    .find(|key| key.led == ed.keyboard_key)
                    .map(|key| key.name.as_str())
                    .unwrap_or("Choose a key");
                egui::ComboBox::from_id_salt("canvas-key")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for key in layout {
                            ui.selectable_value(&mut ed.keyboard_key, key.led, &key.name);
                        }
                    });
                if ui
                    .add_enabled(
                        ed.editing_keys || ed.active_is_keyframes(),
                        egui::Button::new(if ed.editing_keys {
                            "Include key"
                        } else {
                            "Paint key"
                        }),
                    )
                    .clicked()
                {
                    ed.snapshot();
                    ed.paint(
                        ed.keyboard_key,
                        if ed.editing_keys {
                            Rgb::WHITE
                        } else {
                            ed.brush
                        },
                    );
                }
                if ui
                    .add_enabled(
                        ed.editing_keys || ed.active_is_keyframes(),
                        egui::Button::new(if ed.editing_keys {
                            "Exclude key"
                        } else {
                            "Erase key"
                        }),
                    )
                    .clicked()
                {
                    ed.snapshot();
                    ed.paint(ed.keyboard_key, Rgb::BLACK);
                }
            });
        });
    });
}

fn layer_tools(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos]) {
    if ed.editing_keys {
        return;
    }
    theme::surface(ui, |ui| {
        if ed.active_is_keyframes() {
            theme::section(
                ui,
                "Animation & tools",
                "Fill, blend, and animate this layer's selected keys.",
            );
            brush_bar(ui, ed);
            if ui.button("Apply gradient").clicked() {
                ed.snapshot();
                ed.apply_gradient(layout);
            }
            ui.add_space(12.0);
            egui::CollapsingHeader::new("Animation timeline")
                .default_open(true)
                .show(ui, |ui| {
                    timeline_controls(ui, ed);
                    timeline_strip(ui, ed);
                    keyframe_controls(ui, ed);
                });
        } else {
            theme::section(ui, "Motion", "Tune the animation on this layer.");
            generator_controls(ui, ed);
        }
    });
}

fn handle_shortcuts(ui: &egui::Ui, ed: &mut Editor) {
    // Text fields own their undo history while they have keyboard focus.
    if ui.ctx().wants_keyboard_input() {
        return;
    }
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

fn top_bar(ui: &mut egui::Ui, ed: &mut Editor, anim_dir: &std::path::Path, can_save: bool) {
    let pal = theme::current();
    let persisted = anim_dir
        .join(format!("{}.klx", sanitise(&ed.file_name)))
        .is_file();
    let status = if ed.dirty {
        "Unsaved changes"
    } else if persisted {
        "All changes saved"
    } else {
        "Draft · Not saved yet"
    };
    let name_width = (ui.available_width() - 170.0).clamp(140.0, 300.0);
    ui.horizontal_wrapped(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(name_width, 58.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            let name = ui.add(egui::TextEdit::singleline(&mut ed.comp.name)
                .desired_width(name_width - 10.0).hint_text("Composition name").font(egui::FontId::proportional(18.0)));
            name.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Composition name"));
            if name.changed() { ed.dirty = true; }
            ui.label(egui::RichText::new(status).small().color(pal.text_muted));
        });
        ui.add_space(8.0);
        if ui.add(theme::primary_button("Save")).on_hover_text("Save composition (Ctrl/Cmd+S)").clicked() { ed.save(anim_dir); }
        ui.menu_button("File", |ui| {
            ui.label("File name");
            ui.text_edit_singleline(&mut ed.file_name);
            ui.add_space(8.0);
            if ui.add_enabled(can_save, egui::Button::new("Save current frame to keyboard"))
                .on_hover_text("Permanently stores this single frame on the keyboard. Animation still needs keylux running.")
                .on_disabled_hover_text("Permanent storage requires a supported native AULA connection.")
                .clicked() { ed.frame_requested = true; ed.nvram_result = None; ui.close_menu(); }
        });
    });
    if ed.frame_requested {
        theme::muted(ui, "Saving frame to keyboard…");
    } else if matches!(ed.nvram_result, Some(Ok(()))) {
        ui.colored_label(pal.success, "Frame saved to keyboard");
    }
    if let Some(Err(message)) = &ed.nvram_result {
        ui.colored_label(pal.danger, format!("Could not save frame: {message}"));
    }
}

/// The layer stack: add / reorder / blend / opacity / enable, and pick active.
fn layer_panel(ui: &mut egui::Ui, ed: &mut Editor) {
    theme::section(ui, "Layers", "Top layers appear above those below.");
    ui.menu_button("Add layer", |ui| {
        if ui
            .button("Static colour")
            .on_hover_text(
                "Add a fixed-colour layer, then choose its keys. Use Paint to change its colour.",
            )
            .clicked()
        {
            ed.add_static_layer();
            ui.close_menu();
        }
        if ui
            .button("Painted animation")
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
            ui.close_menu();
        }
        if ui
            .button("Motion generator")
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
            ui.close_menu();
        }
    });

    // Layers are drawn top-first so the list reads like the visual stack.
    let count = ed.comp.layers.len();
    let mut to_select = None;
    let mut reorder: Option<(usize, isize)> = None;
    let mut toggle: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("layer-stack")
        .min_scrolled_height(190.0)
        .max_height(190.0)
        .show(ui, |ui| {
            for i in (0..count).rev() {
                let layer = &ed.comp.layers[i];
                let name = layer.name.clone();
                let enabled = layer.enabled;
                let selected = i == ed.active;
                let kind = match &layer.content {
                    LayerContent::Keyframes(animation) if animation.keyframes.len() == 1 => {
                        "Static colour"
                    }
                    LayerContent::Keyframes(_) => "Painted animation",
                    LayerContent::Generator(params) => params.kind.label(),
                };
                let pal = theme::current();
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 58.0),
                    egui::Sense::click(),
                );
                let fill = if selected {
                    pal.surface_alt
                } else if response.hovered() {
                    pal.hovered_bg
                } else {
                    pal.surface
                };
                ui.painter().rect_filled(rect, 6.0, fill);
                if selected || response.has_focus() {
                    ui.painter()
                        .rect_stroke(rect, 6.0, egui::Stroke::new(1.0_f32, pal.accent));
                }
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        &name,
                    )
                });
                ui.allocate_new_ui(
                    egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(10.0, 6.0))),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2((ui.available_width() - 36.0).max(40.0), 46.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(&name).strong())
                                            .truncate(),
                                    );
                                    ui.label(
                                        egui::RichText::new(if enabled {
                                            kind.to_owned()
                                        } else {
                                            format!("{kind} · Hidden")
                                        })
                                        .small()
                                        .color(pal.text_muted),
                                    );
                                },
                            );
                            let mut visible = enabled;
                            let visibility = ui
                                .checkbox(&mut visible, "")
                                .on_hover_text(format!("Show or hide {name}"));
                            visibility.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::Checkbox,
                                    true,
                                    visible,
                                    format!("Show {name}"),
                                )
                            });
                            if visibility.changed() {
                                toggle = Some(i);
                            }
                        });
                    },
                );
                if response.clicked() {
                    to_select = Some(i);
                }
                response.context_menu(|ui| {
                    if ui
                        .add_enabled(i + 1 < count, egui::Button::new("Move up"))
                        .clicked()
                    {
                        reorder = Some((i, 1));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(i > 0, egui::Button::new("Move down"))
                        .clicked()
                    {
                        reorder = Some((i, -1));
                        ui.close_menu();
                    }
                });
            }
        });

    if count > 1 {
        ui.horizontal_wrapped(|ui| {
            for (glyph, label, allowed, direction) in [
                ("↑", "Move selected layer up", ed.active + 1 < count, 1),
                ("↓", "Move selected layer down", ed.active > 0, -1),
            ] {
                let response = ui
                    .add_enabled(
                        allowed,
                        egui::Button::new(glyph).min_size(egui::vec2(32.0, 28.0)),
                    )
                    .on_hover_text(label);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, allowed, label)
                });
                if response.clicked() {
                    reorder = Some((ed.active, direction));
                }
            }
            theme::muted(ui, "Layer order");
        });
    }
    ui.add_space(14.0);
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

    if let Some(layer) = ed.comp.layers.get_mut(ed.active) {
        let name_label = ui.label("Layer name");
        let mut changed = ui
            .add(egui::TextEdit::singleline(&mut layer.name).desired_width(f32::INFINITY))
            .labelled_by(name_label.id)
            .changed();
        ui.label("Blend mode");
        egui::ComboBox::from_id_salt("layer-blend")
            .width(ui.available_width() - 12.0)
            .selected_text(blend_label(layer.blend))
            .show_ui(ui, |ui| {
                for blend in [
                    Blend::Normal,
                    Blend::Add,
                    Blend::Screen,
                    Blend::Multiply,
                    Blend::Max,
                ] {
                    changed |= ui
                        .selectable_value(&mut layer.blend, blend, blend_label(blend))
                        .changed();
                }
            });
        ui.label("Opacity");
        ui.spacing_mut().slider_width = (ui.available_width() - 55.0).max(70.0);
        changed |= ui
            .add(
                egui::Slider::new(&mut layer.opacity, 0.0..=1.0)
                    .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
            )
            .changed();
        if changed {
            ed.dirty = true;
        }
    }
    ui.add_space(10.0);
    if count > 1 && ui.button("Delete layer").clicked() {
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
    ui.horizontal_wrapped(|ui| {
        if ui.button("Fill selected").clicked() {
            ed.snapshot();
            let b = ed.brush;
            ed.fill_all(b);
        }
        if ui.button("Clear selected").clicked() {
            ed.snapshot();
            ed.fill_all(Rgb::BLACK);
        }
        ui.separator();
        let can_undo = !ed.undo.is_empty();
        let can_redo = !ed.redo.is_empty();
        if ui
            .add_enabled(can_undo, egui::Button::new("Undo"))
            .clicked()
        {
            ed.undo();
        }
        if ui
            .add_enabled(can_redo, egui::Button::new("Redo"))
            .clicked()
        {
            ed.redo();
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label("Gradient 2nd colour");
        let mut b = [ed.brush_b.r, ed.brush_b.g, ed.brush_b.b];
        if theme::color_edit(ui, "Gradient second colour", &mut b).changed() {
            ed.brush_b = Rgb::new(b[0], b[1], b[2]);
        }
        ui.separator();
        if ui
            .button("Copy frame")
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
                let keys = ed.comp.layers[ed.active].keys.clone();
                if let Some(k) = ed.cur_kf_mut() {
                    for (led, (dst, src)) in k.colors.iter_mut().zip(colors).enumerate() {
                        if keys.as_ref().map_or(true, |keys| keys.contains(&led)) {
                            *dst = src;
                        }
                    }
                }
                ed.dirty = true;
            }
        }
    });
}

fn key_mask_controls(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos]) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .toggle_value(&mut ed.editing_keys, "Select keys")
            .changed()
        {
            ed.region_start = None;
            ed.stroke_open = false;
            if ed.mode == BrushMode::Pick {
                ed.mode = BrushMode::Key;
            }
        }
        let count = layout
            .iter()
            .filter(|key| ed.comp.layers[ed.active].affects(key.led))
            .count();
        ui.label(format!("{count}/{} keys", layout.len()));
        for (label, action) in [("All", 0), ("None", 1), ("Invert", 2)] {
            if ui.button(label).clicked() {
                ed.snapshot();
                let layer = &mut ed.comp.layers[ed.active];
                layer.keys = match action {
                    0 => None,
                    1 => Some(Vec::new()),
                    _ => Some(
                        (0..ed.comp.leds)
                            .filter(|&led| !layer.affects(led))
                            .collect(),
                    ),
                };
                ed.dirty = true;
            }
        }
        if ed.editing_keys {
            for mode in [
                BrushMode::Key,
                BrushMode::Row,
                BrushMode::Column,
                BrushMode::Region,
            ] {
                ui.selectable_value(&mut ed.mode, mode, mode.label());
            }
            if ui
                .add_enabled(!ed.undo.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                ed.undo();
            }
            if ui
                .add_enabled(!ed.redo.is_empty(), egui::Button::new("Redo"))
                .clicked()
            {
                ed.redo();
            }
        }
    });
}

fn board_area(ui: &mut egui::Ui, ed: &mut Editor, layout: &[KeyPos], editable: bool) {
    let sense = if editable {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let desired = egui::vec2(
        ui.available_width(),
        (ui.available_width() * 0.32).clamp(165.0, 235.0),
    );
    let (rect, resp) = ui.allocate_exact_size(desired, sense);
    let frame = ed.preview_frame(ed.comp.leds, layout);
    let geom: BoardGeom = board::draw_board(ui, rect, layout, &frame);
    if ed.editing_keys {
        geom.outline_keys(ui, rect, |led| ed.comp.layers[ed.active].affects(led));
    }

    if !editable {
        return;
    }

    let secondary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
    let erase = secondary_down
        || resp.secondary_clicked()
        || resp.drag_stopped_by(egui::PointerButton::Secondary);
    let color = if erase {
        Rgb::BLACK
    } else if ed.editing_keys {
        Rgb::WHITE
    } else {
        ed.brush
    };

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
            if resp.drag_started()
                || ((resp.clicked() || resp.secondary_clicked()) && !ed.stroke_open)
            {
                ed.snapshot();
                ed.stroke_open = true;
            }
            if let Some(led) = resp.interact_pointer_pos().and_then(|p| geom.led_at(p)) {
                if resp.clicked() || resp.dragged() || resp.secondary_clicked() {
                    ed.brush_at(led, color, layout);
                }
            }
            if resp.drag_stopped() || resp.clicked() || resp.secondary_clicked() {
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
        ed.region_start = ui
            .input(|i| i.pointer.press_origin())
            .or(resp.interact_pointer_pos());
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
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(if ed.playing { "Pause" } else { "Play" })
            .clicked()
        {
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
        if ui.button("Add keyframe").clicked() {
            ed.add_keyframe();
        }
        if ui.button("Delete keyframe").clicked() {
            ed.delete_keyframe();
        }
        let total = ed.anim().map(|a| a.keyframes.len()).unwrap_or(0);
        ui.label(format!("{} of {}", ed.selected + 1, total));
    });

    if let Some(a) = ed.anim_mut() {
        ui.horizontal_wrapped(|ui| {
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
    ui.horizontal_wrapped(|ui| {
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
    });
    ui.horizontal_wrapped(|ui| {
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
            "Pause preview"
        } else {
            "Play preview"
        })
        .clicked()
    {
        ed.playing = !ed.playing;
    }
    let Some(g) = ed.gen_mut() else { return };
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
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
    });
    if g.kind.uses_direction() {
        ui.horizontal_wrapped(|ui| {
            ui.label("Direction");
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
        });
    }

    if g.kind.uses_colors() {
        ui.horizontal_wrapped(|ui| {
            ui.label("Colours");
            let mut a = [g.color_a.r, g.color_a.g, g.color_a.b];
            if theme::color_edit(ui, "Motion foreground colour", &mut a).changed() {
                g.color_a = Rgb::new(a[0], a[1], a[2]);
                changed = true;
            }
            let mut b = [g.color_b.r, g.color_b.g, g.color_b.b];
            if theme::color_edit(ui, "Motion background colour", &mut b).changed() {
                g.color_b = Rgb::new(b[0], b[1], b[2]);
                changed = true;
            }
            theme::muted(ui, "foreground / background");
        });
    }

    ui.spacing_mut().slider_width = (ui.available_width() - 155.0).clamp(70.0, 180.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("Width");
        changed |= ui
            .add(egui::Slider::new(&mut g.width, 0.05..=1.0))
            .changed();
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Passes");
        changed |= ui
            .add(egui::Slider::new(&mut g.cycles, 0.25..=8.0))
            .changed();
    });
    ui.horizontal_wrapped(|ui| {
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

    #[test]
    fn editor_layout_fits_compact_regular_and_large_workspaces() {
        let layout = aula_protocol::f75::keymap::layout();
        for (width, height, large_text) in [
            (334.0, 440.0, false),
            (514.0, 440.0, false),
            (934.0, 650.0, false),
            (1354.0, 780.0, false),
            (450.0, 600.0, true),
        ] {
            let ctx = egui::Context::default();
            theme::DARK.apply(&ctx);
            if large_text {
                let mut style = (*ctx.style()).clone();
                for font in style.text_styles.values_mut() {
                    font.size *= 1.4;
                }
                ctx.set_style(style);
            }
            let mut editor = Editor::new(126);
            for n in 0..12 {
                editor.comp.layers.push(Layer::generator(
                    format!("Motion layer {n}"),
                    GenParams::default(),
                ));
            }
            // Two passes allow egui's scrollbars and wrap measurements to settle.
            for (active, selecting) in [(0, false), (1, false), (1, true)] {
                editor.active = active;
                editor.editing_keys = selecting;
                for _ in 0..2 {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                show(
                                    ui,
                                    &mut editor,
                                    &layout,
                                    std::path::Path::new("animations"),
                                    false,
                                    false,
                                );
                                assert!(
                                    ui.min_rect().right() <= width + 1.0,
                                    "Editor overflows at width {width}: {:?}",
                                    ui.min_rect()
                                );
                                assert!(ui.min_rect().bottom().is_finite());
                            });
                        },
                    );
                }
            }
        }
    }

    #[test]
    fn mask_selection_has_history_and_does_not_paint_colours() {
        let mut ed = Editor::new(4);
        ed.editing_keys = true;
        ed.snapshot();
        ed.paint(1, Rgb::BLACK);
        assert!(!ed.comp.layers[0].affects(1));
        assert!(ed.comp.layers[0].affects(0));
        assert!(ed.dirty);
        ed.undo();
        assert!(ed.comp.layers[0].affects(1));
        ed.redo();
        assert!(!ed.comp.layers[0].affects(1));
        ed.paint(1, Rgb::WHITE);
        ed.paint(999, Rgb::WHITE);
        assert!(ed.comp.layers[0].affects(1));
        assert!(base_anim(&ed).keyframes[0].color(1).is_black());
    }

    #[test]
    fn static_layer_stays_fixed_and_painting_respects_its_mask() {
        let mut ed = Editor::new(4);
        ed.brush = Rgb::new(100, 20, 30);
        ed.add_static_layer();
        assert_eq!(ed.comp.layers[ed.active].blend, Blend::Normal);
        assert_eq!(ed.anim().unwrap().keyframes.len(), 1);
        assert!(ed.editing_keys);
        ed.paint(0, Rgb::BLACK);
        ed.editing_keys = false;
        ed.fill_all(Rgb::WHITE);
        ed.paint(0, Rgb::BLACK);
        assert_eq!(
            ed.anim().unwrap().keyframes[0].color(0),
            Rgb::new(100, 20, 30)
        );
        assert_eq!(ed.anim().unwrap().keyframes[0].color(1), Rgb::WHITE);
        let mut a = Frame::black(4);
        let mut b = Frame::black(4);
        ed.comp.sample(0.0, &[], &mut a);
        ed.comp.sample(1.5, &[], &mut b);
        assert_eq!(a, b);
        assert!(a.get(0).is_black());
        assert_eq!(a.get(1), Rgb::WHITE);
    }

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
