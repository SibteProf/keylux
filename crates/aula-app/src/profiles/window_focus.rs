//! Which application is in focus, and which could be.
//!
//! The real implementations live in per-platform branches: Linux through
//! X11 (`_NET_ACTIVE_WINDOW` and `_NET_CLIENT_LIST`), Windows through
//! `GetForegroundWindow` and `EnumWindows`, macOS through `NSWorkspace`.
//!
//! This is the stub that fixes the interface. `core` builds and runs with
//! it — the profile editor works, the watcher polls, and nothing happens,
//! because nothing can be determined. A platform branch replaces this file
//! and the rest of the app does not change.

/// The application that currently has focus, as the platform reports it.
///
/// This is the string `Profile.app_id` is matched against — `WM_CLASS` on
/// Linux, the executable's basename on Windows, the bundle identifier on
/// macOS. `None` means the platform cannot say, or nothing has focus.
#[allow(unused)]
pub fn active_process_name() -> Option<String> {
    None
}

/// Every application that could be bound to a profile, for the picker.
///
/// Empty on a platform with no implementation, which the picker treats as
/// "type the name yourself".
#[allow(unused)]
pub fn list_windows() -> Vec<String> {
    Vec::new()
}
