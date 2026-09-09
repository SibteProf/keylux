# keylux

Open-source per-key RGB control for the **AULA F75** mechanical keyboard,
written in Rust.

Per-key colour and smooth animation over USB, with no vendor software and no
firmware modification. Write your own effects as small scripts — no Rust
toolchain required.

> **Status: early but usable.** The protocol is reverse-engineered, documented
> and verified on hardware, and the app runs effects, scripts and animations.
> Only the F75 is supported so far; see [Roadmap](#roadmap).

## Why

The F75 is a popular budget board with no open-source lighting support. OpenRGB
has an [open, unimplemented request](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/4232)
for it, and the one published reverse-engineering write-up stops at the point
where colours refuse to render.

This project documents the full protocol — including the parts that are
genuinely surprising — and turns it into something usable.

## What works

- Per-key static colour (persists across reboots)
- Smooth animation at the hardware's real ceiling of ~21 FPS
- Correct LED map, derived from the keyboard's own key-matrix table
- Desktop app with a live keyboard preview and controls generated from each
  effect's own parameter declaration
- Built-in effects: solid, wave, sweep, bloom, scrolling text
- User effects as `.rhai` scripts, hot-reloaded from `effects/`
- Keyframe animations: paint them in the timeline editor, or import a GIF,
  image or folder of frames
- Runs in the system tray (Windows), so the lighting keeps going with the
  window closed

See [`docs/PROTOCOL.md`](docs/PROTOCOL.md) for the complete protocol.

## Build

Needs a Rust toolchain and a C linker (`hidapi` links against system HID
libraries).

- **Windows** — [rustup](https://rustup.rs) plus Visual Studio Build Tools with
  the C++ workload
- **Linux** — rustup, plus `libudev-dev` and `pkg-config`
- **macOS** — rustup and Xcode command line tools

```bash
cargo build --release
```

### Linux permissions

Opening the vendor HID interface needs a udev rule:

```bash
sudo cp packaging/99-aula.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## Try it

The keyboard must be in **wired USB-C mode** — the 2.4 GHz dongle and Bluetooth
do not expose the RGB interface.

```bash
cargo run --example smoke -- info      # find the device, show which collection is used
cargo run --example smoke -- red       # whole board red
cargo run --example smoke -- wave 20   # streamed rainbow wave for 20 seconds
cargo run --example smoke -- dry       # print packet headers, no hardware needed
```

## Layout

```
crates/
  aula-protocol/   HID transport, device trait, F75 driver
  aula-effects/    effect engine, parameters, built-ins, Rhai host
  aula-app/        desktop app
effects/           user-authored .rhai effect scripts
docs/PROTOCOL.md   the full protocol write-up
reference/         the original TypeScript prototype, kept as provenance
```

## Roadmap

- [x] Protocol reverse-engineered and documented
- [x] `aula-protocol`: transport, F75 driver, LED map
- [x] Hardware verification of the Rust port
- [x] Port remaining effects (sweep, bloom, scrolling text)
- [x] Rhai scripting host with hot reload
- [x] egui app: live keyboard preview, effect picker, auto-generated controls
- [x] Timeline editor and GIF/image import
- [x] System tray
- [ ] Packaged releases
- [ ] A second device behind the same `RgbDevice` trait

## Running in the tray

Closing the window asks whether to minimise to the tray or quit, and can
remember the answer. Minimising hides to the tray as well. Both, plus whether
the lighting keeps rendering while hidden, are under **Window** at the bottom of
the effects panel.

The tray icon's menu has *Show*, *Play / pause lighting* and *Quit*; a
double-click reopens the window.

**Hiding to the tray is Windows-only for now.** egui does not repaint a hidden
window, so reopening one has to go through the windowing system directly, and
only the Windows path is implemented. Elsewhere the app refuses to hide at all
and closing simply quits — better than a window nothing can bring back. The
same applies if the tray icon cannot be created.

Preferences live in `%APPDATA%\keylux\settings.json` (or
`~/.config/keylux/settings.json`), and can be edited or deleted by hand.

## Writing an effect

Effects are pure functions of time: given `t` and the keyboard's physical
layout, produce a frame. They **declare** their own parameters, and the UI
builds controls from that declaration — so a new script gets sliders and colour
pickers for free.

Drop a `.rhai` file into `effects/` and it appears in the list within a second,
sliders and all; edit it while the app runs and the change is picked up without
a restart. The shipped examples in that folder are the quickest way in, and the
Rust trait behind it is in
[`crates/aula-effects/src/lib.rs`](crates/aula-effects/src/lib.rs).

## Contributing

Adding another keyboard is very welcome — the process is documented at the end
of [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

Please read the safety rules in the protocol doc first. Some are not obvious
and one of them can brick a keyboard.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option. This is the Rust ecosystem convention: MIT is short and
permissive, and Apache-2.0 adds an explicit patent grant that MIT lacks.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
licence, shall be dual licensed as above, without any additional terms or
conditions.

### A note on OpenRGB

OpenRGB is GPLv2, and permissive licensing here does not get in its way. The
protocol itself is a set of facts about how the hardware behaves, documented
in [docs/PROTOCOL.md](docs/PROTOCOL.md) — anyone is free to implement it, and
an OpenRGB driver would be written in C++ from those notes rather than by
copying this Rust code. Contributions upstreaming F75 support to OpenRGB are
very welcome.
