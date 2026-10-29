//! XDG desktop-session autostart. Core file/encoding functions are also built
//! in host tests so they can be exercised without changing real login settings.

use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "keylux.desktop";

fn config_home(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(path) = xdg.map(PathBuf::from).filter(|path| path.is_absolute()) {
        return Ok(path);
    }
    home.map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(".config"))
        .ok_or_else(|| {
            "Set XDG_CONFIG_HOME or HOME to an absolute path to configure startup.".into()
        })
}

#[cfg(target_os = "linux")]
fn locations() -> Result<(PathBuf, Vec<PathBuf>), String> {
    let config = config_home(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    let system = std::env::var_os("XDG_CONFIG_DIRS")
        .filter(|value| !value.is_empty())
        .map(|value| {
            std::env::split_paths(&value)
                .filter(|path| path.is_absolute())
                .collect()
        })
        .unwrap_or_else(|| vec![PathBuf::from("/etc/xdg")]);
    Ok((config, system))
}

#[cfg(target_os = "linux")]
pub(super) fn enabled() -> Result<bool, String> {
    let (user, system) = locations()?;
    read_enabled(&user, &system)
}

#[cfg(target_os = "linux")]
pub(super) fn set_enabled(enabled: bool) -> Result<(), String> {
    let (user, _) = locations()?;
    if enabled {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let exe = exe
            .to_str()
            .ok_or("Move keylux to a folder with a UTF-8 path before enabling startup.")?;
        write(&user, exe, true)
    } else {
        write(&user, "", false)
    }
}

fn entry_enabled(text: &str) -> bool {
    let mut main_group = false;
    let (mut application, mut command, mut hidden, mut desktop_disabled) =
        (false, false, false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            main_group = line == "[Desktop Entry]";
            continue;
        }
        if !main_group {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "Type" => application = value.trim() == "Application",
                "Exec" => command = !value.trim().is_empty(),
                "Hidden" => hidden = value.trim() == "true",
                "X-GNOME-Autostart-enabled" => desktop_disabled = value.trim() == "false",
                _ => {}
            }
        }
    }
    application && command && !hidden && !desktop_disabled
}

fn read_enabled(user: &Path, system: &[PathBuf]) -> Result<bool, String> {
    for config in std::iter::once(user).chain(system.iter().map(PathBuf::as_path)) {
        let path = config.join("autostart").join(FILE_NAME);
        match fs::read_to_string(&path) {
            Ok(text) => return Ok(entry_enabled(&text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
        }
    }
    Ok(false)
}

/// Desktop string escaping is applied after Exec argument quoting, which is
/// distinct from shell escaping. No shell or environment expansion is used.
fn desktop_string(value: &str) -> String {
    value.replace('\\', "\\\\")
}

fn desktop_entry(exe: &str, enabled: bool) -> Result<String, String> {
    if !enabled {
        // A user override is necessary: deleting our file could expose a
        // system-wide autostart entry and unexpectedly launch the app again.
        return Ok("[Desktop Entry]\nType=Application\nName=keylux\nHidden=true\n".into());
    }
    if !exe.starts_with('/')
        || exe.contains('=')
        || exe.chars().any(char::is_control)
        || exe.ends_with('/')
    {
        return Err("Move keylux to an absolute executable path without '=' or control characters before enabling startup.".into());
    }
    let mut quoted = String::from("\"");
    for character in exe.chars() {
        match character {
            '\\' | '"' | '`' | '$' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    let parent = exe
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .filter(|parent| !parent.is_empty())
        .unwrap_or("/");
    Ok(format!("[Desktop Entry]\nType=Application\nName=keylux\nComment=Keyboard lighting\nExec={}\nPath={}\nTerminal=false\nHidden=false\n", desktop_string(&quoted), desktop_string(parent)))
}

fn write(config: &Path, exe: &str, enabled: bool) -> Result<(), String> {
    let text = desktop_entry(exe, enabled)?;
    let dir = config.join("autostart");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let temporary = dir.join(format!(".keylux-{}-{stamp}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // Atomic replacement also replaces a symlink instead of editing its
        // target, so a user's override never changes a system-wide file.
        fs::rename(&temporary, dir.join(FILE_NAME))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("keylux-autostart-{}-{stamp}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            if let (Ok(path), Ok(base)) =
                (self.0.canonicalize(), std::env::temp_dir().canonicalize())
            {
                if path.parent() == Some(base.as_path())
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("keylux-autostart-"))
                {
                    let _ = fs::remove_dir_all(path);
                }
            }
        }
    }

    #[test]
    fn xdg_directory_overrides_home_and_invalid_paths_fall_back() {
        let root = Temp::new();
        assert_eq!(config_home(Some(root.0.as_os_str()), None).unwrap(), root.0);
        assert_eq!(
            config_home(Some(OsStr::new("relative")), Some(root.0.as_os_str())).unwrap(),
            root.0.join(".config")
        );
        assert_eq!(
            config_home(Some(OsStr::new("")), Some(root.0.as_os_str())).unwrap(),
            root.0.join(".config")
        );
        assert!(config_home(None, None).is_err());
    }

    #[test]
    fn exec_quotes_spaces_unicode_and_desktop_reserved_characters() {
        let text = desktop_entry("/opt/My Apps/کیlux/keylux", true).unwrap();
        assert!(text.contains("Exec=\"/opt/My Apps/کیlux/keylux\"\n"));
        assert!(text.contains("Path=/opt/My Apps/کیlux\n"));
        let text = desktop_entry("/opt/a\\b\"$`%/keylux", true).unwrap();
        assert!(
            text.contains(r#"Exec="/opt/a\\\\b\\"\\$\\`%%/keylux""#),
            "{text}"
        );
        assert!(desktop_entry("relative/keylux", true).is_err());
        assert!(desktop_entry("/opt/a=b/keylux", true).is_err());
        assert!(desktop_entry("/opt/a\nHidden=true/keylux", true).is_err());
    }

    #[test]
    fn enabling_disabling_and_reenabling_override_system_entries() {
        let root = Temp::new();
        let user = root.0.join("user");
        let system = root.0.join("system");
        let dirs = [system.clone()];
        assert!(!read_enabled(&user, &dirs).unwrap());
        write(&system, "/usr/bin/keylux", true).unwrap();
        assert!(read_enabled(&user, &dirs).unwrap());
        write(&user, "", false).unwrap();
        assert!(!read_enabled(&user, &dirs).unwrap());
        write(&user, "/opt/My Apps/keylux", true).unwrap();
        assert!(read_enabled(&user, &dirs).unwrap());
        write(&user, "", false).unwrap();
        write(&user, "", false).unwrap();
        assert!(!read_enabled(&user, &dirs).unwrap());
        assert!(
            read_enabled(&system, &[]).unwrap(),
            "system entry was not modified"
        );
        assert_eq!(
            fs::read_dir(user.join("autostart")).unwrap().count(),
            1,
            "no temporary files remain"
        );
    }

    #[test]
    fn desktop_environment_disabled_entries_and_unrelated_groups_are_respected() {
        let base = "[Desktop Entry]\nType=Application\nExec=keylux\n";
        assert!(entry_enabled(base));
        assert!(!entry_enabled(&format!("{base}Hidden=true\n")));
        assert!(!entry_enabled(&format!(
            "{base}X-GNOME-Autostart-enabled=false\n"
        )));
        assert!(entry_enabled(&format!(
            "{base}[Desktop Action Other]\nHidden=true\n"
        )));
        assert!(!entry_enabled("[Desktop Entry]\nHidden=false\n"));
    }
}
