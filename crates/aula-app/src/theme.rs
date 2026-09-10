//! One central palette, applied once at startup.
//!
//! Before this module the status and board colours were inline `Color32`
//! literals repeated across `ui.rs` and `editor.rs`, which drifted apart and
//! made a coherent look impossible. Everything visual now names a token here,
//! so the whole app can be reskinned — or given a light theme — in one place.

use eframe::egui::{self, Color32};

/// Named colour tokens. Dark-first, matching the app's existing look, but
/// centralised so a second [`Palette`] could define a light theme later.
pub struct Palette {
    /// Window background, behind every panel.
    pub bg: Color32,
    /// Panel and card fill.
    pub surface: Color32,
    /// Slightly raised fill: selected rows, inset wells.
    pub surface_alt: Color32,
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
    bg: Color32::from_gray(16),
    surface: Color32::from_gray(24),
    surface_alt: Color32::from_gray(34),
    text: Color32::from_gray(225),
    text_muted: Color32::from_gray(150),
    accent: Color32::from_rgb(120, 190, 255),
    success: Color32::from_rgb(80, 200, 120),
    warning: Color32::from_rgb(230, 170, 60),
    danger: Color32::from_rgb(220, 90, 90),
    key_unlit: Color32::from_gray(38),
    key_stroke: Color32::from_gray(60),
    board_bg: Color32::from_gray(18),
};

/// The palette in force. A single constant for now; a settings-driven switch
/// would assign this.
pub fn current() -> &'static Palette {
    &DARK
}

impl Palette {
    /// Apply the palette to egui's global style: panel fills, selection colour,
    /// rounded widgets, and a touch more breathing room than the default.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        let v = &mut style.visuals;

        v.dark_mode = true;
        v.override_text_color = Some(self.text);
        v.panel_fill = self.surface;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.bg;
        v.faint_bg_color = self.surface_alt;
        v.selection.bg_fill = self.accent.linear_multiply(0.45);
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
        v.widgets.inactive.weak_bg_fill = self.surface_alt;
        v.widgets.hovered.weak_bg_fill = self.surface_alt.linear_multiply(1.4);

        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);

        ctx.set_style(style);
    }
}
