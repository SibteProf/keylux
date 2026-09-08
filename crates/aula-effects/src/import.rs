//! Turn images and GIFs into keyframe animations.
//!
//! The keyboard is roughly a 16x6 grid of unevenly spaced keys, so an image is
//! not scaled — each key **samples the region of the image it physically
//! covers**. That keeps logos and pixel art recognisable despite the stagger,
//! and means a key's colour is the average of everything under it rather than
//! one arbitrary pixel.

use std::path::Path;

use aula_protocol::{KeyPos, Rgb};

use crate::animation::{Animation, HexColor, Keyframe};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("could not decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("no frames found in {0}")]
    Empty(String),
    #[error("unsupported file type: {0}")]
    Unsupported(String),
}

/// Map one RGBA image onto the key grid.
///
/// `leds` is the device's LED count; keys not in `layout` stay black.
pub fn image_to_keyframe(
    img: &image::RgbaImage,
    layout: &[KeyPos],
    leds: usize,
    t: f32,
) -> Keyframe {
    let mut colors = vec![HexColor::from(Rgb::BLACK); leds];

    // Board extent in key units, so the image maps to the whole keyboard.
    let board_w = layout
        .iter()
        .map(|k| k.x + k.w / 2.0)
        .fold(0.0f32, f32::max)
        .max(1.0);
    let board_h = layout.iter().map(|k| k.row).max().unwrap_or(0) as f32 + 1.0;

    let (iw, ih) = (img.width() as f32, img.height() as f32);

    for k in layout {
        if k.led >= leds {
            continue;
        }
        // The rectangle this key occupies, in image pixels.
        let x0 = ((k.x - k.w / 2.0) / board_w * iw).floor().max(0.0) as u32;
        let x1 = ((k.x + k.w / 2.0) / board_w * iw).ceil().min(iw) as u32;
        let y0 = (f32::from(k.row) / board_h * ih).floor().max(0.0) as u32;
        let y1 = ((f32::from(k.row) + 1.0) / board_h * ih).ceil().min(ih) as u32;

        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        for y in y0..y1.max(y0 + 1) {
            for x in x0..x1.max(x0 + 1) {
                if x >= img.width() || y >= img.height() {
                    continue;
                }
                let p = img.get_pixel(x, y).0;
                // Premultiply by alpha so transparent areas read as unlit.
                let a = p[3] as u64;
                r += p[0] as u64 * a / 255;
                g += p[1] as u64 * a / 255;
                b += p[2] as u64 * a / 255;
                n += 1;
            }
        }
        if let (Some(r), Some(g), Some(b)) = (r.checked_div(n), g.checked_div(n), b.checked_div(n))
        {
            colors[k.led] = HexColor::from(Rgb::new(r as u8, g as u8, b as u8));
        }
    }

    Keyframe { t, colors }
}

/// Import a still image as a one-frame animation.
pub fn import_image(
    path: impl AsRef<Path>,
    layout: &[KeyPos],
    leds: usize,
) -> Result<Animation, ImportError> {
    let path = path.as_ref();
    let img = image::open(path)?.to_rgba8();
    let name = stem(path);

    let mut anim = Animation::new(&name, leds);
    anim.description = format!("Imported from {}", file_name(path));
    anim.duration = 1.0;
    anim.interpolate = false;
    anim.keyframes = vec![image_to_keyframe(&img, layout, leds, 0.0)];
    anim.normalise();
    Ok(anim)
}

/// Import an animated GIF, one keyframe per GIF frame, honouring frame delays.
pub fn import_gif(
    path: impl AsRef<Path>,
    layout: &[KeyPos],
    leds: usize,
) -> Result<Animation, ImportError> {
    use image::AnimationDecoder;

    let path = path.as_ref();
    let file = std::fs::File::open(path)?;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file))?;

    let mut t = 0.0f32;
    let mut keyframes = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame?;
        let (num, den) = frame.delay().numer_denom_ms();
        let delay = if den == 0 {
            100.0
        } else {
            num as f32 / den as f32
        };
        let buf = frame.into_buffer();
        keyframes.push(image_to_keyframe(&buf, layout, leds, t));
        // GIFs commonly report 0 delay meaning "as fast as possible"; clamp to
        // something the keyboard can actually show.
        t += (delay / 1000.0).max(1.0 / 21.0);
    }

    if keyframes.is_empty() {
        return Err(ImportError::Empty(file_name(path)));
    }

    let mut anim = Animation::new(&stem(path), leds);
    anim.description = format!("Imported from {}", file_name(path));
    anim.duration = t.max(0.1);
    anim.interpolate = false; // GIFs are discrete frames, not tweens
    anim.keyframes = keyframes;
    anim.normalise();
    Ok(anim)
}

