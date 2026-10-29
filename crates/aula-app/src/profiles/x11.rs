//! X11 focus detection adapted from headblade-dev's PR #15.
//! Uses _NET_ACTIVE_WINDOW and the class field of WM_CLASS. No input hooks.

fn session_supported(
    session_type: Option<&str>,
    display: Option<&str>,
    wayland: Option<&str>,
) -> bool {
    if session_type == Some("wayland") || wayland.is_some_and(|s| !s.is_empty()) {
        return false;
    }
    display.is_some_and(|s| !s.trim().is_empty())
        && matches!(session_type, Some("x11") | None | Some(""))
}

fn parse_wm_class(value: &[u8]) -> Option<String> {
    // WM_CLASS contains two NUL-terminated strings: instance, then class.
    // Match the class so multiple windows/instances share one profile.
    let mut parts = value.split(|b| *b == 0);
    let _instance = parts.next()?;
    let class = parts.next().filter(|p| !p.is_empty())?;
    if parts.next()? != b"" || parts.next().is_some() {
        return None;
    }
    std::str::from_utf8(class).ok().map(str::to_owned)
}

#[cfg(target_os = "linux")]
pub fn available() -> bool {
    session_supported(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
    )
}

#[cfg(target_os = "linux")]
pub struct Session {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    active_atom: u32,
    pid_atom: u32,
}

#[cfg(target_os = "linux")]
impl Session {
    pub fn connect() -> anyhow::Result<Self> {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::ConnectionExt;

        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let active_atom = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        let pid_atom = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
        Ok(Self {
            conn,
            root,
            active_atom,
            pid_atom,
        })
    }

    fn active_window(&self) -> anyhow::Result<Option<u32>> {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

        let reply = self
            .conn
            .get_property(false, self.root, self.active_atom, AtomEnum::WINDOW, 0, 1)?
            .reply()?;
        if reply.type_ != u32::from(AtomEnum::WINDOW)
            || reply.format != 32
            || reply.value.len() != 4
            || reply.bytes_after != 0
        {
            return Ok(None);
        }
        Ok(reply
            .value32()
            .and_then(|mut v| v.next())
            .filter(|id| *id != 0))
    }

    pub fn foreground(&self) -> anyhow::Result<super::Foreground> {
        use super::Foreground;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

        let Some(window) = self.active_window()? else {
            return Ok(Foreground::Unavailable);
        };
        let pid = self
            .conn
            .get_property(false, window, self.pid_atom, AtomEnum::CARDINAL, 0, 1)?
            .reply()?;
        let pid = (pid.type_ == u32::from(AtomEnum::CARDINAL) && pid.format == 32)
            .then(|| pid.value32().and_then(|mut v| v.next()))
            .flatten();
        let class = self
            .conn
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)?
            .reply()?;
        // Discard results if focus moved while the properties were read.
        if self.active_window()? != Some(window) {
            return Ok(Foreground::Unavailable);
        }
        if pid == Some(std::process::id()) {
            return Ok(Foreground::OwnWindow);
        }
        if class.type_ != u32::from(AtomEnum::STRING) || class.format != 8 || class.bytes_after != 0
        {
            return Ok(Foreground::Unavailable);
        }
        let Some(class) = parse_wm_class(&class.value) else {
            return Ok(Foreground::Unavailable);
        };
        // Some window managers omit _NET_WM_PID. eframe's app class is keylux.
        if pid.is_none() && class.eq_ignore_ascii_case("keylux") {
            return Ok(Foreground::OwnWindow);
        }
        Ok(Foreground::Application(class))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_uses_the_class_instead_of_the_instance() {
        assert_eq!(
            parse_wm_class(b"browser-42\0Firefox\0").as_deref(),
            Some("Firefox")
        );
        assert_eq!(parse_wm_class(b"\0Code\0").as_deref(), Some("Code"));
        for malformed in [
            b"Firefox".as_slice(),
            b"instance\0",
            b"a\0\0",
            b"a\0b\0extra",
            b"a\0\xff\0",
        ] {
            assert_eq!(parse_wm_class(malformed), None);
        }
    }

    #[test]
    fn x11_rejects_wayland_even_when_xwayland_sets_display() {
        assert!(session_supported(Some("x11"), Some(":1"), None));
        assert!(session_supported(None, Some(":1"), None));
        assert!(!session_supported(Some("wayland"), Some(":1"), None));
        assert!(!session_supported(None, Some(":1"), Some("wayland-0")));
        assert!(!session_supported(Some("x11"), None, None));
        assert!(!session_supported(Some("tty"), Some(":1"), None));
        assert!(!session_supported(Some("x11"), Some(""), None));
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[ignore = "run with xvfb-run on an isolated X server"]
    fn x11_focus_drives_profiles_and_preserves_own_window_lighting() -> anyhow::Result<()> {
        use super::super::{resolve, AppLighting, AppProfile, Foreground, LightingPreset};
        use aula_effects::registry::Registry;
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, CreateWindowAux, WindowClass};
        use x11rb::wrapper::ConnectionExt as _;

        let detector = Session::connect()?;
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let browser = conn.generate_id()?;
        let own = conn.generate_id()?;
        for window in [browser, own] {
            conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                100,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
        }
        conn.change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            browser,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"browser-instance\0Firefox\0",
        )?
        .check()?;
        conn.change_property32(
            x11rb::protocol::xproto::PropMode::REPLACE,
            own,
            detector.pid_atom,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?
        .check()?;
        let config = AppLighting {
            enabled: true,
            default: LightingPreset {
                effect: "solid".into(),
                ..Default::default()
            },
            profiles: vec![AppProfile {
                name: "Browsing".into(),
                applications: "firefox".into(),
                lighting: LightingPreset {
                    effect: "wave".into(),
                    ..Default::default()
                },
                ..Default::default()
            }],
        };
        let reg = Registry::new();
        let focus = |window| -> anyhow::Result<()> {
            conn.change_property32(
                x11rb::protocol::xproto::PropMode::REPLACE,
                root,
                detector.active_atom,
                AtomEnum::WINDOW,
                &[window],
            )?
            .check()?;
            Ok(())
        };
        focus(browser)?;
        let Foreground::Application(class) = detector.foreground()? else {
            anyhow::bail!("missing focused class");
        };
        assert_eq!(class, "Firefox");
        assert_eq!(
            resolve(&config, &reg, Some(&class), false)
                .unwrap()
                .0
                .preset
                .effect,
            "wave"
        );
        conn.change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            browser,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"editor-instance\0Code\0",
        )?
        .check()?;
        let Foreground::Application(class) = detector.foreground()? else {
            anyhow::bail!("missing unmatched class");
        };
        assert_eq!(
            resolve(&config, &reg, Some(&class), false)
                .unwrap()
                .0
                .preset
                .effect,
            "solid"
        );
        focus(own)?;
        assert_eq!(detector.foreground()?, Foreground::OwnWindow);
        focus(0)?;
        assert_eq!(detector.foreground()?, Foreground::Unavailable);
        // Reuse the same connection across focus changes, as the worker does.
        focus(browser)?;
        assert_eq!(
            detector.foreground()?,
            Foreground::Application("Code".into())
        );
        Ok(())
    }
}
