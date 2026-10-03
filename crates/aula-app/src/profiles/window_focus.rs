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
pub fn active_process_name() -> Option<String> {
    #[cfg(target_os = "linux")]
    return linux_active();
    #[cfg(not(target_os = "linux"))]
    None
}
/// Get active window on Linux with X11.
///
/// Wayland is not supported.
#[cfg(target_os = "linux")]
fn linux_active() -> Option<String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    // Connect to X11 server and setup connection with screen's root.
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    // Get atom (id) of _NET_ACTIVE_WINDOW parameter.
    // _NET_ACTIVE_WINDOW is not in `AtomEnum`, so it has to be interned by name.
    let active_atom = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    // Request for _NET_ACTIVE_WINDOW option
    let reply = conn
        .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let id = reply.value32()?.next()?;
    if id == 0 {
        return None;
    }
    let class = linux_get_class(&conn, id)?;
    Some(class)
}
// Get list of launched apps
pub fn list_windows() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        linux_app_list()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}
/// Get list of windows on Linux with X11.
///
/// Wayland is not supported.
#[cfg(target_os = "linux")]
fn linux_app_list() -> Vec<String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let mut classes: Vec<String> = Vec::new();
    // Connect to X11 server and setup connection with screen's root.
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return classes;
    };
    let root = conn.setup().roots[screen_num].root;
    // Get atom (id) of _NET_CLIENT_LIST parameter.
    // _NET_CLIENT_LIST is not in `AtomEnum`, so it has to be interned by name.
    let Ok(cookie) = conn.intern_atom(false, b"_NET_CLIENT_LIST") else {
        return classes;
    };
    let Ok(reply) = cookie.reply() else {
        return classes;
    };
    let list_atom = reply.atom;
    // Request for _NET_CLIENT_LIST option
    let Ok(cookie) = conn.get_property(false, root, list_atom, AtomEnum::WINDOW, 0, 1024) else {
        return classes;
    };
    let Ok(reply) = cookie.reply() else {
        return classes;
    };
    let Some(ids) = reply.value32() else {
        return classes;
    };
    for id in ids {
        if id == 0 {
            continue;
        }
        if let Some(class) = linux_get_class(&conn, id) {
            classes.push(class);
        }
    }
    classes.sort();
    classes.dedup();
    classes
}
/// Whether this build can currently report the focused application.
///
/// Not "does the platform support it" — Linux with X11 can, and so can
/// Windows and macOS in principle. This answers whether *this build* has
/// the implementation, so a platform whose driver is not written yet says
/// no rather than quietly doing nothing when profiles are turned on.
///
/// On Linux it also asks the environment. Wayland has no mechanism
/// equivalent to X11's `_NET_ACTIVE_WINDOW`: a client cannot ask the
/// compositor which surface has focus, by design. A Wayland session
/// therefore answers no even though the Linux driver exists.
pub fn available() -> bool {
    #[cfg(target_os = "linux")]
    return match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("x11") => true,
        Ok("wayland") => false,
        _ => std::env::var("DISPLAY").is_ok(),
    };
    #[cfg(not(target_os = "linux"))]
    false
}
#[cfg(target_os = "linux")]
fn linux_get_class(conn: &impl x11rb::connection::Connection, window: u32) -> Option<String> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    // WM_CLASS is "instance\0class\0". The class — the second field — is what
    // identifies the application: the same for every window it opens, where the
    // instance can differ between launches. Matching on the class means one
    // profile covers every window of an application, which is what "while this
    // app is focused" is supposed to mean.
    let reply = conn
        .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)
        .ok()?
        .reply()
        .ok()?;
    let mut parts = reply.value.split(|b| *b == 0);
    let _instance = parts.next().filter(|p| !p.is_empty())?;
    let class = parts.next().filter(|p| !p.is_empty())?;
    std::str::from_utf8(class).ok().map(str::to_string)
}
