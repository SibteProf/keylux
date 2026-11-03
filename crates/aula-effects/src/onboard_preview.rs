//! Illustrative previews of onboard lighting, not recovered firmware algorithms.
//!
//! This renderer has no device access. Reactive modes use synthetic key presses;
//! speed, brightness and palette handling are approximations for the UI only.

use std::f32::consts::TAU;

use aula_protocol::{onboard::LightingSettings, Frame, KeyPos, Rgb};

/// Render a preview into the same frame type used by software effects.
/// Unsupported modes (including the unread custom key table) stay unlit.
pub fn render(
    settings: LightingSettings,
    palette: &[Rgb; 7],
    seconds: f32,
    layout: &[KeyPos],
    out: &mut Frame,
) {
    out.fill(Rgb::BLACK);
    let width = layout.iter().map(|k| k.x).fold(1.0_f32, f32::max);
    let rows = layout.iter().map(|k| k.row).max().unwrap_or(1).max(1) as f32;
    let time = seconds.max(0.0) * (0.12 + f32::from(settings.speed.min(15)) * 0.08);
    let brightness = (f32::from(settings.brightness.min(20)) + 1.0) / 21.0;

    // Synthetic presses let reactive effects demonstrate their shape without
    // recording global keyboard input or communicating with the receiver.
    let press = time.floor();
    let center_x = noise(press + 1.0);
    let center_y = noise(press + 8.0);

    for key in layout {
        let x = key.x / width;
        let y = f32::from(key.row) / rows;
        let dx = x - 0.5;
        let dy = (y - 0.5) * 0.4;
        let radius = dx.hypot(dy);
        let angle = dy.atan2(dx) / TAU;
        let distance = (x - center_x).hypot((y - center_y) * 0.4);
        let age = time.fract();
        let (phase, intensity) = match settings.mode {
            1 => (x, 1.0), // Static
            2 => (time * 0.15, 0.5 - 0.5 * (time * TAU).cos()),
            3 => (x + y * 0.2 - time, 1.0), // Dream Rainbow
            4 => (press * 0.23, (-distance * 45.0).exp() * (1.0 - age)),
            5 => {
                // Rain Walk: columns of falling light.
                let column = (x * 18.0).floor();
                let tail = (y - time * 2.0 - noise(column)).rem_euclid(1.0);
                (column * 0.13 + time * 0.1, (-tail * 10.0).exp())
            }
            7 => (
                distance - age,
                (1.0 - (distance - age * 0.9).abs() * 14.0).max(0.0),
            ),
            8 => {
                // Starry Sky
                let seed = noise(key.led as f32 + 1.0);
                (
                    seed + time * 0.1,
                    (time * TAU + seed * TAU).sin().max(0.0).powi(6),
                )
            }
            10 => (
                x - time,
                (0.5 + 0.5 * ((x * 2.0 - time) * TAU).cos()).powi(3),
            ),
            11 => (x * 0.7 + y * 0.3 - time, 1.0), // Drifting
            12 => (press * 0.23, (1.0 - distance * 7.0).max(0.0) * (1.0 - age)),
            13 => {
                let line = 0.5 + 0.4 * ((x - time) * TAU).sin();
                (x - time, (1.0 - (y - line).abs() * 4.0).max(0.0))
            }
            15 => (
                angle * 3.0 - time,
                0.3 + 0.7 * ((angle * 3.0 - time) * TAU).cos().max(0.0),
            ),
            16 => (y - time + x * 0.15, 1.0), // Colorful Waterfall
            17 => (
                radius * 2.0 - time,
                0.2 + 0.8 * ((radius * 3.0 - time + angle * 5.0) * TAU).cos().max(0.0),
            ),
            _ => continue,
        };
        let color = if settings.multicolor {
            palette_color(palette, phase)
        } else {
            palette[0]
        };
        out.set(key.led, color.scale(brightness * intensity));
    }
}

fn palette_color(palette: &[Rgb; 7], phase: f32) -> Rgb {
    let position = phase.rem_euclid(1.0) * palette.len() as f32;
    let index = (position.floor() as usize).min(palette.len() - 1);
    palette[index]
        .scale(1.0 - position.fract())
        .blend_add(palette[(index + 1) % palette.len()].scale(position.fract()))
}

fn noise(seed: f32) -> f32 {
    ((seed * 12.9898).sin() * 4375.8545).rem_euclid(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aula_protocol::{f75::keymap, onboard::EFFECTS};

    const PALETTE: [Rgb; 7] = [
        Rgb::new(255, 0, 0),
        Rgb::new(0, 255, 0),
        Rgb::new(0, 0, 255),
        Rgb::new(255, 255, 0),
        Rgb::new(255, 0, 255),
        Rgb::new(0, 255, 255),
        Rgb::new(255, 255, 255),
    ];

    fn settings(mode: u16) -> LightingSettings {
        LightingSettings {
            mode,
            brightness: 20,
            speed: 8,
            multicolor: true,
        }
    }

    fn frame(settings: LightingSettings, time: f32) -> Frame {
        let mut out = Frame::black(126);
        render(settings, &PALETTE, time, &keymap::layout(), &mut out);
        out
    }

    #[test]
    fn every_animated_mode_moves_but_static_and_off_do_not() {
        for &(mode, name) in EFFECTS {
            let a = frame(settings(mode), 0.1);
            let b = frame(settings(mode), 0.73);
            if matches!(mode, 0 | 1 | 277) {
                assert_eq!(a, b, "{name}");
            } else {
                assert_ne!(a, b, "{name} must animate");
            }
        }
    }

    #[test]
    fn controls_affect_preview_and_single_color_uses_saved_color() {
        let full = settings(11);
        let mut changed = full;
        changed.brightness = 0;
        assert_ne!(frame(full, 0.73), frame(changed, 0.73));
        changed = full;
        changed.speed = 0;
        assert_ne!(frame(full, 0.73), frame(changed, 0.73));
        changed.multicolor = false;
        let single = frame(changed, 0.73);
        assert!(keymap::layout()
            .iter()
            .all(|k| single.get(k.led) == PALETTE[0]));
    }

    #[test]
    fn unavailable_modes_clear_previous_frame_and_empty_layout_is_safe() {
        for mode in [0, 277, 999] {
            let mut out = Frame::solid(126, Rgb::new(255, 255, 255));
            render(settings(mode), &PALETTE, 1.0, &keymap::layout(), &mut out);
            assert!(out.iter().all(|color| color.is_black()));
        }
        render(settings(11), &PALETTE, 0.0, &[], &mut Frame::black(0));
    }
}
