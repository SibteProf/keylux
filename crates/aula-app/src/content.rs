//! Installed builds keep editable content in the user's settings directory.
use std::path::{Path, PathBuf};

pub fn installed_effects_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundled = exe.parent()?;
    if !bundled.join("installed.txt").is_file() {
        return None;
    }
    let base = crate::settings::Settings::path()?.parent()?.join("content");
    let effects = base.join("effects");
    let _ = seed_missing(&bundled.join("effects"), &effects);
    let _ = std::fs::create_dir_all(base.join("animations"));
    Some(effects)
}

fn seed_missing(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let target = destination.join(entry.file_name());
        // Exclusive creation preserves user edits, including during upgrades.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
        {
            Ok(mut file) => {
                std::io::copy(&mut std::fs::File::open(entry.path())?, &mut file)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_add_missing_effects_and_preserve_user_edits() {
        let base = std::env::temp_dir().join(format!(
            "keylux-content-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = base.join("bundled");
        let target = base.join("user");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("breathe.rhai"), "bundled").unwrap();
        seed_missing(&source, &target).unwrap();
        std::fs::write(target.join("breathe.rhai"), "user edits").unwrap();
        std::fs::write(source.join("new.rhai"), "new effect").unwrap();
        seed_missing(&source, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("breathe.rhai")).unwrap(),
            "user edits"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("new.rhai")).unwrap(),
            "new effect"
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
