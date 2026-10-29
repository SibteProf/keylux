//! egui front end.

use eframe::egui;

use aula_effects::{ParamKind, ParamSpec, Params, Value};
use aula_protocol::{Link, Rgb};
use strum::IntoEnumIterator;

use crate::board;
use crate::editor::{self, Editor};
use crate::engine::{Cmd, DeviceStatus, Engine};
use crate::profiles::{self, AppProfile, LightingPreset};
use crate::settings::{CloseAction, ColorTheme, Settings};
use crate::startup;
use crate::theme;
use crate::tray::{Tray, TrayAction};
use crate::window_ctl::WindowRef;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Play,
    Create,
    Apps,
    Settings,
}

pub struct App {
    engine: Engine,
    /// The UI owns the parameter values and pushes the whole set on change, so
    /// UI and engine can never drift apart.
    params: Params,
    selected: usize,
    selection_revision: u64,
    effects_dir: std::path::PathBuf,
    anim_dir: std::path::PathBuf,
    tab: Tab,
    editor: Option<Editor>,
    last_tick: std::time::Instant,
    notice: Option<String>,
    effect_query: String,
    sidebar_visible: bool,
    startup_enabled: bool,
    startup_error: Option<String>,
    /// True while the editor is streaming its preview to the keyboard, so we
    /// know to hand control back to the effect engine when that stops.
    live_active: bool,
    last_device_target: Option<String>,

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
        let settings = Settings::load();
        theme::install_fonts(&cc.egui_ctx);
        cc.egui_ctx
            .set_zoom_factor(settings.ui_scale.clamp(0.8, 2.0));
        theme::set_current(settings.color_theme);
        theme::current().apply(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let engine = Engine::spawn(
            effects_dir.clone(),
            settings.target(),
            settings.allow_list(),
            settings.endpoint(),
            settings.app_lighting.clone(),
            move || ctx.request_repaint(),
        );
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

        engine.send(Cmd::SetMaxFps(settings.max_fps));
        let (startup_enabled, startup_error) = match startup::enabled() {
            Ok(enabled) => (enabled, None),
            Err(error) => (false, Some(error)),
        };

        Self {
            engine,
            params: Params::default(),
            selected: usize::MAX, // forces a sync on the first frame
            selection_revision: u64::MAX,
            effects_dir,
            anim_dir,
            tab: Tab::Play,
            editor: None,
            last_tick: std::time::Instant::now(),
            notice: None,
            effect_query: String::new(),
            sidebar_visible: true,
            startup_enabled,
            startup_error,
            live_active: false,
            last_device_target: None,

            settings,
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
        // The editor stops drawing while hidden, so hand the board back to the
        // effect engine rather than leaving a stale preview frame on it.
        if self.live_active {
            self.engine.send(Cmd::LivePreview(None));
            self.live_active = false;
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
                theme::muted(
                    ui,
                    "Quitting stops the lighting; the keyboard keeps the last frame it was sent.",
                );
                ui.add_space(10.0);
                ui.checkbox(&mut self.remember_choice, "Remember my choice");
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
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

    /// How hard to drive the keyboard, and which keyboard to drive.
    ///
    /// The board's 8051 scans the key matrix and services USB on the same
    /// budget, so lighting traffic competes with typing. The default already
    /// matches what the vendor driver does, but a busy machine or a marginal
    /// cable can still push it over — hence a lever rather than a fixed number.
    fn device_settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Keyboard")
            .default_open(true)
            .show(ui, |ui| {
                self.device_picker_ui(ui);
                ui.separator();

                // The ceiling is the connected device's, not the protocol's: the
                // wireless link is paced more slowly, and offering a rate the
                // device will silently clamp reads as a bug.
                let ceiling = self.engine.shared.lock().unwrap().max_fps_ceiling.max(1);
                let mut fps = self.settings.max_fps.clamp(1, ceiling);
                let resp = ui
                    .add(egui::Slider::new(&mut fps, 1..=ceiling).text("Frame rate"))
                    .on_hover_text(
                        "Lower this if the keyboard misses keypresses or repeats them.\n\
                     Lighting traffic and key scanning share the same processor.",
                    );
                if resp.changed() {
                    self.settings.max_fps = fps;
                    self.engine.send(Cmd::SetMaxFps(fps));
                }
                // Only persist once the drag ends, so a sweep across the slider
                // does not write the file a hundred times.
                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                    self.settings.save();
                }
                theme::muted(ui, "Unchanged colors skip repeated lighting writes.");
            });
    }

    /// Pick which board to drive, and find receivers that are not built in.
    ///
    /// A 2.4 GHz receiver is a separate USB device with its own id, and that id
    /// differs between production runs — so it cannot be shipped in the source
    /// table and has to be found here once and remembered.
    fn device_picker_ui(&mut self, ui: &mut egui::Ui) {
        let (devices, pinned, scanning) = {
            let s = self.engine.shared.lock().unwrap();
            (s.devices.clone(), s.pinned.clone(), s.scanning)
        };

        let current = match &pinned {
            None => "Automatic (wired preferred)".to_string(),
            Some(id) => devices
                .iter()
                .find(|d| &d.id == id)
                .map(|d| d.label.clone())
                .unwrap_or_else(|| id.to_string()),
        };

        ui.horizontal_wrapped(|ui| {
            ui.label("Device");
            egui::ComboBox::from_id_salt("device-picker")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(pinned.is_none(), "Automatic (wired preferred)")
                        .clicked()
                    {
                        self.engine.send(Cmd::SelectDevice(None));
                        self.settings.unpin_device();
                        self.settings.save();
                    }
                    for d in &devices {
                        let mut label = d.label.clone();
                        if !d.present {
                            label.push_str("  (not plugged in)");
                        }
                        if ui
                            .selectable_label(pinned.as_ref() == Some(&d.id), label)
                            .clicked()
                        {
                            self.engine.send(Cmd::SelectDevice(Some(d.id.clone())));
                            self.settings.remember_target(d.id.clone());
                            self.settings.save();
                        }
                    }
                });
            if scanning {
                ui.spinner();
            }
        });

