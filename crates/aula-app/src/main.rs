//! `keylux` — desktop control for AULA keyboard lighting.
//!
//! Runs the GUI by default. Any argument drops to the CLI, which is handy for
//! scripting and for hardware checks without a window.

mod cli;
mod editor;
mod engine;
mod ui;

fn effects_dir() -> std::path::PathBuf {
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

fn main() -> anyhow::Result<()> {
    if std::env::args().len() > 1 {
        return cli::run();
    }

    let dir = effects_dir();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([880.0, 620.0])
            .with_min_inner_size([640.0, 480.0])
            .with_title("keylux"),
        ..Default::default()
    };

    eframe::run_native(
        "keylux",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, dir)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
