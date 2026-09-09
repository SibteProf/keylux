//! egui front end.

use eframe::egui;

use aula_effects::{ParamKind, Params, Value};
use aula_protocol::Rgb;

use crate::editor::{self, Editor};
use crate::engine::{Cmd, DeviceStatus, Engine};
use crate::settings::{CloseAction, Settings};
use crate::tray::{Tray, TrayAction};
use crate::window_ctl::WindowRef;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Play,
    Create,
}

pub struct App {
    engine: Engine,
    /// The UI owns the parameter values and pushes the whole set on change, so
    /// UI and engine can never drift apart.
    params: Params,
    selected: usize,
    effects_dir: std::path::PathBuf,
    anim_dir: std::path::PathBuf,
    tab: Tab,
    editor: Option<Editor>,
    last_tick: std::time::Instant,
    notice: Option<String>,

    settings: Settings,
    /// `None` when the tray could not be created. Everything that hides the
    /// window checks this first — without a tray there would be no way back.
    tray: Option<Tray>,
    /// `None` on platforms with no way to show a hidden window from off the UI
    /// thread. Hiding is refused there for the same reason.
    window: Option<WindowRef>,
    /// The close button was pressed and we are waiting on the user's answer.
    confirm_close: bool,
    /// Tick the "don't ask again" box in that dialog.
    remember_choice: bool,
    /// The window is hidden in the tray.
    hidden: bool,
    /// A quit is already in flight, so the next close request is ours and must
    /// not be intercepted again.
    quitting: bool,
    /// Whether lighting was running when we hid, so restoring puts it back.
    resume_on_show: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, effects_dir: std::path::PathBuf) -> Self {
        let ctx = cc.egui_ctx.clone();
        let engine = Engine::spawn(effects_dir.clone(), move || ctx.request_repaint());
        let anim_dir = effects_dir
            .parent()
            .map(|p| p.join("animations"))
            .unwrap_or_else(|| std::path::PathBuf::from("animations"));
        let _ = std::fs::create_dir_all(&anim_dir);

        // Captured once at startup: the only way to reach a hidden window,
        // since egui cannot repaint one and so never runs `update` for it.
        let window = WindowRef::capture(cc);

        let tray = {
            let ctx = cc.egui_ctx.clone();
            let tx = engine.sender();
            let shared = std::sync::Arc::clone(&engine.shared);
            Tray::new(move |action| {
                match action {
                    // Show the window here and now. Everything else about the
                    // restore can wait for the frame this makes possible.
                    TrayAction::Show | TrayAction::Quit => {
                        if let Some(w) = window {
                            w.show();
                        }
                    }
                    // Handled entirely here, so pausing from the tray works
                    // while hidden. `update` deliberately ignores it.
                    TrayAction::TogglePlay => {
                        let running = shared.lock().unwrap().running;
                        let _ = tx.send(Cmd::SetRunning(!running));
                    }
                }
                ctx.request_repaint();
            })
        };

        Self {
            engine,
            params: Params::default(),
            selected: usize::MAX, // forces a sync on the first frame
            effects_dir,
            anim_dir,
            tab: Tab::Play,
            editor: None,
            last_tick: std::time::Instant::now(),
            notice: None,

            settings: Settings::load(),
            tray,
            window,
            confirm_close: false,
            remember_choice: false,
            hidden: false,
            quitting: false,
            resume_on_show: false,
        }
    }

    /// Bring the window back from the tray.
    fn restore(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if self.hidden && self.resume_on_show {
            self.engine.send(Cmd::SetRunning(true));
            self.resume_on_show = false;
        }
        self.hidden = false;
        self.confirm_close = false;
    }

    /// Hide the window, leaving the app alive in the tray.
    ///
    /// Refuses unless there is both a tray to click and a way to act on that
    /// click while hidden — otherwise the window would be unreachable.
    fn hide_to_tray(&mut self, ctx: &egui::Context, running: bool) -> bool {
        if !self.can_hide() {
            return false;
        }
        if !self.settings.run_in_background && running {
            self.engine.send(Cmd::SetRunning(false));
            self.resume_on_show = true;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.hidden = true;
        self.confirm_close = false;
        true
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.confirm_close = false;
        // Unhide first: a hidden window that is closing never processes the
        // close on some platforms, leaving the process running invisibly.
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Whether the window can be hidden and got back again. Both halves are
    /// required: the tray to ask with, and a way to show the window from the
    /// thread that hears the asking.
    fn can_hide(&self) -> bool {
        self.tray.is_some() && self.window.is_some()
    }

    /// Tray clicks, minimise-to-tray, and the close button.
    fn window_lifecycle(&mut self, ctx: &egui::Context, running: bool, status: &str) {
        if let Some(tray) = self.tray.as_mut() {
            tray.sync(running, status);
        }
        for action in self.tray.as_ref().map(Tray::drain).unwrap_or_default() {
            match action {
                TrayAction::Show => self.restore(ctx),
                // Already applied on the tray thread, so it works while the
                // window is hidden. Re-sending here would undo it.
                TrayAction::TogglePlay => {}
                TrayAction::Quit => self.quit(ctx),
            }
        }

        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        if minimized && !self.hidden && self.settings.minimize_to_tray {
            self.hide_to_tray(ctx, running);
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            // Without a tray, close can only mean quit.
            let action = if self.can_hide() {
                self.settings.close_action
            } else {
                CloseAction::Quit
            };
            match action {
                CloseAction::Quit => self.quitting = true,
                CloseAction::Tray => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.hide_to_tray(ctx, running);
                }
                CloseAction::Ask => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.confirm_close = true;
                }
            }
        }
    }

    /// "Minimise to tray or quit?", shown when the close button is pressed and
    /// the user has not already told us which they mean.
    fn close_dialog(&mut self, ctx: &egui::Context, running: bool) {
        if !self.confirm_close {
            return;
        }
        let mut choice: Option<CloseAction> = None;

        egui::Window::new("Close keylux?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("keylux can keep running in the notification area, or quit.");
                ui.add_space(2.0);
                ui.weak(
                    "Quitting stops the lighting; the keyboard keeps the last frame it was sent.",
                );
                ui.add_space(10.0);
                ui.checkbox(&mut self.remember_choice, "Remember my choice");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Minimise to tray").clicked() {
                        choice = Some(CloseAction::Tray);
                    }
                    if ui.button("Quit").clicked() {
                        choice = Some(CloseAction::Quit);
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_close = false;
                        self.remember_choice = false;
                    }
                });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.confirm_close = false;
            self.remember_choice = false;
        }

        let Some(choice) = choice else { return };
        if self.remember_choice {
            self.settings.close_action = choice;
            self.settings.save();
            self.remember_choice = false;
        }
        match choice {
            CloseAction::Tray => {
                self.hide_to_tray(ctx, running);
            }
            CloseAction::Quit => self.quit(ctx),
            CloseAction::Ask => {}
        }
    }

    /// Window behaviour, tucked at the bottom of the effects panel so a user
    /// who regrets a "remember my choice" can undo it without editing JSON.
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Window", |ui| {
            if !self.can_hide() {
                ui.weak("No system tray on this platform, so closing quits.");
                return;
            }
            let mut dirty = false;
            dirty |= ui
                .checkbox(&mut self.settings.minimize_to_tray, "Minimise to tray")
                .changed();
            dirty |= ui
                .checkbox(
                    &mut self.settings.run_in_background,
                    "Keep lighting running when hidden",
                )
                .changed();

            ui.horizontal(|ui| {
                ui.label("Close button");
                let text = match self.settings.close_action {
                    CloseAction::Ask => "Ask",
                    CloseAction::Tray => "Minimise to tray",
                    CloseAction::Quit => "Quit",
                };
                egui::ComboBox::from_id_salt("close_action")
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        for (v, label) in [
                            (CloseAction::Ask, "Ask"),
                            (CloseAction::Tray, "Minimise to tray"),
                            (CloseAction::Quit, "Quit"),
                        ] {
                            dirty |= ui
                                .selectable_value(&mut self.settings.close_action, v, label)
                                .changed();
                        }
                    });
            });

            if dirty {
                self.settings.save();
            }
        });
    }

    /// Import a GIF, image or folder of frames as an animation.
    fn import_dialog(&mut self, layout: &[aula_protocol::KeyPos], leds: usize) -> Option<String> {
        let file = rfd::FileDialog::new()
            .add_filter(
                "Animations & art",
                &["gif", "png", "jpg", "jpeg", "bmp", "webp", "json", "rhai"],
            )
            .set_title("Import an effect, animation or image")
            .pick_file()?;

        let ext = file
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        // Scripts and existing animations are just copied into place.
        if ext == "rhai" || ext == "json" {
            let dest_dir = if ext == "rhai" {
                &self.effects_dir
            } else {
                &self.anim_dir
            };
            let dest = dest_dir.join(file.file_name()?);
            return match std::fs::copy(&file, &dest) {
                Ok(_) => {
                    self.engine.send(Cmd::Rescan);
                    Some(format!("Imported {}", dest.display()))
                }
                Err(e) => Some(format!("Import failed: {e}")),
            };
        }

        // Artwork is converted into a keyframe animation.
        match aula_effects::import::import_any(&file, layout, leds) {
            Ok(anim) => {
                let stem = file.file_stem()?.to_string_lossy().to_lowercase();
                let dest = self.anim_dir.join(format!("{stem}.json"));
                match anim.save(&dest) {
                    Ok(()) => {
                        self.engine.send(Cmd::Rescan);
                        Some(format!(
                            "Imported {} keyframes to {}",
                            anim.keyframes.len(),
                            dest.display()
                        ))
                    }
                    Err(e) => Some(format!("Could not save: {e}")),
                }
            }
            Err(e) => Some(format!("Import failed: {e}")),
        }
    }
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Snapshot the engine state, then release the lock before drawing.
        let (
            frame,
            layout,
            fps,
            status,
            effects,
            engine_sel,
            errors,
            running,
            frames_sent,
            script_error,
        ) = {
            let s = self.engine.shared.lock().unwrap();
            (
                s.frame.clone(),
                s.layout.clone(),
                s.fps,
                s.status.clone(),
                s.effects.clone(),
                s.selected,
                s.errors.clone(),
                s.running,
                s.frames_sent,
                s.script_error.clone(),
            )
        };

        let status_line = match &status {
            DeviceStatus::Connected { name, .. } => name.clone(),
            DeviceStatus::Disconnected(_) => "not connected".to_string(),
        };
        self.window_lifecycle(ctx, running, &status_line);
        self.close_dialog(ctx, running);

        // First frame, or the engine changed selection (e.g. after a rescan).
        if self.selected == usize::MAX || self.selected != engine_sel {
            self.selected = engine_sel.min(effects.len().saturating_sub(1));
            if let Some(e) = effects.get(self.selected) {
                self.params = Params::from_specs(&e.meta.params);
            }
        }

        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                match &status {
                    DeviceStatus::Connected { name, .. } => {
                        ui.colored_label(egui::Color32::from_rgb(80, 200, 120), "●");
                        ui.label(egui::RichText::new(name).strong());
                        ui.label(format!("{fps:.0} FPS"));
                        ui.weak(format!("{frames_sent} frames"));
                    }
                    DeviceStatus::Disconnected(why) => {
                        ui.colored_label(egui::Color32::from_rgb(220, 90, 90), "●");
                        ui.label("Not connected");
                        ui.weak(short(why, 90));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if running { "⏸ Pause" } else { "▶ Play" };
                    if ui.button(label).clicked() {
                        self.engine.send(Cmd::SetRunning(!running));
                    }
                });
            });
        });

        egui::SidePanel::left("effects")
            .resizable(true)
            .default_width(230.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Effects");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("⟳")
                            .on_hover_text("Rescan effects folder")
                            .clicked()
                        {
                            self.engine.send(Cmd::Rescan);
                        }
                    });
                });
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, e) in effects.iter().enumerate() {
                        let label = match (e.is_script, e.is_animation) {
                            (_, true) => format!("🎞 {}", e.meta.name),
                            (true, _) => format!("📜 {}", e.meta.name),
                            _ => format!("⚙ {}", e.meta.name),
                        };
                        let resp = ui.selectable_label(i == self.selected, label);
                        if !e.meta.description.is_empty() {
                            resp.clone().on_hover_text(&e.meta.description);
                        }
                        if resp.clicked() && i != self.selected {
                            self.selected = i;
                            self.params = Params::from_specs(&e.meta.params);
                            self.engine.send(Cmd::SelectEffect(i));
                            self.engine.send(Cmd::SetParams(self.params.clone()));
                        }
                    }
                });

                ui.separator();
                ui.weak("⚙ built-in   📜 script   🎞 animation");

                ui.add_space(6.0);
                if ui
                    .button("📥 Import…")
                    .on_hover_text("GIF, PNG, JPG, a .rhai script or a .json animation")
                    .clicked()
                {
                    let msg = self.import_dialog(&layout, frame.len());
                    if let Some(m) = msg {
                        self.notice = Some(m);
                    }
                }
                ui.horizontal(|ui| {
                    if ui.small_button("📂 Effects").clicked() {
                        open_folder(&self.effects_dir);
                    }
                    if ui.small_button("📂 Animations").clicked() {
                        open_folder(&self.anim_dir);
                    }
                });
                if let Some(n) = &self.notice {
                    ui.add_space(4.0);
                    ui.weak(short(n, 120));
                }

                ui.add_space(8.0);
                ui.separator();
                self.settings_ui(ui);
            });

        let has_errors = !errors.is_empty() || script_error.is_some();
        egui::TopBottomPanel::bottom("errors").show_animated(ctx, has_errors, |ui| {
            // A script that throws keeps its last good frame on the board, so
            // without this strip a broken effect looks like a frozen app.
            if let Some(e) = &script_error {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(230, 110, 90), "✖");
                    ui.label(egui::RichText::new(short(e, 200)).small());
                });
            }
            if !errors.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(230, 160, 60), "⚠");
                    for e in errors.iter().take(4) {
                        ui.label(egui::RichText::new(short(e, 160)).small());
                    }
                });
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Play, "▶ Play");
                if ui
                    .selectable_label(self.tab == Tab::Create, "✏ Create")
                    .clicked()
                {
                    self.tab = Tab::Create;
                    if self.editor.is_none() {
                        self.editor = Some(Editor::new(frame.len().max(1)));
                    }
                }
            });
            ui.separator();

            if self.tab == Tab::Create {
                let dt = self.last_tick.elapsed().as_secs_f32();
                self.last_tick = std::time::Instant::now();
                let anim_dir = self.anim_dir.clone();
                if let Some(ed) = self.editor.as_mut() {
                    ed.tick(dt);
                    editor::show(ui, ed, &layout, &anim_dir);
                }
                return;
            }
            self.last_tick = std::time::Instant::now();

            // ---- live preview ----
            let desired = egui::vec2(ui.available_width(), 190.0);
            let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
            draw_keyboard(ui, rect, &layout, &frame);

            ui.add_space(8.0);
            ui.separator();

            // ---- parameters, generated from the effect's declaration ----
            let Some(effect) = effects.get(self.selected) else {
                ui.weak("No effects found.");
                return;
            };

            ui.horizontal(|ui| {
                ui.heading(&effect.meta.name);
                // Imported artwork and saved timelines are both plain keyframe
                // JSON, so anything the registry lists as an animation can be
                // reopened and reworked rather than only replaced.
                if effect.is_animation {
                    if let Some(file) = effect.file.clone() {
                        if ui
                            .button("✏ Edit")
                            .on_hover_text("Open this animation in the timeline editor")
                            .clicked()
                        {
                            let path = self.anim_dir.join(&file);
                            match aula_effects::Animation::load(&path) {
                                Ok(anim) => {
                                    let stem = std::path::Path::new(&file)
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| file.clone());
                                    self.editor = Some(Editor::from_animation(anim, stem));
                                    self.tab = Tab::Create;
                                }
                                Err(e) => self.notice = Some(format!("Could not open: {e}")),
                            }
                        }
                    }
                }
            });
            if let Some(f) = &effect.file {
                let kind = if effect.is_animation {
                    "animation"
                } else {
                    "script"
                };
                ui.weak(format!("{kind}: {f}"));
            }
            ui.add_space(4.0);

            let mut changed = false;
            egui::Grid::new("params")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    for spec in &effect.meta.params {
                        ui.label(&spec.label);
                        match &spec.kind {
                            ParamKind::Float { min, max, default } => {
                                let mut v = self.params.float(&spec.id, *default);
                                if ui.add(egui::Slider::new(&mut v, *min..=*max)).changed() {
                                    self.params.set(&spec.id, Value::Float(v));
                                    changed = true;
                                }
                            }
                            ParamKind::Int { min, max, default } => {
                                let mut v = self.params.int(&spec.id, *default);
                                if ui.add(egui::Slider::new(&mut v, *min..=*max)).changed() {
                                    self.params.set(&spec.id, Value::Int(v));
                                    changed = true;
                                }
                            }
                            ParamKind::Bool { default } => {
                                let mut v = self.params.bool(&spec.id, *default);
                                if ui.checkbox(&mut v, "").changed() {
                                    self.params.set(&spec.id, Value::Bool(v));
                                    changed = true;
                                }
                            }
                            ParamKind::Color { default } => {
                                let c = self.params.color(&spec.id, *default);
                                let mut rgb = [c.r, c.g, c.b];
                                if ui.color_edit_button_srgb(&mut rgb).changed() {
                                    self.params.set(
                                        &spec.id,
                                        Value::Color(Rgb::new(rgb[0], rgb[1], rgb[2])),
                                    );
                                    changed = true;
                                }
                            }
                            ParamKind::Text { default } => {
                                let mut v = self.params.text(&spec.id, default);
                                if ui.text_edit_singleline(&mut v).changed() {
                                    self.params.set(&spec.id, Value::Text(v));
                                    changed = true;
                                }
                            }
                            ParamKind::Choice { options, default } => {
                                let cur = self.params.int(&spec.id, *default as i64) as usize;
                                let text = options.get(cur).cloned().unwrap_or_default();
                                egui::ComboBox::from_id_salt(&spec.id)
                                    .selected_text(text)
                                    .show_ui(ui, |ui| {
                                        for (i, opt) in options.iter().enumerate() {
                                            if ui.selectable_label(i == cur, opt).clicked() {
                                                self.params.set(&spec.id, Value::Int(i as i64));
                                                changed = true;
                                            }
                                        }
                                    });
                            }
                        }
                        ui.end_row();
                    }
                });

            if effect.meta.params.is_empty() {
                ui.weak("This effect has no parameters.");
            }

            if changed {
                self.engine.send(Cmd::SetParams(self.params.clone()));
            }
        });
    }
}

