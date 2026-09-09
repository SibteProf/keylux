//! Small persisted preferences.
//!
//! Deliberately tiny and hand-rolled rather than using eframe's storage: the
//! only things worth remembering across runs are the window-close behaviour and
//! whether minimising hides to the tray, and both need to be readable and
//! hand-editable by a user who has trapped themselves behind a bad choice.

use std::path::PathBuf;

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            close_action: CloseAction::Ask,
            minimize_to_tray: true,
            run_in_background: true,
            max_fps: aula_protocol::f75::protocol::MAX_FPS,
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
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.close_action, CloseAction::Tray);
        assert!(!back.minimize_to_tray);
        assert!(!back.run_in_background);
        assert_eq!(back.max_fps, 12);
    }

    /// A settings file written by an older build must not wipe the user's
    /// other preferences, and a corrupt one must not stop the app starting.
    #[test]
    fn missing_and_bad_fields_fall_back_to_defaults() {
        let partial: Settings = serde_json::from_str(r#"{"close_action":"quit"}"#).unwrap();
        assert_eq!(partial.close_action, CloseAction::Quit);
        assert!(partial.minimize_to_tray, "unset fields keep their default");

        assert!(serde_json::from_str::<Settings>("not json").is_err());
    }
}