        ui.horizontal_wrapped(|ui| {
            if ui.button("Rescan keyboards").clicked() {
                self.engine.send(Cmd::ScanDevices { deep: false });
            }
            if ui
                .button("Find wireless receivers")
                .on_hover_text(
                    "Briefly opens each vendor HID collection read-only to see which one \
                     answers.\nTo find yours: unplug the receiver, scan, plug it back in and \
                     scan again. The entry that comes and goes is it.",
                )
                .clicked()
            {
                self.engine.send(Cmd::ScanDevices { deep: true });
            }
        });

        ui.add_space(8.0);
        egui::CollapsingHeader::new("OpenRGB connection").default_open(false).show(ui, |ui| {

        if ui
            .checkbox(
                &mut self.settings.openrgb_enabled,
                "Enable OpenRGB keyboards",
            )
            .changed()
        {
            self.engine
                .send(Cmd::ConfigureOpenRgb(self.settings.endpoint()));
            self.settings.save();
        }
        if self.settings.openrgb_enabled {
            ui.horizontal_wrapped(|ui| {
                ui.label("SDK address");
                ui.text_edit_singleline(&mut self.settings.openrgb_endpoint);
                if ui.button("Apply").clicked() {
                    self.engine
                        .send(Cmd::ConfigureOpenRgb(self.settings.endpoint()));
                    self.settings.save();
                }
            });
            theme::muted(ui, "Start the SDK server in OpenRGB. Select a keyboard above; keylux supplies the effects.");
        }
        let warnings = self
            .engine
            .shared
            .lock()
            .unwrap()
            .discovery_warnings
            .clone();
        for warning in warnings {
            ui.colored_label(theme::current().warning, warning);
        }


        });

        // The side switch on the keyboard, not this picker, decides which link
        // is actually driving the LEDs. Without this note, pinning the dongle
        // while the switch is on wired looks like the app being broken.
        theme::muted(
            ui,
            "The keyboard's own wired/2.4 GHz switch decides which link is live. \
             Set it to match.",
        );

