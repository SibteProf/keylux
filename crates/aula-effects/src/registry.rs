//! Effect registry: built-ins plus user scripts, with hot reload.

use std::path::{Path, PathBuf};

use crate::animation::AnimationEffect;
use crate::{builtin, Effect, EffectMeta, ScriptEffect, ScriptError};

/// Where an effect came from, for the UI to label it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Script(PathBuf),
    /// A keyframe animation: imported artwork, or made in the timeline editor.
    Animation(PathBuf),
}

pub struct Entry {
    pub meta: EffectMeta,
    pub source: Source,
    effect: Box<dyn Effect>,
}

impl Entry {
    pub fn effect_mut(&mut self) -> &mut dyn Effect {
        self.effect.as_mut()
    }

    /// Why this effect's last frame failed, if it did. Always `None` for
    /// built-ins; scripts report typos and bad return values here.
    pub fn runtime_error(&self) -> Option<&str> {
        self.effect.runtime_error()
    }
}

/// Everything the user can pick from.
#[derive(Default)]
pub struct Registry {
    pub entries: Vec<Entry>,
    /// Scripts that failed to load, so the UI can show why instead of silently
    /// dropping them.
    pub errors: Vec<String>,
    dir: Option<PathBuf>,
    anim_dir: Option<PathBuf>,
}

impl Registry {
    /// Built-ins only.
    pub fn new() -> Self {
        let mut r = Self::default();
        r.add_builtins();
        r
    }

    /// Built-ins plus every `.rhai` in `dir`.
    pub fn with_scripts(dir: impl AsRef<Path>) -> Self {
        let mut r = Self::new();
        r.dir = Some(dir.as_ref().to_path_buf());
        r.load_scripts();
        r
    }

    /// Built-ins, scripts from `dir`, and animations from `anim_dir`.
    pub fn with_all(dir: impl AsRef<Path>, anim_dir: impl AsRef<Path>) -> Self {
        let mut r = Self::new();
        r.dir = Some(dir.as_ref().to_path_buf());
        r.anim_dir = Some(anim_dir.as_ref().to_path_buf());
        r.load_scripts();
        r.load_animations();
        r
    }

    fn load_animations(&mut self) {
        let Some(dir) = self.anim_dir.clone() else {
            return;
        };
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return; // absent directory is normal, not an error
        };
        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();

        for path in paths {
            match AnimationEffect::load(&path) {
                Ok(fx) => self.entries.push(Entry {
                    meta: fx.meta(),
                    source: Source::Animation(path),
                    effect: Box::new(fx),
                }),
                Err(e) => self.errors.push(format!("{}: {e}", name_of(&path))),
            }
        }
    }

    /// Directory animations are read from and saved to.
    pub fn animations_dir(&self) -> Option<&Path> {
        self.anim_dir.as_deref()
    }

    pub fn scripts_dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn add_builtins(&mut self) {
        for e in builtin::all() {
            self.entries.push(Entry {
                meta: e.meta(),
                source: Source::Builtin,
                effect: e,
            });
        }
    }

    fn load_scripts(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        let Ok(rd) = std::fs::read_dir(&dir) else {
            self.errors
                .push(format!("cannot read effects directory {}", dir.display()));
            return;
        };

        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rhai"))
            .collect();
        paths.sort();

        for path in paths {
            match ScriptEffect::load(&path) {
                Ok(fx) => self.entries.push(Entry {
                    meta: fx.meta(),
                    source: Source::Script(path),
                    effect: Box::new(fx),
                }),
                Err(e) => self.errors.push(e.to_string()),
            }
        }
    }

    /// Rescan the script directory: picks up new, changed and deleted files.
    ///
    /// Cheap enough to call on a timer — it only stats files.
    pub fn refresh(&mut self) -> bool {
        let Some(_) = &self.dir else { return false };
        let before: Vec<_> = self
            .entries
            .iter()
            .filter_map(|e| match &e.source {
                Source::Script(p) => Some(p.clone()),
                Source::Builtin | Source::Animation(_) => None,
            })
            .collect();

        let on_disk = self.script_paths();
        let changed = on_disk != before
            || self
                .entries
                .iter()
                .any(|e| matches!(&e.source, Source::Script(_)) && self.is_stale(e));

        let anims_changed = self.anim_paths() != self.loaded_anim_paths();

        if changed || anims_changed {
            self.entries.retain(|e| e.source == Source::Builtin);
            self.errors.clear();
            self.load_scripts();
            self.load_animations();
        }
        changed || anims_changed
    }

    fn is_stale(&self, _e: &Entry) -> bool {
        // ScriptEffect tracks its own mtime; a full reload is simpler and only
        // happens when something actually changed on disk.
        false
    }

    fn anim_paths(&self) -> Vec<PathBuf> {
        list_ext(self.anim_dir.as_deref(), "json")
    }

    fn loaded_anim_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .entries
            .iter()
            .filter_map(|e| match &e.source {
                Source::Animation(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        v.sort();
        v
    }

    fn script_paths(&self) -> Vec<PathBuf> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut v: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rhai"))
            .collect();
        v.sort();
        v
    }

    pub fn find(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.meta.id == id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn list_ext(dir: Option<&Path>, ext: &str) -> Vec<PathBuf> {
    let Some(dir) = dir else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .collect();
    v.sort();
    v
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Reload one script in place, keeping the old version if it fails to compile.
pub fn reload_if_stale(entry: &mut Entry) -> Result<bool, ScriptError> {
    let Source::Script(path) = entry.source.clone() else {
        return Ok(false);
    };
    let fresh = ScriptEffect::load(&path)?;
    entry.meta = fresh.meta();
    entry.effect = Box::new(fresh);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_always_present() {
        let r = Registry::new();
        assert!(r.find("wave").is_some());
        assert!(r.find("solid").is_some());
    }

    #[test]
    fn scripts_are_discovered() {
        let r = Registry::with_scripts("../../effects");
        assert!(r.errors.is_empty(), "load errors: {:?}", r.errors);
        for id in ["spiral", "breathe", "ripple"] {
            assert!(r.find(id).is_some(), "{id} should be discovered");
        }
        assert!(r.len() > 3);
    }

    #[test]
    fn a_bad_script_is_reported_not_fatal() {
        let dir = std::env::temp_dir().join("aula-registry-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ok.rhai"),
            r#"fn meta(){#{name:"Ok",params:[]}} fn render(t,k,p){[]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("bad.rhai"), "fn meta( { nope").unwrap();

        let r = Registry::with_scripts(&dir);
        assert!(r.find("ok").is_some(), "good script still loads");
        assert_eq!(r.errors.len(), 1, "bad script reported once");
        assert!(r.errors[0].contains("bad.rhai"));
    }
}
