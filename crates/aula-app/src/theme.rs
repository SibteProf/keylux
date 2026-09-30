//! Before this module the status and board colours were inline `Color32`
//! literals repeated across `ui.rs` and `editor.rs`, which drifted apart and
//! made a coherent look impossible. Everything visual now names a token here,
//! so the whole app can be reskinned in one place.

use eframe::egui::{self, Color32};
use std::sync::RwLock;

/// Use `RwLock` for blocking access to current palette
static CURRENT: RwLock<&'static Palette> = RwLock::new(&DARK);

/// Named colour tokens. Dark/Light-first, matching the app's existing look, but
/// centralised so a other [`Palette`]-s could define an another theme later.
pub struct Palette {
    /// Is this theme dark.
    pub dark_mode: bool,
    /// Window background, behind every panel. Also a `TextEdit` background.
    pub bg: Color32,
    /// Panel and card fill.
    pub surface: Color32,
    /// Slightly changed fill: buttons, selected rows, inset wells.
    pub surface_alt: Color32,
    /// Buttons background when hover
    pub hovered_bg: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    /// Selection / focus / primary action.
    pub accent: Color32,
    /// Connected, healthy.
    pub success: Color32,
    /// Waiting, unsaved, soft warning.
    pub warning: Color32,
    /// Disconnected, error.
    pub danger: Color32,
    /// An unlit key on the board preview — visible so it still reads as a key.
    pub key_unlit: Color32,
    /// Hairline around each key.
    pub key_stroke: Color32,
    /// The board's own backdrop.
    pub board_bg: Color32,
}

/// The default dark palette.
pub const DARK: Palette = Palette {
    dark_mode: true,
    bg: Color32::from_gray(16),
    surface: Color32::from_gray(24),
    surface_alt: Color32::from_gray(34),
    hovered_bg: Color32::from_gray(34),
    text: Color32::from_gray(225),
    text_muted: Color32::from_gray(150),
    accent: Color32::from_rgb(95, 145, 192),
    success: Color32::from_rgb(80, 200, 120),
    warning: Color32::from_rgb(230, 170, 60),
    danger: Color32::from_rgb(220, 90, 90),
    key_unlit: Color32::from_gray(38),
    key_stroke: Color32::from_gray(60),
    board_bg: Color32::from_gray(18),
};

/// The light palette.
pub const LIGHT: Palette = Palette {
    dark_mode: false,
    bg: Color32::from_rgb(246, 247, 248),
    surface: Color32::from_rgb(253, 254, 255),
    surface_alt: Color32::from_gray(240),
    hovered_bg: Color32::from_gray(240),
    text: Color32::from_rgb(65, 72, 82),
    text_muted: Color32::from_rgb(119, 127, 137),
    accent: Color32::from_rgb(138, 185, 230),
    success: Color32::from_rgb(40, 160, 80),
    warning: Color32::from_rgb(190, 130, 20),
    danger: Color32::from_rgb(190, 60, 60),
    key_unlit: Color32::from_rgb(241, 243, 245),
    key_stroke: Color32::from_rgb(217, 222, 229),
    board_bg: Color32::from_rgb(250, 251, 252),
};

/// Return the current selected palette
pub fn current() -> &'static Palette {
    *CURRENT.read().unwrap()
}

/// Switch the active palette.
pub fn set_current(theme: crate::settings::ColorTheme) {
    use crate::settings::ColorTheme;

    let pal = match theme {
        ColorTheme::Dark => &DARK,
        ColorTheme::Light => &LIGHT,
    };
    *CURRENT.write().unwrap() = pal;
}

impl Palette {
    /// Apply the palette to egui's global style: panel fills, selection colour,
    /// rounded widgets, and a touch more breathing room than the default.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        let v = &mut style.visuals;

        v.dark_mode = self.dark_mode;
        v.override_text_color = Some(self.text);
        v.panel_fill = self.surface;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.bg;
        v.faint_bg_color = self.surface_alt;
        v.selection.bg_fill = self.accent;
        v.selection.stroke = egui::Stroke::new(1.0_f32, self.accent);
        v.hyperlink_color = self.accent;

        let rounding = egui::Rounding::same(5.0);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = rounding;
        }

        // Enable left side of slider fill (which is disabled in light mode by default)
        v.slider_trailing_fill = true;

        // Buttons and combo-box
        v.widgets.inactive.weak_bg_fill = self.surface_alt;
        v.widgets.hovered.weak_bg_fill = self.hovered_bg;
        v.widgets.active.weak_bg_fill = self.hovered_bg;
        v.widgets.noninteractive.weak_bg_fill = self.hovered_bg; // disabled buttons
        v.widgets.open.weak_bg_fill = self.accent; // combo-box while opened

        // Slider dot color when interacting
        v.widgets.active.bg_fill = self.accent;

        // Separators color
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, self.key_stroke);

        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);

        ctx.set_style(style);
    }
}