/// Import a folder of images, sorted by filename, as a frame sequence.
pub fn import_sequence(
    dir: impl AsRef<Path>,
    layout: &[KeyPos],
    leds: usize,
    fps: f32,
) -> Result<Animation, ImportError> {
    let dir = dir.as_ref();
    let mut paths: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_still(p))
        .collect();
    paths.sort();

    if paths.is_empty() {
        return Err(ImportError::Empty(file_name(dir)));
    }

    let step = 1.0 / fps.max(0.1);
    let mut keyframes = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let img = image::open(p)?.to_rgba8();
        keyframes.push(image_to_keyframe(&img, layout, leds, i as f32 * step));
    }

    let mut anim = Animation::new(&stem(dir), leds);
    anim.description = format!("{} frames from {}", keyframes.len(), file_name(dir));
    anim.duration = keyframes.len() as f32 * step;
    anim.interpolate = false;
    anim.keyframes = keyframes;
    anim.normalise();
    Ok(anim)
}

/// Import whatever the path points at: GIF, still image, or folder of stills.
pub fn import_any(
    path: impl AsRef<Path>,
    layout: &[KeyPos],
    leds: usize,
) -> Result<Animation, ImportError> {
    let path = path.as_ref();
    if path.is_dir() {
        return import_sequence(path, layout, leds, 10.0);
    }
    match ext(path).as_str() {
        "gif" => import_gif(path, layout, leds),
        e if is_still_ext(e) => import_image(path, layout, leds),
        other => Err(ImportError::Unsupported(other.to_string())),
    }
}

fn is_still(p: &Path) -> bool {
    p.is_file() && is_still_ext(&ext(p))
}

fn is_still_ext(e: &str) -> bool {
    matches!(e, "png" | "jpg" | "jpeg" | "bmp" | "webp" | "tga")
}

fn ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".into())
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aula_protocol::f75::keymap;

    #[test]
    fn a_solid_image_lights_every_key_the_same() {
        let layout = keymap::layout();
        let img = image::RgbaImage::from_pixel(32, 12, image::Rgba([0, 128, 255, 255]));
        let kf = image_to_keyframe(&img, &layout, 126, 0.0);

        for k in &layout {
            let c = kf.color(k.led);
            assert!(
                c.b > 200 && c.g > 100 && c.r < 40,
                "key {} got {c:?}",
                k.name
            );
        }
    }

    #[test]
    fn left_and_right_halves_map_to_the_right_keys() {
        let layout = keymap::layout();
        // Left half red, right half green.
        let mut img = image::RgbaImage::new(32, 12);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = if x < 16 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 255, 0, 255])
            };
        }
        let kf = image_to_keyframe(&img, &layout, 126, 0.0);

        let esc = layout.iter().find(|k| k.name == "Esc").unwrap();
        let end = layout.iter().find(|k| k.name == "End").unwrap();
        assert!(
            kf.color(esc.led).r > kf.color(esc.led).g,
            "Esc is on the left"
        );
        assert!(
            kf.color(end.led).g > kf.color(end.led).r,
            "End is on the right"
        );
    }

    #[test]
    fn transparency_reads_as_unlit() {
        let layout = keymap::layout();
        let img = image::RgbaImage::from_pixel(16, 8, image::Rgba([255, 255, 255, 0]));
        let kf = image_to_keyframe(&img, &layout, 126, 0.0);
        assert!(kf.color(layout[0].led).is_black());
    }

    #[test]
    fn unsupported_files_are_rejected_clearly() {
        let layout = keymap::layout();
        let p = std::env::temp_dir().join("aula-import-test.txt");
        std::fs::write(&p, "not an image").unwrap();
        let err = import_any(&p, &layout, 126).unwrap_err();
        assert!(matches!(err, ImportError::Unsupported(_)));
    }
}
