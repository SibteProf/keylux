//! Hardware smoke test, and the tool for identifying a 2.4 GHz receiver.
//!
//! ```text
//! cargo run --example smoke -- scan             # devices that speak the protocol
//! cargo run --example smoke -- scan --deep      # + unrecognised keyboard-shaped hardware
//! cargo run --example smoke -- scan --deep --all  # + anything with a vendor collection
//! cargo run --example smoke -- info             # list HID collections, pick the RGB one
//! cargo run --example smoke -- ping             # read the config block; writes nothing
//! cargo run --example smoke -- red              # whole board red (static path)
//! cargo run --example smoke -- wave 20          # streamed rainbow wave for 20s
//! cargo run --example smoke -- layout           # print the LED map
//! cargo run --example smoke -- dry              # print packet headers, no device needed
//!
//! cargo run --example smoke -- dongle-info      # receivers present
//! cargo run --example smoke -- dongle-rows      # one colour per row: an LED-map test
//! cargo run --example smoke -- dongle-red 10    # solid, held, to see whether it sticks
//! cargo run --example smoke -- dongle-wave 20   # streamed wave over the receiver
//! ```
//!
//! Every command except `layout` and `dry` accepts `--device vid:pid` to target
//! one board, and `--allow vid:pid` to treat an id as known (the equivalent of
//! the app's remembered devices). `wave` takes `--fps`.
//!
//! `dongle-wave` additionally takes `--fps`, `--hues` and `--gap`, which
//! override the driver's tuning for one run. They exist because the receiver's
//! limits had to be found by bisection on hardware and will have to be found
//! again on any other board — the numbers in `dongle::protocol` came from
//! exactly these flags. `--gap` below the vendor's 13 ms is the risky one; see
//! the warning on `set_packet_gap_ms`.

use std::time::{Duration, Instant};