/// Draw the board from real layout geometry: rounded rects at each key's
/// physical position, filled with that key's colour in the current frame.
///
/// This doubles as an LED-map check — a wrong map shows up immediately as an
/// effect that looks scrambled here but fine on the keyboard, or vice versa.
fn draw_keyboard(
    ui: &egui::Ui,
    rect: egui::Rect,
    layout: &[aula_protocol::KeyPos],
    frame: &aula_protocol::Frame,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, egui::Color32::from_gray(18));

    if layout.is_empty() {
        return;
    }

    let cols = layout.iter().map(|k| k.x + k.w / 2.0).fold(0.0, f32::max);
    let rows = layout.iter().map(|k| k.row).max().unwrap_or(0) as f32 + 1.0;

    let pad = 10.0;
    let unit = ((rect.width() - pad * 2.0) / cols).min((rect.height() - pad * 2.0) / rows);
    let origin = egui::pos2(
        rect.left() + pad + ((rect.width() - pad * 2.0) - unit * cols) / 2.0,
        rect.top() + pad + ((rect.height() - pad * 2.0) - unit * rows) / 2.0,
    );

    for k in layout {
        let c = frame.get(k.led);
        let w = unit * k.w - 2.0;
        let h = unit - 2.0;
        let pos = egui::pos2(
            origin.x + (k.x - k.w / 2.0) * unit + 1.0,
            origin.y + f32::from(k.row) * unit + 1.0,
        );
        let key_rect = egui::Rect::from_min_size(pos, egui::vec2(w.max(1.0), h.max(1.0)));

        // Unlit keys stay visible as dark outlines so the board reads as a
        // keyboard even when an effect is mostly black.
        let fill = if c.is_black() {
            egui::Color32::from_gray(34)
        } else {
            egui::Color32::from_rgb(c.r, c.g, c.b)
        };
        painter.rect_filled(key_rect, 2.0, fill);
    }
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
