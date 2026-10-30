/// The app icon as raw RGBA, drawn in code so the repo carries no binary asset
/// and the icon cannot go missing at runtime: a dark rounded board with rows of
/// lit keys sweeping through the spectrum. Shared by the tray, window, and executable so
/// the taskbar shows the same mark as the notification area.
pub fn icon_rgba() -> (Vec<u8>, u32, u32) {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];

    let put = |rgba: &mut [u8], x: usize, y: usize, c: [u8; 4]| {
        let i = (y * N + x) * 4;
        rgba[i..i + 4].copy_from_slice(&c);
    };

    // Body: rounded rectangle, corners clipped by a radius test.
    let (x0, y0, x1, y1) = (2usize, 6usize, 29usize, 25usize);
    let r = 4.0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = ((x0 as f32 + r) - x as f32)
                .max(x as f32 - (x1 as f32 - r))
                .max(0.0);
            let dy = ((y0 as f32 + r) - y as f32)
                .max(y as f32 - (y1 as f32 - r))
                .max(0.0);
            if dx * dx + dy * dy <= r * r {
                put(&mut rgba, x, y, [28, 30, 36, 255]);
            }
        }
    }

    // Keys: three rows, hue sweeping left to right so the icon reads as RGB
    // even at 16x16 on a crowded taskbar.
    for (row, y) in [10usize, 15, 20].into_iter().enumerate() {
        for (col, x) in (5..=25).step_by(5).enumerate() {
            let hue = (col as f32 * 0.16 + row as f32 * 0.06) % 1.0;
            let c = hsv_bytes(hue);
            for dy in 0..3 {
                for dx in 0..4 {
                    put(&mut rgba, x + dx, y + dy, [c[0], c[1], c[2], 255]);
                }
            }
        }
    }

    (rgba, N as u32, N as u32)
}

fn hsv_bytes(h: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (q, t) = (1.0 - f, f);
    let (r, g, b) = match i as i32 % 6 {
        0 => (1.0, t, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, t),
        3 => (0.0, q, 1.0),
        4 => (t, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hues_span_the_wheel() {
        assert_eq!(hsv_bytes(0.0), [255, 0, 0]);
        assert_eq!(hsv_bytes(1.0 / 3.0)[1], 255, "a third of the way is green");
    }
}