        if let DeviceStatus::Connected {
            can_change_mode: false,
            ..
        } = self.engine.shared.lock().unwrap().status
        {
            ui.colored_label(
                theme::current().warning,
                "Streaming only: nothing has confirmed this device, so lighting-mode \
                 changes are refused.",
            );
        }
    }

    /// The Settings tab: device/frame-rate controls and window behaviour, so a
    /// user who regrets a "remember my choice" can undo it without editing JSON.
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        theme::page_heading(
            ui,
            "Settings",
            "Make keylux work with your keyboard and your desktop.",
        );
        theme::surface(ui, |ui| {
            theme::section(ui, "Appearance", "Choose a comfortable view.");
            ui.horizontal_wrapped(|ui| {
                ui.label("Theme");
                let previous = self.settings.color_theme;
                for value in ColorTheme::iter() {
                    ui.selectable_value(&mut self.settings.color_theme, value, value.to_string());
                }
                if previous != self.settings.color_theme {
                    theme::set_current(self.settings.color_theme);
                    theme::current().apply(ui.ctx());
                    self.settings.save();
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Interface size");
                let response = ui.add(
                    egui::Slider::new(&mut self.settings.ui_scale, 0.8..=2.0)
                        .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
                );
                if response.drag_stopped() || (response.changed() && !response.dragged()) {
                    ui.ctx().set_zoom_factor(self.settings.ui_scale);
                    self.settings.save();
                }
                if ui.button("Reset size").clicked() {
                    self.settings.ui_scale = 1.0;
                    ui.ctx().set_zoom_factor(1.0);
                    self.settings.save();
                }
            });
        });
        ui.add_space(16.0);
        theme::surface(ui, |ui| {
            theme::section(
                ui,
                "Startup",
                "Launch keylux when you sign in to your desktop.",
            );
            let mut enabled = self.startup_enabled;
            if ui
                .add_enabled(
                    startup::supported(),
                    egui::Checkbox::new(&mut enabled, "Run at startup"),
                )
                .changed()
            {
                match startup::set_enabled(enabled) {
                    Ok(()) => {
                        self.startup_enabled = enabled;
                        self.startup_error = None;
                    }
                    Err(error) => {
                        self.startup_error = Some(format!("Could not change startup: {error}"))
                    }
                }
            }
            if !startup::supported() {
                theme::muted(
                    ui,
                    "Startup registration is available on Windows and Linux.",
                );
            }
            if let Some(error) = &self.startup_error {
                ui.colored_label(theme::current().danger, error);
            }
        });
        ui.add_space(16.0);
        theme::surface(ui, |ui| {
            self.device_settings_ui(ui);
        });
        ui.add_space(16.0);
        theme::surface(ui, |ui| {
            egui::CollapsingHeader::new("Window")
                .default_open(true)
                .show(ui, |ui| {
                    if !self.can_hide() {
                        theme::muted(ui, "No system tray on this platform, so closing quits.");
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

                    ui.horizontal_wrapped(|ui| {
                        ui.label("When closing the window");
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
        });
    }

    fn app_profiles_ui(&mut self, ui: &mut egui::Ui) {
        theme::page_heading(
            ui,
            "Apps",
            "Switch lighting when you focus a game, editor, or another application.",
        );
        if !profiles::supported() {
            ui.colored_label(
                theme::current().warning,
                "Automatic detection needs Windows or a Linux X11 session. Wayland and macOS are not supported.",
            );
        }
        let (effects, status) = {
            let state = self.engine.shared.lock().unwrap();
            (state.effects.clone(), state.profile_status.clone())
        };
        let current = effects
            .get(self.selected)
            .map(|effect| LightingPreset::capture(effect.meta.id.clone(), &self.params));
        let mut dirty = ui
            .add_enabled(
                profiles::supported(),
                egui::Checkbox::new(
                    &mut self.settings.app_lighting.enabled,
                    "Switch automatically",
                ),
            )
            .changed();
        if self.settings.app_lighting.enabled {
            if let Some(app) = &status.application {
                theme::muted(ui, format!("Last active application: {app}"));
            }
            ui.label(format!(
                "Lighting: {}",
                status.active.as_deref().unwrap_or("Default")
            ));
            if let Some(warning) = status.warning {
                ui.colored_label(theme::current().warning, warning);
            }
        }
        ui.add_space(8.0);

        theme::surface(ui, |ui| {
            egui::CollapsingHeader::new("Default lighting")
                .default_open(true)
                .show(ui, |ui| {
                    theme::muted(
                        ui,
                        "Used when no application matches, or when automatic switching is off.",
                    );
                    ui.push_id("default-preset", |ui| {
                        dirty |= preset_ui(ui, &mut self.settings.app_lighting.default, &effects);
                    });
                    if ui
                        .add_enabled(
                            current.is_some(),
                            egui::Button::new("Use current lighting as default"),
                        )
                        .clicked()
                    {
                        self.settings.app_lighting.default = current.clone().unwrap();
                        dirty = true;
                    }
                });
        });
        ui.add_space(18.0);
        theme::section(
            ui,
            "Profiles",
            "Profiles are checked from top to bottom. The first match wins.",
        );
        if self.settings.app_lighting.profiles.is_empty() {
            theme::surface(ui, |ui| {
                ui.label("No application profiles yet");
                theme::muted(
                    ui,
                    "Add a profile, choose an application, and assign its lighting.",
                );
            });
            ui.add_space(12.0);
        }
        let mut remove = None;
        let mut move_profile = None;
        let count = self.settings.app_lighting.profiles.len();
        for (index, profile) in self.settings.app_lighting.profiles.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                theme::surface(ui, |ui| {
                    egui::CollapsingHeader::new(format!("{}. {}", index + 1, profile.name))
                        .id_salt("profile")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                dirty |= ui.checkbox(&mut profile.enabled, "Enabled").changed();
                                if ui
                                    .add_enabled(index > 0, egui::Button::new("Move up"))
                                    .clicked()
                                {
                                    move_profile = Some((index, index - 1));
                                }
                                if ui
                                    .add_enabled(index + 1 < count, egui::Button::new("Move down"))
                                    .clicked()
                                {
                                    move_profile = Some((index, index + 1));
                                }
                                if ui.button("Delete").clicked() {
                                    remove = Some(index);
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Name");
                                dirty |= ui.text_edit_singleline(&mut profile.name).changed();
                            });
                            ui.label(if cfg!(target_os = "linux") {
                                "X11 application classes (WM_CLASS), separated by commas"
                            } else {
                                "Executable names or full paths, separated by commas"
                            });
                            dirty |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut profile.applications)
                                        .hint_text(if cfg!(target_os = "linux") {
                                            "Code, Firefox"
                                        } else {
                                            "Code.exe, cursor.exe"
                                        })
                                        .desired_width(f32::INFINITY),
                                )
                                .changed();
                            if profile
                                .applications
                                .split(',')
                                .all(|app| app.trim().is_empty())
                            {
                                ui.colored_label(
                                    theme::current().warning,
                                    "Add an application to activate this profile.",
                                );
                            }
                            if cfg!(target_os = "windows")
                                && ui.button("Choose application…").clicked()
                            {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Application", &["exe"])
                                    .set_title("Choose an application's executable")
                                    .pick_file()
                                {
                                    let path = path.to_string_lossy();
                                    if !profile.applications.trim().is_empty() {
                                        profile.applications.push_str(", ");
                                    }
                                    profile.applications.push_str(&path);
                                    dirty = true;
                                }
                            }
                            if cfg!(target_os = "linux") {
                                if let Some(app) = &status.application {
                                    if ui.button(format!("Add last active app: {app}")).clicked() {
                                        if !profile.applications.trim().is_empty() {
                                            profile.applications.push_str(", ");
                                        }
                                        profile.applications.push_str(app);
                                        dirty = true;
                                    }
                                }
                            }
                            dirty |= preset_ui(ui, &mut profile.lighting, &effects);
                            if ui
                                .add_enabled(
                                    current.is_some(),
                                    egui::Button::new("Use current lighting for this profile"),
                                )
                                .clicked()
                            {
                                profile.lighting = current.clone().unwrap();
                                dirty = true;
                            }
                        });
                });
            });
            ui.add_space(12.0);
        }
        if let Some(index) = remove {
            self.settings.app_lighting.profiles.remove(index);
            dirty = true;
        } else if let Some((from, to)) = move_profile {
            self.settings.app_lighting.profiles.swap(from, to);
            dirty = true;
        }
        if ui.button("Add application profile").clicked() {
            self.settings.app_lighting.profiles.push(AppProfile {
                name: format!("Profile {}", count + 1),
                lighting: current.unwrap_or_default(),
                ..Default::default()
            });
            dirty = true;
        }
        if dirty {
            self.settings.save();
            self.engine.send(Cmd::ConfigureAppLighting(
                self.settings.app_lighting.clone(),
            ));
        }
    }

    /// Import a GIF, image or folder of frames as an animation.
    fn import_dialog(&mut self, layout: &[aula_protocol::KeyPos], leds: usize) -> Option<String> {
        let file = rfd::FileDialog::new()
            .add_filter(
                "Effects & art",
                &[
                    "gif", "png", "jpg", "jpeg", "bmp", "webp", "json", "klx", "rhai",
                ],
            )
            .set_title("Import an effect, animation or image")
            .pick_file()?;

        let ext = file
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        // Scripts, animations and compositions are just copied into place.
        if ext == "rhai" || ext == "json" || ext == "klx" {
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
            link,
            engine_params,
            selection_revision,
            profile_status,
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
                s.link,
                s.params.clone(),
                s.selection_revision,
                s.profile_status.clone(),
            )
        };
        let wireless = link == Some(Link::Dongle);
        let target = match &status {
            DeviceStatus::Connected { id, .. } => Some(id.clone()),
            _ => None,
        };
        if target != self.last_device_target {
            if let Some(ed) = self.editor.as_mut() {
                ed.live = false;
            }
            self.engine.send(Cmd::LivePreview(None));
            self.live_active = false;
            self.last_device_target = target;
        }
        let can_save = matches!(&status, DeviceStatus::Connected { can_save: true, .. });

        let status_line = match &status {
            DeviceStatus::Connected { name, .. } => name.clone(),
            DeviceStatus::Waiting { label, .. } => format!("waiting for {label}"),
            DeviceStatus::Disconnected(_) => "not connected".to_string(),
        };
        self.window_lifecycle(ctx, running, &status_line);
        self.close_dialog(ctx, running);

        // First frame, or the engine changed selection (e.g. after a rescan).
        if self.selected == usize::MAX || self.selection_revision != selection_revision {
            self.selected = engine_sel.min(effects.len().saturating_sub(1));
            self.params = engine_params;
            self.selection_revision = selection_revision;
        }

        let pal = theme::current();
        // Stable navigation and transport; diagnostics belong in the device menu.
        let shortcut = ctx.input_mut(|input| {
            for (key, tab) in [
                (egui::Key::Num1, Tab::Play),
                (egui::Key::Num2, Tab::Create),
                (egui::Key::Num3, Tab::Apps),
                (egui::Key::Num4, Tab::Settings),
            ] {
                if input.consume_key(egui::Modifiers::COMMAND, key) {
                    return Some(tab);
                }
            }
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma) {
                return Some(Tab::Settings);
            }
            None
        });
        if let Some(tab) = shortcut {
            self.tab = tab;
        }
        if self.tab == Tab::Create && self.editor.is_none() {
            self.editor = Some(Editor::new(frame.len().max(1)));
        }
        let compact_header = ctx.screen_rect().width() < 620.0;
        let show_sidebar = self.sidebar_visible && ctx.screen_rect().width() >= 720.0;
        egui::TopBottomPanel::top("workspace-toolbar")
            .frame(
                egui::Frame::none()
                    .fill(pal.surface)
                    .inner_margin(egui::Margin::symmetric(18.0, 12.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.menu_button(egui::RichText::new("keylux").size(20.0).strong(), |ui| {
                        for (tab, label) in [
                            (Tab::Play, "Play"),
                            (Tab::Create, "Create"),
                            (Tab::Apps, "Apps"),
                            (Tab::Settings, "Settings"),
                        ] {
                            if ui.selectable_label(self.tab == tab, label).clicked() {
                                self.tab = tab;
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui
                            .checkbox(&mut self.sidebar_visible, "Show sidebar")
                            .changed()
                        {
                            ui.close_menu();
                        }
                    });
                    if !compact_header {
                        ui.separator();
                        let title_limit = if ui.available_width() < 700.0 { 28 } else { 60 };
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(short(&status_line, title_limit)).strong(),
                            )
                            .on_hover_text(&status_line);
                            let (signal, label) = match &status {
                                DeviceStatus::Connected { .. } => {
                                    (pal.success, if running { "Connected" } else { "Paused" })
                                }
                                DeviceStatus::Waiting { .. } => {
                                    (pal.warning, "Waiting for keyboard")
                                }
                                DeviceStatus::Disconnected(_) => {
                                    (pal.danger, "No keyboard connected")
                                }
                            };
                            ui.horizontal_wrapped(|ui| {
                                status_dot(ui, signal);
                                ui.label(egui::RichText::new(label).small().color(pal.text_muted));
                            });
                        });
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(theme::primary_button(if compact_header {
                                if running {
                                    "Pause"
                                } else {
                                    "Resume"
                                }
                            } else if running {
                                "Pause lighting"
                            } else {
                                "Resume lighting"
                            }))
                            .clicked()
                        {
                            self.engine.send(Cmd::SetRunning(!running));
                        }
                        ui.menu_button("Device", |ui| {
                            ui.label(&status_line);
                            if let Some(link) = link {
                                theme::muted(ui, link.suffix());
                            }
                            theme::muted(ui, format!("{fps:.0} FPS · {frames_sent} frames sent"));
                            match &status {
                                DeviceStatus::Disconnected(why) => {
                                    ui.label(why);
                                }
                                DeviceStatus::Connected {
                                    has_matrix: false, ..
                                } => {
                                    theme::muted(
                                        ui,
                                        "LED grid: no physical map supplied by OpenRGB.",
                                    );
                                }
                                _ => {}
                            }
                            if ui.button("Keyboard settings").clicked() {
                                self.tab = Tab::Settings;
                                ui.close_menu();
                            }
                            if matches!(status, DeviceStatus::Waiting { .. })
                                && ui.button("Choose automatically").clicked()
                            {
                                self.engine.send(Cmd::SelectDevice(None));
                                self.settings.unpin_device();
                                self.settings.save();
                                ui.close_menu();
                            }
                        });
                    });
                });
                if compact_header {
                    theme::muted(ui, short(&status_line, 44));
                }
            });
        egui::SidePanel::left("workspace-navigation")
            .resizable(false)
            .exact_width(202.0)
            .frame(egui::Frame::none().fill(pal.surface).inner_margin(14.0))
            .show_animated(ctx, show_sidebar, |ui| {
                for (tab, label, detail) in [
                    (Tab::Play, "Play", "Choose and tune lighting"),
                    (Tab::Create, "Create", "Layers and keyframes"),
                    (Tab::Apps, "Apps", "Application profiles"),
                    (Tab::Settings, "Settings", "Keyboard and preferences"),
                ] {
                    if ui
                        .add_sized(
                            [ui.available_width(), 36.0],
                            egui::SelectableLabel::new(
                                self.tab == tab,
                                egui::RichText::new(label)
                                    .strong()
                                    .color(if self.tab == tab {
                                        pal.on_accent
                                    } else {
                                        pal.text
                                    }),
                            ),
                        )
                        .on_hover_text(detail)
                        .clicked()
                    {
                        self.tab = tab;
                    }
                }
                if self.tab == Tab::Create && self.editor.is_none() {
                    self.editor = Some(Editor::new(frame.len().max(1)));
                }
                if self.tab == Tab::Play {
                    ui.add_space(22.0);
                    let effects_label = ui.label(egui::RichText::new("Effects").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.effect_query)
                            .hint_text("Search effects")
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(effects_label.id);
                    let query = self.effect_query.trim().to_lowercase();
                    let mut matches = 0;
                    egui::ScrollArea::vertical()
                        .id_salt("effect-library")
                        .max_height((ui.available_height() - 140.0).max(80.0))
                        .show(ui, |ui| {
                            let group_of = |e: &crate::engine::EffectInfo| -> usize {
                                if e.is_composition {
                                    3
                                } else if e.is_animation {
                                    2
                                } else if e.is_script {
                                    1
                                } else {
                                    0
                                }
                            };
                            for (group, heading) in
                                ["Built-in", "Scripts", "Animations", "Compositions"]
                                    .iter()
                                    .enumerate()
                            {
                                let visible: Vec<_> = effects
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, e)| {
                                        group_of(e) == group
                                            && e.meta.name.to_lowercase().contains(&query)
                                    })
                                    .collect();
                                if visible.is_empty() {
                                    continue;
                                }
                                ui.add_space(12.0);
                                ui.label(
                                    egui::RichText::new(*heading).small().color(pal.text_muted),
                                );
                                for (index, effect) in visible {
                                    matches += 1;
                                    let blocked =
                                        wireless && (effect.is_animation || effect.is_composition);
                                    let response = ui
                                        .add_enabled_ui(!blocked, |ui| {
                                            ui.add_sized(
                                                [ui.available_width(), 32.0],
                                                egui::SelectableLabel::new(
                                                    index == self.selected,
                                                    &effect.meta.name,
                                                ),
                                            )
                                        })
                                        .inner;
                                    let response = if blocked {
                                        response.on_disabled_hover_text(
                                            "Connect by USB to play this animation.",
                                        )
                                    } else {
                                        response.on_hover_text(&effect.meta.description)
                                    };
                                    if response.clicked() && index != self.selected {
                                        self.selected = index;
                                        self.params = Params::from_specs(&effect.meta.params);
                                        self.engine.send(Cmd::SelectEffect(index));
                                    }
                                }
                            }
                            if matches == 0 {
                                theme::muted(
                                    ui,
                                    "No matching effects. Try another name or import an effect.",
                                );
                            }
                        });
                    ui.add_space(14.0);
                    if ui
                        .add_sized(
                            [ui.available_width(), 32.0],
                            egui::Button::new("Import effect…"),
                        )
                        .clicked()
                    {
                        if let Some(message) = self.import_dialog(&layout, frame.len()) {
                            self.notice = Some(message);
                        }
                    }
                    ui.menu_button("Library options", |ui| {
                        if ui.button("Refresh effects").clicked() {
                            self.engine.send(Cmd::Rescan);
                            ui.close_menu();
                        }
                        if ui.button("Open effects folder").clicked() {
                            open_folder(&self.effects_dir);
                            ui.close_menu();
                        }
                        if ui.button("Open animations folder").clicked() {
                            open_folder(&self.anim_dir);
                            ui.close_menu();
                        }
                    });
                } else {
                    ui.add_space(22.0);
                    theme::muted(
                        ui,
                        match self.tab {
                            Tab::Create => {
                                "Build lighting with static keys, motion, and painted animation."
                            }
                            Tab::Apps => "Match your lighting to the application you're using.",
                            _ => "Device connections, appearance, and window behaviour.",
                        },
                    );
                }
            });

        if self.tab == Tab::Create && self.editor.is_none() {
            self.editor = Some(Editor::new(frame.len().max(1)));
        }
        let has_errors = !errors.is_empty() || script_error.is_some();
        egui::TopBottomPanel::top("errors").show_animated(ctx, has_errors, |ui| {
            // A script that throws keeps its last good frame on the board, so
            // without this strip a broken effect looks like a frozen app.
            if let Some(e) = &script_error {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(pal.danger, "Effect error");
                    ui.label(egui::RichText::new(short(e, 200)).small());
                });
            }
            if !errors.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(pal.warning, "Warning");
                    for e in errors.iter().take(4) {
                        ui.label(egui::RichText::new(short(e, 160)).small());
                    }
                });
            }
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.bg).inner_margin(22.0))
            .show(ctx, |ui| {
            if let Some(message) = self.notice.clone() {
                theme::surface(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(short(&message, 180));
                        if ui.button("Dismiss").clicked() { self.notice = None; }
                    });
                });
                ui.add_space(12.0);
            }
            if self.settings.app_lighting.enabled && self.tab == Tab::Play {
                theme::muted(ui, format!("Application lighting: {}", profile_status.active.as_deref().unwrap_or("Default")));
                if let Some(warning) = &profile_status.warning {
                    ui.colored_label(pal.warning, warning);
                }
            }
            // Leaving the editor (or closing its live toggle) hands the board
            // back to the effect engine.
            let editor_live =
                self.tab == Tab::Create && self.editor.as_ref().map(|e| e.live).unwrap_or(false);
            if !editor_live && self.live_active {
                self.engine.send(Cmd::LivePreview(None));
                self.live_active = false;
            }

            if self.tab == Tab::Apps {
                egui::ScrollArea::vertical().show(ui, |ui| self.app_profiles_ui(ui));
                return;
            }

            if self.tab == Tab::Settings {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.settings_ui(ui);
                });
                return;
            }

            if self.tab == Tab::Create {
                let dt = self.last_tick.elapsed().as_secs_f32();
                self.last_tick = std::time::Instant::now();
                let anim_dir = self.anim_dir.clone();
                let saved = self.editor.as_ref().map_or(true, |ed| !ed.dirty);
                let mut new_composition = false;
                ui.horizontal(|ui| {
                    ui.heading("Create");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        new_composition = ui.add_enabled(saved, egui::Button::new("New composition"))
                            .on_disabled_hover_text("Save your changes before starting a new composition.").clicked();
                    });
                });
                theme::muted(ui, "Combine static keys, motion, and painted animation.");
                ui.add_space(12.0);
                if new_composition {
                    self.engine.send(Cmd::LivePreview(None)); self.live_active = false;
                    self.editor = Some(Editor::new(frame.len().max(1)));
                }
                if let Some(ed) = self.editor.as_mut() {
                    ed.tick(dt);
                    let matching_size = ed.comp.leds == frame.len();
                    let can_stream = matching_size && matches!(&status, DeviceStatus::Connected { .. });
                    if !can_stream {
                        ed.live = false;
                        if self.live_active {
                            self.engine.send(Cmd::LivePreview(None));
                            self.live_active = false;
                        }
                    }
                    if !matching_size {
                        ui.colored_label(pal.warning, format!("This composition has {} LEDs; the keyboard has {}. Create a new composition for this keyboard. Your existing work is preserved.", ed.comp.leds, frame.len()));
                    }
                    editor::show(ui, ed, &layout, &anim_dir, can_save && matching_size, can_stream);
                    if ed.frame_requested {
                        ed.frame_requested = false;
                        let f = ed.preview_frame(ed.comp.leds, &layout);
                        if let DeviceStatus::Connected { id, .. } = &status {
                            self.engine.send(Cmd::WriteToNvram { target: id.clone(), frame: f });
                        }
                    }

                    if let Some(result) =
                        self.engine.shared.lock().unwrap().last_nvram_result.take()
                    {
                        ed.nvram_result = Some(result);
                    }
                    // Stream the editor's preview to the board while the toggle
                    // is on, so the keyboard follows the brush.
                    if ed.live {
                        let f = ed.preview_frame(ed.comp.leds, &layout);
                        if let DeviceStatus::Connected { id, .. } = &status {
                            self.engine.send(Cmd::LivePreview(Some((id.clone(), f))));
                        }
                        self.live_active = true;
                    }
                }
                return;
            }
            self.last_tick = std::time::Instant::now();

            egui::ScrollArea::vertical().id_salt("play-workspace").show(ui, |ui| {
            if !show_sidebar {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("compact-effect-picker")
                        .selected_text(effects.get(self.selected).map(|effect| effect.meta.name.as_str()).unwrap_or("Choose an effect"))
                        .show_ui(ui, |ui| {
                            for (index, effect) in effects.iter().enumerate() {
                                let available = !(wireless && (effect.is_animation || effect.is_composition));
                                if ui.add_enabled(available, egui::SelectableLabel::new(self.selected == index, &effect.meta.name)).clicked() {
                                    self.selected = index;
                                    self.params = Params::from_specs(&effect.meta.params);
                                    self.engine.send(Cmd::SelectEffect(index));
                                }
                            }
                        });
                    if ui.button("Import…").clicked() {
                        if let Some(message) = self.import_dialog(&layout, frame.len()) { self.notice = Some(message); }
                    }
                    ui.menu_button("Library", |ui| {
                        if ui.button("Refresh effects").clicked() { self.engine.send(Cmd::Rescan); ui.close_menu(); }
                        if ui.button("Open effects folder").clicked() { open_folder(&self.effects_dir); ui.close_menu(); }
                        if ui.button("Open animations folder").clicked() { open_folder(&self.anim_dir); ui.close_menu(); }
                    });
                });
                ui.add_space(12.0);
            }
            // ---- parameters, generated from the effect's declaration ----
            let Some(effect) = effects.get(self.selected) else {
                theme::muted(ui, "No effects found.");
                return;
            };

            ui.horizontal_wrapped(|ui| {
                ui.heading(&effect.meta.name);
                // Compositions and keyframe animations both open in the editor:
                // a legacy animation is wrapped as a single keyframe layer, so
                // anything made here can be reworked rather than only replaced.
                if effect.is_composition || effect.is_animation {
                    if let Some(file) = effect.file.clone() {
                        if ui
                            .button("Edit composition")
                            .on_hover_text("Open in the composition editor")
                            .clicked()
                        {
                            let path = self.anim_dir.join(&file);
                            let stem = std::path::Path::new(&file)
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| file.clone());
                            let opened = if effect.is_composition {
                                aula_effects::Composition::load(&path)
                                    .map(|c| Editor::from_composition(c, stem.clone()))
                            } else {
                                aula_effects::Animation::load(&path)
                                    .map(|a| Editor::from_animation(a, stem.clone()))
                            };
                            match opened {
                                Ok(ed) => {
                                    self.editor = Some(ed);
                                    self.tab = Tab::Create;
                                }
                                Err(e) => self.notice = Some(format!("Could not open: {e}")),
                            }
                        }
                    }
                }
            });
            if let Some(f) = &effect.file {
                let kind = if effect.is_composition {
                    "composition"
                } else if effect.is_animation {
                    "animation"
                } else {
                    "script"
                };
                theme::muted(ui, format!("{kind}: {f}"));
            }
            theme::muted(ui, &effect.meta.description);
            ui.add_space(18.0);
            theme::surface(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Keyboard preview").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { theme::muted(ui, if running { "Live" } else { "Paused" }); });
                });
                ui.add_space(10.0);
                let height = (ui.available_width() * 0.30).clamp(160.0, 230.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
                board::draw_board(ui, rect, &layout, &frame);
            });
            if wireless {
                ui.add_space(12.0);
                ui.colored_label(pal.warning, "Connected wirelessly. Use USB for smooth animation playback.");
            }
            ui.add_space(18.0);
            let mut changed = false;
            theme::surface(ui, |ui| {
                theme::section(ui, "Effect controls", "Changes are applied to your keyboard immediately.");
                changed = params_ui(ui, &effect.meta.params, &mut self.params);
            });

            if effect.meta.params.is_empty() {
                theme::muted(ui, "This effect has no parameters.");
            }

            if changed {
                self.engine.send(Cmd::SetParams { revision: self.selection_revision, params: self.params.clone() });
            }
            });
        });
    }
}

