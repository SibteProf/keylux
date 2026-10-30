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

fn app_icon() -> Icon {
    let (rgba, w, h) = icon_rgba();
    Icon::from_rgba(rgba, w, h).expect("32x32 RGBA is a valid icon")
}

pub use crate::icon::icon_rgba;

#[cfg(test)]
mod tests {
    use super::*;

    /// The icon is built at startup and `expect`s valid dimensions; a wrong
    /// buffer length would panic the app before the window ever appears.
    #[test]
    fn the_icon_buffer_matches_its_dimensions() {
        let _ = app_icon();
    }
}
