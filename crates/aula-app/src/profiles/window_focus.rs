//! Which application is in focus, and which could be.
//!
//! The real implementations live in per-platform branches: Linux through
//! X11 (`_NET_ACTIVE_WINDOW` and `_NET_CLIENT_LIST`), Windows through
//! `GetForegroundWindow` and `EnumWindows`, macOS through `NSWorkspace`.

/// The application that currently has focus, as the platform reports it.
///
/// This is the string `Profile.app_id` is matched against — `WM_CLASS` on
/// Linux, the executable's basename on Windows, the bundle identifier on
/// macOS. `None` means the platform cannot say, or nothing has focus.
pub fn active_process_name() -> Option<String> {
    #[cfg(target_os = "linux")]
    return linux_active();
    #[cfg(target_os = "windows")]
    return windows_active();
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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
/// Get active window on Windows.
#[cfg(target_os = "windows")]
fn windows_active() -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    windows_process_name(hwnd)
}
// Get list of launched apps
pub fn list_windows() -> Vec<String> {
    #[cfg(target_os = "linux")]
    return linux_app_list();
    #[cfg(target_os = "windows")]
    return windows_app_list();
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    return Vec::new();
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
/// Get list of apps on Windows.
#[cfg(target_os = "windows")]
fn windows_app_list() -> Vec<String> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, IsWindowVisible,
    };

    // The callback receives the `LPARAM` we passed to `EnumWindows` and a
    // window handle. We use it to push the handle into a `Vec<HWND>` living
    // on the caller's stack.
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = &mut *(lparam as *mut Vec<HWND>);
        windows.push(hwnd);
        TRUE
    }

    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(Some(collect), &mut handles as *mut _ as LPARAM);
    }

    let mut classes: Vec<String> = Vec::new();
    for hwnd in handles {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                continue;
            }
            // Windows with no title are usually invisible helpers: the
            // IME window, hidden message-only windows, tray icon hosts.
            // Filtering by title is the cheap way to skip them.
            if GetWindowTextLengthW(hwnd) == 0 {
                continue;
            }
        }
        if let Some(name) = windows_process_name(hwnd) {
            classes.push(name);
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
    #[cfg(target_os = "windows")]
    return true;
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    return false;
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
#[cfg(target_os = "windows")]
fn windows_process_name(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }

        // PROCESS_QUERY_LIMITED_INFORMATION is enough to read the image name
        // and does not require elevation for most process. Protected
        // processes (system, DWM, anti-cheats, anti-malware) still fall -
        // that is expected and they return `None`.
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }

        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);

        if ok == 0 {
            return None;
        }

        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
    }
}
