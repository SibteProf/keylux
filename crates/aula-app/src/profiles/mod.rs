//! Per-application lighting profiles.
//!
//! A profile binds one application to one effect: while that application's
//! window is in the foreground (in focus), its effect is what the keyboard
//! renders. The binding is by `app_id`, which is whatever the platform reports as an
//! application's identity — `WM_CLASS` on Linux, the executable's basename on
//! Windows, the bundle identifier on macOS.
//!
//! Profiles are one JSON file each, in a `profiles/` directory beside
//! `animations/`. One file per profile rather than one file for all of them,
//! because a profile is the natural unit of sharing: send someone
//! `firefox.json` and they drop it in and it works. It also means a
//! hand-edit touches only that profile, and a corrupt one costs that profile
//! alone.
//!
//! The filename is derived from `app_id`, lower-cased and stripped of
//! anything a filesystem might object to — but the filename is only an
//! address. `app_id` is what matching reads, and it is stored in full inside
//! the file, so a name that had to be mangled to fit is still matched
//! exactly.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod window_focus;

/// The `app_id` reserved for the fallback profile.
///
/// A profile with this id is what runs when the foreground application has no
/// profile of its own. It is a normal profile in every other way — same file,
/// same fields — so the UI can show and edit it, and a user who wants "always
/// use this effect unless something else says otherwise" gets it by adding one
/// profile rather than by finding a hidden setting.
///
/// Reserved: the UI must refuse to let a profile for a real application take this
/// id, because it is the one id whose meaning is fixed rather than a name.
pub const DEFAULT_APP_ID: &str = "Default";

/// Whether the filesystem treats `Firefox.json` and `firefox.json` as
/// different files.
///
/// Linux is case-sensitive always: ext4, btrfs, xfs and tmpfs all keep the
/// two names apart. Everywhere else — Windows always, macOS on every default
/// APFS volume — the two names are the same file.
///
/// On Linux only the lower-case name is read: a mixed-case one would mean a
/// second profile claiming the same address, and a save that silently
/// overwrote one of them. On a case-insensitive filesystem there is nothing
/// to tell apart, so the check is skipped.
pub const CASE_SENSITIVE_FS: bool = cfg!(target_os = "linux");

/// One application bound to one effect
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Profile {
    /// How the user calls this profile.
    /// Not unique, not used for matching profile,
    /// just a label
    pub name: String,
    /// What the profile is bound to. The platform reports it,
    /// matching reads it, and it is unique across the store:
    /// one app_id, one profile
    pub app_id: String,
    /// The effect to render, by its registry id (not an index, but id)
    pub effect_id: String,
}

/// Every profile on disk
pub struct ProfilesStore {
    /// Directory the profiles were read from,
    /// and the one a save writes back
    pub dir: PathBuf,
    pub list: Vec<Profile>,
}

impl ProfilesStore {
    /// Read every profile in `dir`.
    ///
    /// Corrupted files and non-JSON files are skipped;
    /// Filenames are not checked against the `app_id` inside: the name is only
    /// an address, and `app_id` is what matching reads. This lets a user keep
    /// backups by renaming a file or changing its extension, without the store
    /// treating the copy as a profile.
    pub fn load(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir(&dir);
        let mut list: Vec<Profile> = Vec::new();

        let Ok(files) = std::fs::read_dir(&dir) else {
            return Self { dir, list };
        };

        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            };

            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut profile: Profile = match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(_) => {
                    continue;
                }
            };

            // On case-sensitive fylesystems, `Firefox.json` and `firefox.json`
            // are two different files, and only the lower-case one is read —
            // a stray rename or a manual copy cannot produce a second profile
            // for the same application. On Windows and macOS the filesystem
            // cannot tell the two names apart, so the name is read as if it were lower-case.
            if CASE_SENSITIVE_FS {
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if stem != stem.to_lowercase() {
                    continue;
                }
            }

            if profile.name.trim().is_empty() {
                profile.name = profile.app_id.clone();
            }

            list.push(profile);
        }

        Self { dir, list }
    }

    /// Write a profile file
    ///
    /// The filename comes from `app_id`, so the same applications always
    /// lands in the same file and a second `save()` overwrites rather than
    /// accumulating. When an existing profile's `app_id` has changed -
    /// the user rebound the profile to another application - the old file is
    /// removed first, or the store will grow an orphan that the list no
    /// longer knows about.
    ///
    /// `old_app_id` - the profile's `app_id` before this edit, or `None` if it is new.
    ///
    /// Returns the `Some(PathBuf)` path written, or `None` if nothing was saved.
    pub fn save(&mut self, old_app_id: Option<&str>, profile: Profile) -> Option<PathBuf> {
        let path = self.profile_path(&profile.app_id);

        match old_app_id {
            // Editing the existing profile, rebound to a different application
            Some(old) if !old.eq_ignore_ascii_case(&profile.app_id) => {
                let old_path = self.profile_path(old);
                let _ = std::fs::remove_file(&old_path);
                self.list.retain(|p| !p.app_id.eq_ignore_ascii_case(old));
                self.list.push(profile.clone());
            }

            _ => match self
                .list
                .iter_mut()
                .find(|p| p.app_id.eq_ignore_ascii_case(&profile.app_id))
            {
                Some(slot) => *slot = profile.clone(),
                None => self.list.push(profile.clone()),
            },
        }

        let json = serde_json::to_string_pretty(&profile).ok()?;
        std::fs::write(&path, json).ok()?;
        Some(path)
    }

    fn profile_path(&self, app_id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", app_id.to_lowercase()))
    }
}