/// A filled status dot, drawn rather than typed: the `●` glyph is not in the
/// bundled font and renders as a tofu box.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn preset_ui(
    ui: &mut egui::Ui,
    preset: &mut LightingPreset,
    effects: &[crate::engine::EffectInfo],
) -> bool {
    let mut changed = false;
    let label = effects
        .iter()
        .find(|e| e.meta.id == preset.effect)
        .map(|e| e.meta.name.as_str())
        .unwrap_or(&preset.effect)
        .to_owned();
    ui.horizontal_wrapped(|ui| {
        ui.label("Effect");
        egui::ComboBox::from_id_salt("preset-effect")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for effect in effects {
                    if ui
                        .selectable_label(preset.effect == effect.meta.id, &effect.meta.name)
                        .clicked()
                        && preset.effect != effect.meta.id
                    {
                        *preset = LightingPreset::capture(
                            effect.meta.id.clone(),
                            &Params::from_specs(&effect.meta.params),
                        );
                        changed = true;
                    }
                }
            });
    });
    if let Some(effect) = effects.iter().find(|e| e.meta.id == preset.effect) {
        let mut params = preset.restore(&effect.meta.params);
        if params_ui(ui, &effect.meta.params, &mut params) {
            *preset = LightingPreset::capture(preset.effect.clone(), &params);
            changed = true;
        }
    } else {
        ui.colored_label(
            theme::current().warning,
            "Effect unavailable. Choose another effect or restore its file.",
        );
    }
    changed
}

