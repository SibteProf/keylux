
// Platform-specific window control.
//
// Windows uses HWND/DWM.
// macOS uses NSWindow and NSApplication.

use raw_window_handle::HasWindowHandle;

/// A thread-safe reference to the app's OS window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowRef {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    hwnd: isize,

    #[cfg(target_os = "macos")]
    ns_window: usize,
}

impl WindowRef {
    /// Capture the native window handle.
    pub fn capture(window: &impl HasWindowHandle) -> Option<Self> {
        use raw_window_handle::RawWindowHandle;

        let handle = window.window_handle().ok()?;

        match handle.as_raw() {
            #[cfg(target_os = "windows")]
            RawWindowHandle::Win32(h) => Some(Self {
                hwnd: h.hwnd.get(),
            }),

            #[cfg(target_os = "macos")]
            RawWindowHandle::AppKit(h) => {
                use objc::runtime::Object;
                use objc::{msg_send, sel, sel_impl};

                let view = h.ns_view.as_ptr() as *mut Object;
                let ns_window: *mut Object =
                    unsafe { msg_send![view, window] };

                if ns_window.is_null() {
                    return None;
                }

                Some(Self {
                    hwnd: 0,
                    ns_window: ns_window as usize,
                })
            }

            _ => None,
        }
    }

    /// Keep the window out of view.
    pub fn set_cloaked(self, cloaked: bool) -> bool {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::{
                Foundation::{BOOL, HWND},
                Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK},
            };

            let value: BOOL = i32::from(cloaked);

            unsafe {
                DwmSetWindowAttribute(
                    self.hwnd as HWND,
                    DWMWA_CLOAK as u32,
                    (&value as *const BOOL).cast(),
                    std::mem::size_of::<BOOL>() as u32,
                ) >= 0
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = cloaked;
            false
        }
    }

    /// Change macOS Dock visibility.
    ///
    /// Accessory = 1 (no Dock icon).
    /// Regular = 0 (normal Dock icon).
    #[cfg(target_os = "macos")]
    fn set_activation_policy(accessory: bool) {
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};

        unsafe {
            let app: *mut Object =
                msg_send![class!(NSApplication), sharedApplication];

            let policy: isize = if accessory { 1 } else { 0 };

            let _: bool = msg_send![app, setActivationPolicy: policy];
        }
    }

    /// Hide the application from the macOS Dock.
    ///
    /// Must be called on the main thread.
    #[cfg(target_os = "macos")]
    pub fn hide_from_dock() {
        Self::set_activation_policy(true);
    }

    /// Show the application in the macOS Dock.
    ///
    /// Must be called on the main thread.
    #[cfg(target_os = "macos")]
    pub fn show_in_dock() {
    }

    /// Show, un-minimise and focus the window.
    ///
    /// Safe to call from the tray event thread.
    pub fn show(self) {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
            };

            let hwnd = self.hwnd as windows_sys::Win32::Foundation::HWND;

            self.set_cloaked(false);
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }

        #[cfg(target_os = "macos")]
        unsafe {
            use objc::runtime::Object;
            use objc::{msg_send, sel, sel_impl};

            let window = self.ns_window as *mut Object;

            // Restore normal Dock visibility before showing the window.
            Self::set_activation_policy(false);

            let _: () = msg_send![
                window,
                performSelectorOnMainThread: sel!(makeKeyAndOrderFront:)
                withObject: std::ptr::null_mut::<Object>()
                waitUntilDone: 0i8
            ];
        }
    }
}

