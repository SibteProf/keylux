#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

//! `keylux` — desktop keyboard lighting through native AULA and OpenRGB drivers.
//!
//! Runs the GUI by default. Any argument drops to the CLI, which is handy for
//! scripting and for hardware checks without a window.

mod backend;
mod board;
mod calibration;
mod cli;
mod content;
mod editor;
mod engine;
mod icon;
mod profiles;
mod settings;
mod startup;
mod theme;
mod tray;
mod ui;
mod window_ctl;

fn effects_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("KEYLUX_EFFECTS_DIR").filter(|s| !s.is_empty()) {
        return dir.into();
    }
    if let Some(dir) = content::installed_effects_dir() {
        return dir;
    }
    // Alongside the executable when installed, or the repo's effects/ in dev.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("effects");
            if p.is_dir() {
                return p;
            }
        }
    }
    for candidate in ["effects", "../effects", "../../effects"] {
        let p = std::path::PathBuf::from(candidate);
        if p.is_dir() {
            return p;
        }
    }
    std::path::PathBuf::from("effects")
}

fn animations_dir(effects: &std::path::Path) -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("KEYLUX_ANIMATIONS_DIR").filter(|s| !s.is_empty()) {
        return dir.into();
    }
    // Distribution packages can supply effects from a read-only location.
    // Keep user-created content in writable XDG storage in that case.
    if std::env::var_os("KEYLUX_EFFECTS_DIR").is_some_and(|s| !s.is_empty()) {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|s| !s.is_empty())
                    .map(|h| std::path::PathBuf::from(h).join(".local/share"))
            });
        if let Some(data) = data {
            return data.join("keylux/animations");
        }
    }
    effects
        .parent()
        .map(|p| p.join("animations"))
        .unwrap_or_else(|| std::path::PathBuf::from("animations"))
}

fn main() -> anyhow::Result<()> {
    if std::env::args().len() > 1 {
        return cli::run();
    }

    let dir = effects_dir();
    let (rgba, w, h) = tray::icon_rgba();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([760.0, 560.0])
            .with_title("keylux")
            // The taskbar/title-bar icon, same mark as the tray, instead of
            // eframe's default "e".
            .with_icon(eframe::egui::IconData {
                rgba,
                width: w,
                height: h,
            }),
        ..Default::default()
    };

    #[cfg(target_os = "linux")]
    if let Err(e) = gtk::init() {
        eprintln!("Failed to initialize GTK: {e}");
        std::process::exit(1);
    }

    eframe::run_native(
        "keylux",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, dir)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
