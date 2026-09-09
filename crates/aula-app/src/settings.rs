//! Small persisted preferences.
//!
//! Deliberately tiny and hand-rolled rather than using eframe's storage: the
//! only things worth remembering across runs are the window-close behaviour and
//! whether minimising hides to the tray, and both need to be readable and
//! hand-editable by a user who has trapped themselves behind a bad choice.

use std::path::PathBuf;

use aula_protocol::DeviceId;
use serde::{Deserialize, Serialize};

/// What the window's close button does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseAction {
    /// Ask each time. The default, because guessing wrong either kills the
    /// lighting the user wanted to keep running or hides a window they wanted
    /// gone.
    #[default]
    Ask,
    /// Keep running in the tray.
    Tray,
    /// Quit, which also turns the lighting loop off.
    Quit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub close_action: CloseAction,
    /// Hide the window to the tray when minimised, instead of leaving it on
    /// the taskbar.
    pub minimize_to_tray: bool,
    /// Keep rendering while the window is hidden. Off means the keyboard falls
    /// back to whatever the firmware was last told, which is what most people
    /// expect from "close".
    pub run_in_background: bool,
    /// Write rate cap. The device clamps this to its own ceiling, so a value
    /// from an older or hand-edited file can only ever make things gentler.
    pub max_fps: u32,
    /// Which keyboard to drive, as `"vid:pid"`. `None` means automatic, which
    /// prefers the wired board.
    ///
    /// Deliberately not a HID path: paths change on every replug and differ
    /// between USB ports, so a remembered one would be wrong by the next boot.
    pub device: Option<String>,
    /// Receivers a scan has turned up on this machine, as `"vid:pid"`.
    ///
    /// A 2.4 GHz receiver enumerates under its own chipset's id, which cannot
    /// be shipped in the source table because it differs between production
    /// runs. Remembering it here means normal startup finds it again without
    /// ever probing unrecognised hardware on a timer.
    pub known_devices: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            close_action: CloseAction::Ask,
            minimize_to_tray: true,
            run_in_background: true,
            max_fps: aula_protocol::f75::protocol::MAX_FPS,
            device: None,
            known_devices: Vec::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// The pinned device, if the file names a valid one.
    ///
    /// A malformed entry is ignored rather than fatal: this file is meant to be
    /// hand-editable, and a typo should cost the user their pin, not their app.
    pub fn pinned_device(&self) -> Option<DeviceId> {
        self.device.as_deref()?.parse().ok()
    }

    /// Ids discovery should treat as known, alongside the built-in table.
    pub fn allow_list(&self) -> Vec<DeviceId> {
        self.known_devices
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect()
    }

    /// Pin a device and remember it for future scans.
    pub fn remember_device(&mut self, id: DeviceId) {
        let text = id.to_string();
        if !self.known_devices.iter().any(|k| k.parse() == Ok(id)) {
            self.known_devices.push(text.clone());
        }
        self.device = Some(text);
    }

    /// Go back to automatic selection, keeping what scans have found.
    pub fn unpin_device(&mut self) {
        self.device = None;
    }

    /// Best-effort: a failed write costs the user a remembered preference, not
    /// their work, so it is not worth interrupting them over.
    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// `%APPDATA%\keylux\settings.json`, or `$XDG_CONFIG_HOME/keylux/` and
    /// `~/.config/keylux/` elsewhere.
    pub fn path() -> Option<PathBuf> {
        let base = if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        }?;
        Some(base.join("keylux").join("settings.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = Settings {
            close_action: CloseAction::Tray,
            minimize_to_tray: false,
            run_in_background: false,
            max_fps: 12,
            device: Some("3554:fa09".into()),
            known_devices: vec!["3554:fa09".into()],
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.close_action, CloseAction::Tray);
        assert!(!back.minimize_to_tray);
        assert!(!back.run_in_background);
        assert_eq!(back.max_fps, 12);
        assert_eq!(back.pinned_device(), "3554:fa09".parse().ok());
        assert_eq!(back.allow_list().len(), 1);
    }

    /// A settings file written by an older build must not wipe the user's
    /// other preferences, and a corrupt one must not stop the app starting.
    #[test]
    fn missing_and_bad_fields_fall_back_to_defaults() {
        let partial: Settings = serde_json::from_str(r#"{"close_action":"quit"}"#).unwrap();
        assert_eq!(partial.close_action, CloseAction::Quit);
        assert!(partial.minimize_to_tray, "unset fields keep their default");
        // A file written before this feature existed must still mean
        // "automatic", not "pinned to nothing".
        assert_eq!(partial.device, None);
        assert!(partial.known_devices.is_empty());

        assert!(serde_json::from_str::<Settings>("not json").is_err());
    }

    /// The settings file is documented as hand-editable, so a typo in a device
    /// id has to degrade to "automatic" rather than stopping the app.
    #[test]
    fn a_bad_device_string_is_ignored_not_fatal() {
        let s: Settings = serde_json::from_str(
            r#"{"device":"not-an-id","known_devices":["3554:fa09","rubbish"]}"#,
        )
        .unwrap();
        assert_eq!(s.pinned_device(), None);
        assert_eq!(
            s.allow_list(),
            vec!["3554:fa09".parse::<DeviceId>().unwrap()]
        );
    }

    #[test]
    fn remembering_a_device_pins_it_and_allow_lists_it_once() {
        let id: DeviceId = "3554:fa09".parse().unwrap();
        let mut s = Settings::default();
        s.remember_device(id);
        s.remember_device(id);
        assert_eq!(s.pinned_device(), Some(id));
        assert_eq!(
            s.known_devices.len(),
            1,
            "the allow-list must not grow duplicates"
        );

        // Unpinning goes back to automatic but keeps what the scan found, so a
        // user who tries the dongle and returns to the cable is not made to
        // scan again.
        s.unpin_device();
        assert_eq!(s.pinned_device(), None);
        assert_eq!(s.allow_list(), vec![id]);
    }
}
