//! Showing a hidden window from a thread that is not the UI thread.
//!
//! This exists because of one awkward fact: eframe only calls `App::update`
//! in response to a redraw, and a hidden window is never asked to paint. So
//! once the window is hidden, `Context::request_repaint` does nothing and
//! `send_viewport_cmd` is queued into an output that is never flushed — the UI
//! thread is, for practical purposes, asleep and cannot be woken from inside
//! egui at all.
//!
//! The way back in is to make the window visible through the windowing system
//! directly. After that the paint messages resume, `update` runs again, and the
//! rest of the restore (focus, un-minimise, bookkeeping) happens normally in
//! egui.
//!
//! Only Windows is implemented. Elsewhere [`WindowRef::capture`] returns `None`
//! and the caller must refuse to hide the window at all, so no platform can end
//! up with a window it cannot get back.

use raw_window_handle::HasWindowHandle;

/// A thread-safe reference to the app's OS window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowRef {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    hwnd: isize,
}

impl WindowRef {
    /// Take a reference to the window, or `None` if this platform has no
    /// implementation of [`show`](Self::show).
    ///
    /// The handle is only used to ask the windowing system to show a window we
    /// own, and stays valid for as long as the app does.
    pub fn capture(window: &impl HasWindowHandle) -> Option<Self> {
        let _ = window;
        #[cfg(target_os = "windows")]
        {
            use raw_window_handle::RawWindowHandle;
            match window.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(Self { hwnd: h.hwnd.get() }),
                _ => None,
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    /// Un-hide, un-minimise and raise the window. Safe to call from any thread.
    pub fn show(self) {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
            };
            let hwnd = self.hwnd as windows_sys::Win32::Foundation::HWND;
            // SW_SHOW undoes the hide; SW_RESTORE additionally un-minimises,
            // which matters because minimising is one of the ways we hide.
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}
