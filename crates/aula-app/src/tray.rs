//! System tray icon.
//!
//! The window can be hidden without stopping the render thread, so the tray is
//! the only way back to it — and the only way to quit once it is hidden. That
//! makes it load-bearing rather than decorative: if the icon fails to create,
//! the app must refuse to hide the window at all. `Tray::new` returning `None`
//! is what the caller checks for.
//!
//! tray-icon delivers events on its own global channels, drained only while
//! something polls them, so blocking reader threads do that here.
//!
//! Those threads run `on_action` directly rather than only queuing for the UI.
//! They have to: eframe calls `App::update` in response to a redraw, and a
//! hidden window is never asked to paint, so while the window is hidden
//! `request_repaint` does nothing and the UI thread cannot be woken from inside
//! egui at all. Clicking "Show" has to reach the windowing system on this
//! thread. Actions are queued too, and replayed by `drain` once frames resume.

use std::sync::{Arc, Mutex};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the user asked for from the tray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// Restore and focus the window.
    Show,
    /// Start or stop the lighting loop.
    TogglePlay,
    /// Quit for real.
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    show_id: tray_icon::menu::MenuId,
    play_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    pending: Arc<Mutex<Vec<TrayAction>>>,
    /// Last state written to the tooltip, so we only touch the native icon
    /// when it actually changes.
    last_tooltip: Option<String>,
}

impl Tray {
    /// Build the tray icon and start forwarding its events.
    ///
    /// `on_action` runs on the reader thread, the moment the click arrives.
    /// That immediacy is the whole point: while the window is hidden the UI
    /// thread is asleep and cannot be woken from inside egui, so anything that
    /// must happen without a repaint — showing the window again above all —
    /// has to happen here. Actions are queued as well, and `drain` replays them
    /// on the next frame for the parts that do belong in the UI.
    pub fn new(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Option<Self> {
        let menu = Menu::new();
        let show = MenuItem::new("Show keylux", true, None);
        // Neutral label rather than "Pause"/"Resume": the text can only be
        // changed from the UI thread, which is asleep whenever the window is
        // hidden, so a stateful label would sit there lying to the user.
        let play = MenuItem::new("Play / pause lighting", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let play_id = play.id().clone();
        menu.append_items(&[
            &show,
            &PredefinedMenuItem::separator(),
            &play,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("keylux")
            .with_icon(app_icon())
            .build()
            .ok()?;

        let pending = Arc::new(Mutex::new(Vec::new()));
        let on_action = Arc::new(on_action);
        let fire = {
            let pending = Arc::clone(&pending);
            let on_action = Arc::clone(&on_action);
            move |action: TrayAction| {
                pending.lock().unwrap().push(action);
                on_action(action);
            }
        };

        // Menu clicks.
        {
            let fire = fire.clone();
            let (show_id, play_id, quit_id) =
                (show.id().clone(), play_id.clone(), quit.id().clone());
            std::thread::Builder::new()
                .name("tray-menu".into())
                .spawn(move || {
                    while let Ok(ev) = MenuEvent::receiver().recv() {
                        let action = if ev.id == show_id {
                            TrayAction::Show
                        } else if ev.id == play_id {
                            TrayAction::TogglePlay
                        } else if ev.id == quit_id {
                            TrayAction::Quit
                        } else {
                            continue;
                        };
                        fire(action);
                    }
                })
                .ok()?;
        }

        // Clicking the icon itself is the obvious way back to the window, and
        // plenty of people never think to open the menu.
        {
            std::thread::Builder::new()
                .name("tray-icon".into())
                .spawn(move || {
                    while let Ok(ev) = TrayIconEvent::receiver().recv() {
                        if let TrayIconEvent::DoubleClick { .. } = ev {
                            fire(TrayAction::Show);
                        }
                    }
                })
                .ok()?;
        }

        Some(Self {
            icon,
            show_id: show.id().clone(),
            play_id: play.id().clone(),
            quit_id: quit.id().clone(),
            pending,
            last_tooltip: None,
        })
    }

    /// Take everything the user has clicked since the last frame.
    pub fn drain(&self) -> Vec<TrayAction> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }

    /// Put the device status in the tooltip, so the tray says something useful
    /// without opening the window. Only runs while the window is visible; the
    /// tooltip simply keeps its last value while hidden.
    pub fn sync(&mut self, running: bool, status: &str) {
        let state = if running { "running" } else { "paused" };
        let tip = format!("keylux — {status}, {state}");
        if self.last_tooltip.as_deref() != Some(tip.as_str()) {
            let _ = self.icon.set_tooltip(Some(&tip));
            self.last_tooltip = Some(tip);
        }
    }

    /// Ids are unused outside construction, but keeping them makes the mapping
    /// above auditable in one place.
    #[allow(dead_code)]
    pub fn ids(&self) -> [&tray_icon::menu::MenuId; 3] {
        [&self.show_id, &self.play_id, &self.quit_id]
    }
}

/// A 32x32 icon drawn in code, so the repo carries no binary asset and the
/// icon cannot go missing at runtime: a dark rounded board with a row of lit
/// keys across it.
fn app_icon() -> Icon {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];

    let put = |rgba: &mut [u8], x: usize, y: usize, c: [u8; 4]| {
        let i = (y * N + x) * 4;
        rgba[i..i + 4].copy_from_slice(&c);
    };

    // Body: rounded rectangle, corners clipped by a radius test.
    let (x0, y0, x1, y1) = (2usize, 6usize, 29usize, 25usize);
    let r = 4.0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = ((x0 as f32 + r) - x as f32)
                .max(x as f32 - (x1 as f32 - r))
                .max(0.0);
            let dy = ((y0 as f32 + r) - y as f32)
                .max(y as f32 - (y1 as f32 - r))
                .max(0.0);
            if dx * dx + dy * dy <= r * r {
                put(&mut rgba, x, y, [28, 30, 36, 255]);
            }
        }
    }

    // Keys: three rows, hue sweeping left to right so the icon reads as RGB
    // even at 16x16 on a crowded taskbar.
    for (row, y) in [10usize, 15, 20].into_iter().enumerate() {
        for (col, x) in (5..=25).step_by(5).enumerate() {
            let hue = (col as f32 * 0.16 + row as f32 * 0.06) % 1.0;
            let c = hsv_bytes(hue);
            for dy in 0..3 {
                for dx in 0..4 {
                    put(&mut rgba, x + dx, y + dy, [c[0], c[1], c[2], 255]);
                }
            }
        }
    }

    Icon::from_rgba(rgba, N as u32, N as u32).expect("32x32 RGBA is a valid icon")
}

fn hsv_bytes(h: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (q, t) = (1.0 - f, f);
    let (r, g, b) = match i as i32 % 6 {
        0 => (1.0, t, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, t),
        3 => (0.0, q, 1.0),
        4 => (t, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The icon is built at startup and `expect`s valid dimensions; a wrong
    /// buffer length would panic the app before the window ever appears.
    #[test]
    fn the_icon_buffer_matches_its_dimensions() {
        let _ = app_icon();
    }

    #[test]
    fn hues_span_the_wheel() {
        assert_eq!(hsv_bytes(0.0), [255, 0, 0]);
        assert_eq!(hsv_bytes(1.0 / 3.0)[1], 255, "a third of the way is green");
    }
}