use aula_protocol::device::RgbDevice;
use aula_protocol::f75::{keymap, protocol as p, F75};
use aula_protocol::transport::ProbeResult;
use aula_protocol::{dongle, ChannelOrder, DeviceId, Frame, Keyboard, Rgb, ScanOptions};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("scan");
    let opts = scan_options(&args);

    match cmd {
        "scan" => scan(&opts)?,
        "info" => info(&opts)?,
        "ping" => ping(&opts, device_arg(&args)?)?,
        "layout" => layout(),
        "dry" => dry_run(),
        "red" => red(&opts, device_arg(&args)?)?,
        "dongle-info" => dongle_info(&opts.allow)?,
        "dongle-red" => dongle_solid(&opts.allow, Rgb::new(255, 0, 0), hold(&args))?,
        "dongle-green" => dongle_solid(&opts.allow, Rgb::new(0, 255, 0), hold(&args))?,
        "dongle-blue" => dongle_solid(&opts.allow, Rgb::new(0, 0, 255), hold(&args))?,
        "dongle-off" => dongle_solid(&opts.allow, Rgb::BLACK, hold(&args))?,
        "dongle-rows" => dongle_rows(&opts.allow, hold(&args))?,
        "dongle-wave" => {
            let secs: f32 = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(15.0);
            dongle_wave(
                &opts.allow,
                secs,
                flag_value(&args, "--gap"),
                flag_value(&args, "--hues"),
                flag_value(&args, "--fps"),
            )?
        }
        "wave" => {
            let secs: f32 = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(15.0);
            wave(&opts, device_arg(&args)?, flag_value(&args, "--fps"), secs)?
        }
        other => {
            eprintln!(
                "unknown command {other:?}; use scan | info | ping | layout | dry | red | wave | \n                 dongle-info | dongle-red | dongle-green | dongle-off | dongle-wave"
            );
            std::process::exit(1);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// argument plumbing
// ---------------------------------------------------------------------------

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(String::as_str)
}

/// Seconds to hold a solid colour; default long enough to outlast a firmware
/// repaint.
fn hold(args: &[String]) -> f32 {
    args.get(1)
        .filter(|a| !a.starts_with("--"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(8.0)
}

fn device_arg(args: &[String]) -> anyhow::Result<Option<DeviceId>> {
    match flag_value(args, "--device") {
        Some(s) => Ok(Some(s.parse::<DeviceId>()?)),
        None => Ok(None),
    }
}

/// `--allow` mirrors the app's remembered-devices list, so a receiver can be
/// driven from the CLI before it has ever been committed to `protocol::KNOWN`.
fn scan_options(args: &[String]) -> ScanOptions {
    let allow = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--allow")
        .filter_map(|(i, _)| args.get(i + 1))
        .filter_map(|s| s.parse::<DeviceId>().ok())
        .collect();
    ScanOptions {
        allow,
        deep: args.iter().any(|a| a == "--deep"),
        include_non_keyboards: args.iter().any(|a| a == "--all"),
    }
}

/// Open whichever keyboard is asked for, on either link.
fn open(opts: &ScanOptions, id: Option<DeviceId>) -> anyhow::Result<Keyboard> {
    Ok(match id {
        Some(id) => Keyboard::open_id(id, opts)?,
        None => Keyboard::open_best(opts)?,
    })
}

// ---------------------------------------------------------------------------
// scan — how you find out which receiver is yours
// ---------------------------------------------------------------------------

/// List everything that answers the protocol, and show the near-misses too.
///
/// The near-misses matter: "opened, but refused the feature report" and "would
/// not open at all" are different problems, and collapsing them into a bare
/// "not found" is what makes this kind of hunt take an afternoon.
fn scan(opts: &ScanOptions) -> anyhow::Result<()> {
    if opts.deep {
        println!("Deep scan: probing unrecognised hardware as well as known devices.");
        println!("This is read-only — it opens vendor collections and reads the config block.\n");
    } else {
        println!("Scanning known and allow-listed devices. Add --deep to look wider.\n");
    }

    // The app-level view: both links, since a receiver is never found by the
    // feature probe the per-collection listing below shows.
    let found = Keyboard::discover(opts)?;
    let probes = F75::probe(opts)?;

    let mut ids: Vec<DeviceId> = probes.iter().map(|c| c.info.id).collect();
    ids.dedup();

    for id in ids {
        let cols: Vec<_> = probes.iter().filter(|c| c.info.id == id).collect();
        let name = cols
            .iter()
            .map(|c| c.info.product.as_str())
            .find(|s| !s.is_empty())
            .unwrap_or("(no product string)");
        let known = p::KNOWN.iter().any(|(k, _)| *k == id);
        println!(
            "{id}  {name:?}  {}",
            if known { "[known]" } else { "[scanned]" }
        );

        for c in &cols {
            let verdict = match &c.result {
                ProbeResult::Answered { confirmed: true } => {
                    "ANSWERED — config block valid (5A A5), this is an AULA board".to_string()
                }
                // Not a colour pipe. The receiver's status collection looks
                // exactly like this, and so does the keyboard before it has
                // been asked anything — which is why width is not the test.
                ProbeResult::Answered { confirmed: false } => {
                    "took the report id, but the config block is not valid — not the RGB channel"
                        .to_string()
                }
                ProbeResult::Silent => {
                    format!("does not take report {:#04x}", p::REPORT_ID)
                }
                ProbeResult::OsOwned => "skipped — the OS owns this collection".to_string(),
                ProbeResult::Unopenable(e) => format!("could not open: {e}"),
            };
            println!(
                "  usage_page={:#06x} usage={:#06x} iface={:<3} {verdict}",
                c.info.usage_page, c.info.usage, c.info.interface
            );
        }
        println!();
    }

    if found.is_empty() {
        println!("Nothing that can carry this protocol was found.");
        if !opts.deep {
            println!("Try: cargo run --example smoke -- scan --deep");
        } else {
            println!(
                "If the keyboard is on its receiver, note that a receiver exposing only narrow\n\
                 vendor collections is not hiding the protocol — it does not carry it. Working\n\
                 out what it does carry needs a USBPcap capture of the vendor driver; see\n\
                 \"How a second device gets added\" in docs/PROTOCOL.md."
            );
        }
        return Ok(());
    }

    println!("{} device(s) can carry the protocol:", found.len());
    for c in &found {
        println!("  {}  {}", c.id, c.label());
    }
    println!(
        "\nTo work out which one is your receiver: unplug it (leave the cable in), scan\n\
         again, and see which entry disappears."
    );
    Ok(())
}

fn info(opts: &ScanOptions) -> anyhow::Result<()> {
    println!("Known device ids:");
    for (id, link) in p::KNOWN {
        println!("  {id}  {}", link.suffix());
    }
    for id in dongle::KNOWN {
        println!("  {id}  2.4 GHz receiver");
    }
    for id in &opts.allow {
        println!("  {id}  allow-listed for this run");
    }
    println!();

    match Keyboard::open_best(opts) {
        Ok(kb) => {
            println!("Opened {} at {}", kb.name(), kb.hid_path());
            println!(
                "  {} leds, {} keys mapped, max {} FPS",
                kb.led_count(),
                kb.layout().len(),
                kb.max_fps()
            );
            // Only the wired board has a config block to be in a mode at all.
            if let Keyboard::Wired(w) = &kb {
                println!("  per-key mode active: {}", w.is_per_key_mode()?);
            }
            println!("  mode changes allowed: {}", kb.can_change_mode());
        }
        Err(e) => println!("could not open a keyboard: {e}"),
    }
    Ok(())
}

/// The safest possible identification: one config read, no writes at all.
///
/// Wired only, because there is nothing to read on the other link: the receiver
/// protocol has no config block and no readback of any kind.
fn ping(opts: &ScanOptions, id: Option<DeviceId>) -> anyhow::Result<()> {
    let kb = match id {
        Some(id) => F75::open_id(id, opts, ChannelOrder::Rgb)?,
        None => F75::open_best(opts, ChannelOrder::Rgb)?,
    };
    println!("{} at {}", kb.name(), kb.hid_path());
    let cfg = kb.read_config()?;
    println!("config block ({} bytes):", cfg.len());
    for (i, chunk) in cfg.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        println!("  {:03}  {}", i * 16, hex.join(" "));
    }
    println!(
        "\nsignature ok, mode bytes [{}]={:#04x} [{}]={:#04x} [{}]={:#04x} — per-key: {}",
        p::mode::OFF_A,
        cfg[p::mode::OFF_A],
        p::mode::OFF_B,
        cfg[p::mode::OFF_B],
        p::mode::OFF_C,
        cfg[p::mode::OFF_C],
        kb.is_per_key_mode()?
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// 2.4 GHz receiver
// ---------------------------------------------------------------------------

/// List receivers, and say plainly when none is plugged in.
fn dongle_info(extra: &[DeviceId]) -> anyhow::Result<()> {
    println!("Known receivers:");
    for id in dongle::KNOWN {
        println!("  {id}");
    }
    let found = dongle::discover(extra)?;
    if found.is_empty() {
        println!("\nNo receiver lighting channel present.");
        println!("The receiver must be plugged in AND the keyboard switched to 2.4 GHz.");
        return Ok(());
    }
    for c in &found {
        println!("\n{} at {}", c.label(), c.path);
    }
    let kb = dongle::open(extra)?;
    println!(
        "  {} leds, {} keys mapped, max {} FPS (tick {} ms)",
        kb.led_count(),
        kb.layout().len(),
        kb.max_fps(),
        aula_protocol::dongle::protocol::TICK_MS
    );
    Ok(())
}

/// Whole board one colour, over the receiver, held for a while.
///
/// The hold matters: a one-shot paint cannot show whether the colour STICKS.
/// The firmware repaints its own stored lighting over the top a second or two
/// after the host goes quiet, so holding is the only way to see the heartbeat
/// doing its job.
fn dongle_solid(extra: &[DeviceId], color: Rgb, seconds: f32) -> anyhow::Result<()> {
    let mut kb = dongle::open(extra)?;
    println!("Using {} ({})", kb.name(), kb.hid_path());
    // Without this the board stays in whatever firmware effect it was left in,
    // and repaints over every colour we send a second later.
    if kb.ensure_per_key_mode()? {
        println!("Switched the board into per-key mode.");
    }
    let frame = Frame::solid(kb.led_count(), color);
    kb.set_static(&frame)?;
    println!(
        "Painted every mapped key rgb({}, {}, {}). Holding {seconds}s.",
        color.r, color.g, color.b
    );
    let start = Instant::now();
    while start.elapsed().as_secs_f32() < seconds {
        kb.stream(&frame)?;
    }
    println!("Did it stay that colour the whole time, or snap back?");
    Ok(())
}

/// Light each physical ROW a different colour, over the receiver.
///
/// A mapping test, and the reason it exists: a solid colour proves the protocol
/// works but says NOTHING about whether LED index N is the key we think it is.
/// Only a spatial pattern can show that. If the index map is right this paints
/// six clean horizontal stripes; if it is wrong it paints confetti.
fn dongle_rows(extra: &[DeviceId], seconds: f32) -> anyhow::Result<()> {
    let mut kb = dongle::open(extra)?;
    println!("Using {} ({})", kb.name(), kb.hid_path());

    let keys = kb.layout().to_vec();
    let palette = [
        ("red", Rgb::new(255, 0, 0)),
        ("green", Rgb::new(0, 255, 0)),
        ("blue", Rgb::new(0, 0, 255)),
        ("yellow", Rgb::new(255, 255, 0)),
        ("magenta", Rgb::new(255, 0, 255)),
        ("cyan", Rgb::new(0, 255, 255)),
    ];

    let mut frame = Frame::black(kb.led_count());
    for k in &keys {
        frame.set(k.led, palette[usize::from(k.row) % palette.len()].1);
    }
    kb.set_static(&frame)?;

    println!("\nExpected, top row downwards:");
    for (i, (name, _)) in palette.iter().enumerate() {
        let n = keys.iter().filter(|k| usize::from(k.row) == i).count();
        if n > 0 {
            println!("  row {i}: {name}  ({n} keys)");
        }
    }
    println!(
        "\nSix clean stripes means the LED index map is right on this link.\n\
         Scattered colours mean it is not, and every effect will look like noise."
    );

    let start = Instant::now();
    while start.elapsed().as_secs_f32() < seconds {
        kb.stream(&frame)?;
    }
    Ok(())
}

/// Streamed wave over the receiver, the delta path under real load.
fn dongle_wave(
    extra: &[DeviceId],
    seconds: f32,
    gap: Option<&str>,
    hues: Option<&str>,
    fps: Option<&str>,
) -> anyhow::Result<()> {
    let mut kb = dongle::open(extra)?;
    println!("Using {} ({})", kb.name(), kb.hid_path());
    if let Some(f) = fps.and_then(|s| s.parse::<u32>().ok()) {
        kb.set_max_fps(f);
        println!("Frame rate capped to {f} FPS.");
    }
    if let Some(h) = hues.and_then(|s| s.parse::<f32>().ok()) {
        kb.set_hue_steps(h);
        println!("Hue steps forced to {h}.");
    }
    if let Some(ms) = gap.and_then(|s| s.parse::<u64>().ok()) {
        // Below the vendor's 13 ms is uncharted. The failure is dropped and
        // doubled keypresses while the lighting still looks perfect, and it
        // clears on a replug.
        kb.set_packet_gap_ms(ms);
        println!("Inter-chunk gap forced to {ms} ms. TYPE while this runs.");
    }
    if kb.ensure_per_key_mode()? {
        println!("Switched the board into per-key mode.");
    }

    let keys = kb.layout().to_vec();
    let max_x = keymap::max_x(&keys).max(1.0);
    let max_row = f32::from(keymap::max_row(&keys)).max(1.0);
    let fps = kb.max_fps();
    println!("Streaming deltas at up to {fps} FPS for {seconds}s. Ctrl+C to stop.");

    let start = Instant::now();
    let mut frames = 0u32;
    while start.elapsed().as_secs_f32() < seconds {
        let t = start.elapsed().as_secs_f32();
        let mut frame = Frame::black(kb.led_count());
        for k in &keys {
            let nx = k.x / max_x;
            let ny = f32::from(k.row) / max_row;
            // A SMOOTH gradient on purpose. Banding belongs in the driver, which
            // owns the trade between colour count and frame size; doing it here
            // too just hides whether that dial works at all.
            let hue = nx * 1.2 + ny * 0.25 - t * 0.35;
            frame.set(k.led, Rgb::from_hsv(hue, 1.0, 1.0));
        }
        kb.stream(&frame)?;
        frames += 1;
    }
    let secs = start.elapsed().as_secs_f32();
    println!(
        "Stopped. {frames} frames in {secs:.1}s = {:.1} FPS.",
        frames as f32 / secs
    );
    println!("Check: does the keyboard still type? (starvation would break it)");
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

fn red(opts: &ScanOptions, id: Option<DeviceId>) -> anyhow::Result<()> {
    let mut kb = open(opts, id)?;
    println!("Using {} ({})", kb.name(), kb.hid_path());
    if kb.ensure_per_key_mode()? {
        println!("Switched the board into per-key mode.");
    }
    let frame = Frame::solid(kb.led_count(), Rgb::new(255, 0, 0));
    kb.set_static(&frame)?;
    println!("Wrote {} slots = full red (static path).", kb.led_count());
    println!("Did the keyboard actually turn red? On an unverified device that is the\nonly thing that proves the colour path works.");
    Ok(())
}

fn wave(
    opts: &ScanOptions,
    id: Option<DeviceId>,
    fps_arg: Option<&str>,
    seconds: f32,
) -> anyhow::Result<()> {
    let mut kb = open(opts, id)?;
    println!("Using {} ({})", kb.name(), kb.hid_path());
    // Once, before streaming. Never inside the loop.
    if kb.ensure_per_key_mode()? {
        println!("Switched the board into per-key mode.");
    }
    if let Some(f) = fps_arg.and_then(|s| s.parse::<u32>().ok()) {
        kb.set_max_fps(f);
    }

    let keys = kb.layout().to_vec();
    let max_x = keymap::max_x(&keys).max(1.0);
    let max_row = f32::from(keymap::max_row(&keys)).max(1.0);
    let fps = fps_arg
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or_else(|| kb.max_fps())
        .clamp(1, kb.max_fps());
    let period = Duration::from_secs_f32(1.0 / fps as f32);

    // Ask the device for its own ceiling rather than reading a constant out of
    // the wired module: the two links are paced by different drivers.
    println!(
        "Link: {} — {} FPS ceiling ({} ms). Streaming at {fps} FPS for {seconds}s.",
        kb.link().suffix(),
        kb.max_fps(),
        1000 / u64::from(kb.max_fps().max(1))
    );
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