fn params_ui(ui: &mut egui::Ui, specs: &[ParamSpec], params: &mut Params) -> bool {
    let mut changed = false;
    for spec in specs {
        ui.push_id(&spec.id, |ui| {
            let mut control = |ui: &mut egui::Ui, label_id: egui::Id| {
                ui.spacing_mut().slider_width = (ui.available_width() - 70.0).clamp(70.0, 240.0);
                ui.spacing_mut().text_edit_width = ui.available_width().clamp(80.0, 360.0);
                match &spec.kind {
                    ParamKind::Float { min, max, default } => {
                        let mut v = params.float(&spec.id, *default);
                        if ui
                            .add(egui::Slider::new(&mut v, *min..=*max))
                            .labelled_by(label_id)
                            .changed()
                        {
                            params.set(&spec.id, Value::Float(v));
                            changed = true;
                        }
                    }
                    ParamKind::Int { min, max, default } => {
                        let mut v = params.int(&spec.id, *default);
                        if ui
                            .add(egui::Slider::new(&mut v, *min..=*max))
                            .labelled_by(label_id)
                            .changed()
                        {
                            params.set(&spec.id, Value::Int(v));
                            changed = true;
                        }
                    }
                    ParamKind::Bool { default } => {
                        let mut v = params.bool(&spec.id, *default);
                        if ui.checkbox(&mut v, "").labelled_by(label_id).changed() {
                            params.set(&spec.id, Value::Bool(v));
                            changed = true;
                        }
                    }
                    ParamKind::Color { default } => {
                        let c = params.color(&spec.id, *default);
                        let mut rgb = [c.r, c.g, c.b];
                        if theme::color_edit(ui, &spec.label, &mut rgb)
                            .labelled_by(label_id)
                            .changed()
                        {
                            params.set(&spec.id, Value::Color(Rgb::new(rgb[0], rgb[1], rgb[2])));
                            changed = true;
                        }
                    }
                    ParamKind::Text { default } => {
                        let mut v = params.text(&spec.id, default);
                        if ui
                            .text_edit_singleline(&mut v)
                            .labelled_by(label_id)
                            .changed()
                        {
                            params.set(&spec.id, Value::Text(v));
                            changed = true;
                        }
                    }
                    ParamKind::Choice { options, default } => {
                        let cur = match params.get(&spec.id) {
                            Some(Value::Choice(v)) => *v,
                            Some(Value::Int(v)) => *v as usize,
                            _ => *default,
                        };
                        let text = options.get(cur).cloned().unwrap_or_default();
                        egui::ComboBox::from_id_salt(&spec.id)
                            .selected_text(text)
                            .show_ui(ui, |ui| {
                                for (i, opt) in options.iter().enumerate() {
                                    if ui.selectable_label(i == cur, opt).clicked() {
                                        params.set(&spec.id, Value::Choice(i));
                                        changed = true;
                                    }
                                }
                            });
                    }
                }
            };
            if ui.available_width() >= 460.0 {
                ui.horizontal(|ui| {
                    let label = ui.add_sized(
                        [145.0, 32.0],
                        egui::Label::new(&spec.label).halign(egui::Align::Min),
                    );
                    control(ui, label.id);
                });
            } else {
                let label = ui.label(&spec.label);
                control(ui, label.id);
            }
        });
    }

    changed
}
