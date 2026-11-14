//! User-controlled sign-in startup. Windows uses HKCU Run; Linux uses XDG
//! autostart. Opening the app only reads registration; the checkbox writes it.

#[cfg(any(target_os = "linux", test))]
#[path = "startup/linux.rs"]
mod linux;

pub fn supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "linux"))
}

pub fn enabled() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        windows::read(windows::RUN_KEY)
            .map(|command| command.is_some_and(|value| !value.is_empty()))
    }
    #[cfg(target_os = "linux")]
    {
        linux::enabled()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Ok(false)
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let command = if enabled {
            Some(windows::command(
                &std::env::current_exe().map_err(|e| e.to_string())?,
            )?)
        } else {
            None
        };
        windows::write(windows::RUN_KEY, command.as_deref())
    }
    #[cfg(target_os = "linux")]
    {
        linux::set_enabled(enabled)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = enabled;
        Err("Run at startup is available on Windows and Linux.".into())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::{os::windows::ffi::OsStrExt, path::Path, ptr};
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::*,
    };

    pub(super) const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Keylux";

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn error(code: u32) -> String {
        std::io::Error::from_raw_os_error(code as i32).to_string()
    }

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn open(path: &str, create: bool) -> Result<Option<Key>, String> {
        let mut key = ptr::null_mut();
        let path = wide(path);
        // The parent is always HKCU: this option never needs elevation or
        // changes startup for another Windows account.
        let result = unsafe {
            if create {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    0,
                    ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    ptr::null(),
                    &mut key,
                    ptr::null_mut(),
                )
            } else {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    0,
                    KEY_QUERY_VALUE,
                    &mut key,
                )
            }
        };
        match result {
            ERROR_SUCCESS => Ok(Some(Key(key))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            code => Err(error(code)),
        }
    }

    /// Quotes the executable as one argument, including spaces and Unicode.
    /// No shell, CLI arguments, or working-directory assumptions are involved.
    pub(super) fn command(exe: &Path) -> Result<Vec<u16>, String> {
        if !exe.is_absolute() {
            return Err("The startup executable must have an absolute path.".into());
        }
        let path: Vec<_> = exe.as_os_str().encode_wide().collect();
        if path.iter().any(|&c| c == 0 || c == u16::from(b'"')) {
            return Err("The startup executable path is invalid.".into());
        }
        if path.len() + 2 > 260 {
            return Err("Move keylux to a shorter folder path before enabling startup.".into());
        }
        Ok(std::iter::once(u16::from(b'"'))
            .chain(path)
            .chain([u16::from(b'"'), 0])
            .collect())
    }

    pub(super) fn read(path: &str) -> Result<Option<Vec<u16>>, String> {
        let Some(key) = open(path, false)? else {
            return Ok(None);
        };
        let name = wide(VALUE);
        let mut kind = 0;
        let mut bytes = 0;
        let result = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null(),
                &mut kind,
                ptr::null_mut(),
                &mut bytes,
            )
        };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if result != ERROR_SUCCESS {
            return Err(error(result));
        }
        if kind != REG_SZ || bytes % 2 != 0 || bytes > 65536 {
            return Err(
                "The existing keylux startup entry is invalid. Toggle startup to replace it."
                    .into(),
            );
        }
        let mut value = vec![0_u16; (bytes / 2) as usize + 1];
        let result = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null(),
                &mut kind,
                value.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if result != ERROR_SUCCESS {
            return Err(error(result));
        }
        if kind != REG_SZ || bytes % 2 != 0 {
            return Err("The startup entry changed while reading it. Try again.".into());
        }
        value.truncate((bytes / 2) as usize);
        while value.last() == Some(&0) {
            value.pop();
        }
        Ok(Some(value))
    }

    pub(super) fn write(path: &str, command: Option<&[u16]>) -> Result<(), String> {
        let name = wide(VALUE);
        if let Some(command) = command {
            let key = open(path, true)?.ok_or("Could not open the startup registry key.")?;
            let result = unsafe {
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    command.as_ptr().cast(),
                    (command.len() * 2) as u32,
                )
            };
            if result != ERROR_SUCCESS {
                return Err(error(result));
            }
        } else {
            // Do not create keys when disabling, and remove only our value.
            let mut key = ptr::null_mut();
            let result = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    wide(path).as_ptr(),
                    0,
                    KEY_SET_VALUE,
                    &mut key,
                )
            };
            if result == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            if result != ERROR_SUCCESS {
                return Err(error(result));
            }
            let key = Key(key);
            let result = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            if result != ERROR_SUCCESS && result != ERROR_FILE_NOT_FOUND {
                return Err(error(result));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn command_quotes_spaces_and_unicode_and_rejects_invalid_paths() {
            let value = command(Path::new(r"C:\My Apps\کیlux\keylux.exe")).unwrap();
            assert_eq!(
                String::from_utf16(&value[..value.len() - 1]).unwrap(),
                "\"C:\\My Apps\\کیlux\\keylux.exe\""
            );
            assert_eq!(value.last(), Some(&0));
            assert!(command(Path::new("keylux.exe")).is_err());
            assert!(command(Path::new(&format!("C:\\{}\\keylux.exe", "a".repeat(260)))).is_err());
        }

        #[test]
        fn registration_round_trips_and_disabling_preserves_other_values() {
            // An isolated ordinary registry key, never the user's real Run key.
            let path = format!(
                r"Software\keylux\Tests\startup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            struct Cleanup(String);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    unsafe {
                        RegDeleteKeyW(HKEY_CURRENT_USER, wide(&self.0).as_ptr());
                    }
                }
            }
            let _cleanup = Cleanup(path.clone());
            assert_eq!(read(&path).unwrap(), None);
            write(&path, None).unwrap();
            let value = command(Path::new(r"C:\My Apps\keylux.exe")).unwrap();
            write(&path, Some(&value)).unwrap();
            assert_eq!(read(&path).unwrap().unwrap(), value[..value.len() - 1]);
            let key = open(&path, true).unwrap().unwrap();
            let other = wide("Another application");
            assert_eq!(
                unsafe {
                    RegSetValueExW(
                        key.0,
                        other.as_ptr(),
                        0,
                        REG_SZ,
                        value.as_ptr().cast(),
                        (value.len() * 2) as u32,
                    )
                },
                ERROR_SUCCESS
            );
            write(&path, None).unwrap();
            write(&path, None).unwrap();
            assert_eq!(read(&path).unwrap(), None);
            let query_key = open(&path, false).unwrap().unwrap();
            let mut size = 0;
            assert_eq!(
                unsafe {
                    RegQueryValueExW(
                        query_key.0,
                        other.as_ptr(),
                        ptr::null(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                        &mut size,
                    )
                },
                ERROR_SUCCESS
            );
        }
    }
}
