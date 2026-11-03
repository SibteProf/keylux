//! Distribution assets and user content must work outside the source tree.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "keylux-assets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("effects")).unwrap();
        std::fs::write(
            path.join("effects/breathe.rhai"),
            include_str!("../../../effects/breathe.rhai"),
        )
        .unwrap();
        Self(path)
    }

    fn list(&self) -> Command {
        let binary = std::env::var_os("KEYLUX_TEST_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_keylux").into());
        let mut command = Command::new(binary);
        command
            .arg("list")
            .current_dir(&self.0)
            .env("HOME", &self.0)
            .env_remove("XDG_DATA_HOME")
            .env_remove("KEYLUX_EFFECTS_DIR")
            .env_remove("KEYLUX_ANIMATIONS_DIR");
        command
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn animation(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("path-check.json"),
        r##"{"name":"Asset path check","leds":1,"duration":1.0,"keyframes":[{"t":0.0,"colors":["#ff0000"]}]}"##,
    )
    .unwrap();
}

fn check(mut command: Command) {
    let result = command.output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("[script: breathe.rhai]"), "{output}");
    assert!(output.contains("[animation: path-check.json]"), "{output}");
}

#[test]
fn packaged_effects_use_xdg_animation_storage() {
    let sandbox = Sandbox::new();
    animation(&sandbox.0.join("data/keylux/animations"));
    let mut command = sandbox.list();
    command
        .env("KEYLUX_EFFECTS_DIR", sandbox.0.join("effects"))
        .env("XDG_DATA_HOME", sandbox.0.join("data"));
    check(command);
}

#[test]
fn packaged_effects_use_home_when_xdg_data_is_empty() {
    let sandbox = Sandbox::new();
    animation(&sandbox.0.join(".local/share/keylux/animations"));
    let mut command = sandbox.list();
    command
        .env("KEYLUX_EFFECTS_DIR", sandbox.0.join("effects"))
        .env("XDG_DATA_HOME", "");
    check(command);
}

#[test]
fn explicit_animation_directory_overrides_packaged_default() {
    let sandbox = Sandbox::new();
    animation(&sandbox.0.join("custom-animations"));
    let mut command = sandbox.list();
    command
        .env("KEYLUX_EFFECTS_DIR", sandbox.0.join("effects"))
        .env("KEYLUX_ANIMATIONS_DIR", sandbox.0.join("custom-animations"));
    check(command);
}
