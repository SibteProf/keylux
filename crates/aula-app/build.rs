#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("keylux.ico");
    let (rgba, width, height) = icon::icon_rgba();
    image::RgbaImage::from_raw(width, height, rgba)
        .expect("valid app icon dimensions")
        .save_with_format(&path, image::ImageFormat::Ico)
        .expect("write Windows icon");
    winresource::WindowsResource::new()
        .set_icon(path.to_str().expect("UTF-8 icon path"))
        .compile()
        .expect("embed Windows executable icon");
}
