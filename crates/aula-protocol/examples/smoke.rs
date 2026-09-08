//! Hardware smoke test — Phase 1 parity check against the TypeScript CLI.
//!
//! ```text
//! cargo run --example smoke -- info      # list HID collections, pick the RGB one
//! cargo run --example smoke -- red       # whole board red (static path)
//! cargo run --example smoke -- wave 20   # streamed rainbow wave for 20s
//! cargo run --example smoke -- layout    # print the LED map
//! cargo run --example smoke -- dry       # print packet headers, no device needed
//! ```

use std::time::{Duration, Instant};

use aula_protocol::device::RgbDevice;
use aula_protocol::f75::{keymap, protocol as p, F75};
use aula_protocol::{transport, Frame, Rgb};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("info");

    match cmd {
        "info" => info()?,
        "layout" => layout(),
        "dry" => dry_run(),
        "red" => red()?,
        "wave" => {
            let secs: f32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(15.0);
            wave(secs)?
        }
        other => {
            eprintln!("unknown command {other:?}; use info | layout | dry | red | wave");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn info() -> anyhow::Result<()> {
    println!(
        "Looking for AULA F75 at {:04x}:{:04x}\n",
        p::VENDOR_ID,
        p::PRODUCT_ID
    );
    let cols = transport::list_collections(p::VENDOR_ID, p::PRODUCT_ID)?;
    if cols.is_empty() {
        println!("NOT FOUND — the RGB interface only exists in wired USB-C mode.");
        return Ok(());
    }
    for c in &cols {
        println!(
            "  usage_page={:#06x} usage={:#06x} iface={}  {}",
            c.usage_page, c.usage, c.interface, c.product
        );
        println!("    {}", c.path);
    }

    match F75::open() {
        Ok(kb) => {
            println!("\nRGB interface selected by probe:\n  {}", kb.hid_path());
            println!(
                "  {} leds, {} keys mapped, max {} FPS",
                kb.led_count(),
                kb.layout().len(),
                kb.max_fps()
            );
            println!("  per-key mode active: {}", kb.is_per_key_mode()?);
        }
        Err(e) => println!("\ncould not open RGB interface: {e}"),
    }
    Ok(())
}

fn layout() {
    let keys = keymap::layout();
    println!("{} keys mapped\n", keys.len());
    for k in &keys {
        println!(
            "  {:<10} row={} x={:>5.2} w={:>4.2} led={}",
            k.name, k.row, k.x, k.w, k.led
        );
    }
}

/// Print packet headers without touching hardware, so the bytes can be diffed
/// against the TypeScript implementation's `--verbose` output.
fn dry_run() {
    let show = |label: &str, pkt: &[u8]| {
        println!(
            "{label:<14} cmd=0x{:02x} addr={:02x} {:02x} {:02x} {:02x} len=0x{:04x}",
            pkt[1],
            pkt[2],
            pkt[3],
            pkt[4],
            pkt[5],
            u16::from(pkt[6]) | (u16::from(pkt[7]) << 8)
        );
    };
    show(
        "read config",
        &p::build_packet(p::cmd::READ_CONFIG, p::ADDR, p::CONFIG_LEN as u16, None),
    );
    show(
        "static",
        &p::build_packet(p::cmd::WRITE_STATIC, p::ADDR, p::STATIC_LEN as u16, None),
    );
    show(
        "stream",
        &p::build_packet(p::cmd::STREAM, p::ADDR, p::STREAM_LEN as u16, None),
    );
    println!("\nexpected, matching the TS CLI:");
    println!("  read config    cmd=0x84 addr=00 00 01 00 len=0x0080");
    println!("  static         cmd=0x06 addr=00 00 01 00 len=0x0180");
    println!("  stream         cmd=0x08 addr=00 00 01 00 len=0x017a");
}

fn red() -> anyhow::Result<()> {
    let mut kb = F75::open()?;
    println!("Using {} ({})", kb.name(), kb.hid_path());
    if kb.ensure_per_key_mode()? {
        println!("Switched the board into per-key mode.");
    }
    let frame = Frame::solid(kb.led_count(), Rgb::new(255, 0, 0));
    kb.set_static(&frame)?;
    println!("Wrote {} slots = full red (static path).", kb.led_count());
    Ok(())
}

fn wave(seconds: f32) -> anyhow::Result<()> {
    let mut kb = F75::open()?;
    println!("Using {} ({})", kb.name(), kb.hid_path());
    // Once, before streaming. Never inside the loop.
    if kb.ensure_per_key_mode()? {
        println!("Switched the board into per-key mode.");
    }

    let keys = kb.layout().to_vec();
    let max_x = keymap::max_x(&keys).max(1.0);
    let max_row = f32::from(keymap::max_row(&keys)).max(1.0);
    let fps = kb.max_fps();
    let period = Duration::from_secs_f32(1.0 / fps as f32);

    println!("Streaming a wave at {fps} FPS for {seconds}s. Ctrl+C to stop.");
    let start = Instant::now();
    let mut frames = 0u32;

    while start.elapsed().as_secs_f32() < seconds {
        let t = start.elapsed().as_secs_f32();
        let mut frame = Frame::black(kb.led_count());
        for k in &keys {
            let nx = k.x / max_x;
            let ny = f32::from(k.row) / max_row;
            let hue = nx * 1.2 + ny * 0.25 - t * 0.35;
            frame.set(k.led, Rgb::from_hsv(hue, 1.0, 1.0));
        }
        kb.stream(&frame)?;
        frames += 1;

        // The driver paces itself; respect_gap() inside stream() enforces the
        // hardware minimum, this just avoids busy-spinning.
        let target = start + period * frames;
        if let Some(wait) = target.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }

    let secs = start.elapsed().as_secs_f32();
    println!(
        "Stopped. {frames} frames in {secs:.1}s = {:.1} FPS.",
        frames as f32 / secs
    );
    println!("Check: does the keyboard still type? (starvation would break it)");
    Ok(())
}
