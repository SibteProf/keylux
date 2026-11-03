# keylux

**AULA F75 Wired `1a2c:9407`:** Keylux recognizes this reported variant, but
per-key lighting and animations are not supported yet. See
[the diagnostic guide](docs/F75-WIRED.md) and run
`keylux diagnose --device 1a2c:9407 --output f75-wired-diagnostics.txt`.

Open-source per-key RGB control for mechanical keyboards, written in Rust.
Native **AULA F75** support plus an **OpenRGB SDK backend** for Corsair, Razer,
ASUS, Logitech, HyperX, SteelSeries, and other compatible keyboards.

Per-key colour, animations, and scriptable effects. The F75 connects directly
over USB or its receiver. Other brands use a separately running OpenRGB SDK
server. No firmware modification is required for the listed targets.

> **Status:** the native F75 protocol is verified on hardware. The new OpenRGB
> backend has automated protocol and CLI integration tests; the additional
> models still need physical validation with keylux. See the
> [keyboard compatibility guide](docs/KEYBOARDS.md) for 15 mechanical targets,
> two additional optical/Hall-effect targets, setup, and connection limits.

## Why

The F75 is a popular budget board with no open-source lighting support. OpenRGB
has an [open, unimplemented request](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/4232)
for it, and the one published reverse-engineering write-up stops at the point
where colours refuse to render.

This project documents the full protocol — including the parts that are
genuinely surprising — and turns it into something usable.

## What works

- Per-key static colour (permanent storage through the native F75 driver)
- Original onboard lighting on the `3554:fa09` F75 receiver: 14 firmware
  effects, saved palettes, brightness, speed, and single/multicolor controls
- Native F75 animation at ~21 FPS; OpenRGB streaming capped at 20 FPS,
  adjustable downward
- Native F75 LED map and dynamic LED order/matrix geometry from OpenRGB
- Device picker, saved device selection, reconnects, and CLI discovery
- Per-device RGB color calibration for tinted switches and keycaps
- Windows installer with folder selection, shortcuts, upgrades, and uninstall
- Desktop app with a live keyboard preview and controls generated from each
  effect's own parameter declaration
- Built-in effects: solid, wave, sweep, bloom, scrolling text
- User effects as `.rhai` scripts, hot-reloaded from `effects/`
- Keyframe animations: paint them in the timeline editor, or import a GIF,
  image or folder of frames
- Layered compositions with per-layer key selection: keep chosen keys static,
  animate others, or turn individual keys off
- Runs in the system tray (Windows), so the lighting keeps going with the
  window closed
- Application lighting profiles (Windows and Linux/X11): automatically switch effects and
  their settings when you focus a game, code editor, or another application

See [`docs/PROTOCOL.md`](docs/PROTOCOL.md) for the native F75 protocol and
[`docs/KEYBOARDS.md`](docs/KEYBOARDS.md) for cross-brand control.

### Mix static and animated keys

In **Create**, choose **Add layer → Motion generator**, then add a
**Static colour** layer above it. Static layers start with the current brush colour
and use Normal blending so they replace the animation on their selected keys.
Choose **Select keys**, press **None**, and click or drag across the keys
you want fixed. Right-click removes keys; Row, Column, and Region select groups.
Outlined keys belong to the active layer. **All** and **Invert** also work on
generator and painted layers, so different effects can cover different areas.

Turn off **Select keys** to paint colours. Fill, clear, paste, and gradient
tools affect only the layer's selected keys. A single-keyframe layer stays static;
add keyframes to animate it. A black Static layer with Normal blending keeps its
selected keys off. Save the composition as `.klx` to retain layers and selections,
then select it for playback. Compositions use the existing wired/OpenRGB streaming
path; native receiver playback remains restricted.

### Desktop workspace

**Play** has a searchable effect library, keyboard preview, and effect controls.
**Create** keeps the layer inspector beside the canvas in larger windows and
stacks the editor in compact layouts. **Apps** contains application profiles;
**Settings** contains connection, appearance, interface size, and window options.
The **Device** menu holds connection diagnostics. The **keylux** menu provides
navigation when the sidebar is hidden or the interface is enlarged.

### Color calibration

Open **Settings → Color calibration** with your keyboard connected. Red, Green,
and Blue default to 100%; reduce channels to compensate for switch or keycap
tints. For a pink tint, try reducing red and blue while leaving green at 100%.
Changes apply immediately to all effects, application profiles, editor live
previews, CLI playback, and frames saved to the keyboard. **Reset calibration**
restores all channels to 100%.

Calibration is saved per device connection, so wired, receiver, and OpenRGB
connections have separate settings. Source effect colors, editor files, and the
on-screen preview remain unchanged. Physical filtering cannot always be fully
corrected, and balancing channels can reduce brightness. Receiver colors still
use the existing limited palette.

### Windows installation

Run `Keylux-0.5.1-Setup-x64.exe`. Choose an installation folder; the default is
`C:\Program Files (x86)\Keylux`. Setup requests administrator access, installs
for all users, adds a Start menu shortcut, and offers an
optional desktop shortcut. Later
installers reuse the selected folder. Uninstall through Windows **Installed apps**.

Installed builds store editable scripts and animations in
`%APPDATA%\keylux\content\effects` and `content\animations`. Settings remain in
`%APPDATA%\keylux\settings.json`. Upgrades and uninstall preserve these user
files. Bundled scripts are copied only when missing, preserving edits. Copy any
custom scripts or animations from an older portable folder into these folders.
Existing startup registration is redirected to the installed executable; new
installs leave startup off until you enable it in Settings. Exit portable copies
from their tray icons before launching the installed app.

The ZIP remains available for portable use. To build the installer locally,
install [Inno Setup](https://jrsoftware.org/isdl.php), then run:

```powershell
./packaging/windows/build-installer.ps1
# Or specify the compiler:
./packaging/windows/build-installer.ps1 -Compiler 'C:\path\to\ISCC.exe'
```

The installer is written to `target/installer/`. Version-tag release builds also
attach it alongside the portable archive.

On Windows and Linux, enable **Settings → Startup → Run at startup** to launch
keylux whenever you sign in to your desktop. Changes apply immediately, and the
checkbox reads the actual startup registration.

Windows uses the current user's
[`Run` registry entry](https://learn.microsoft.com/windows/win32/setupapi/run-and-runonce-registry-keys),
which is removed when unchecked. Linux uses an
[XDG autostart entry](https://specifications.freedesktop.org/autostart/latest/)
at `$XDG_CONFIG_HOME/autostart/keylux.desktop`, defaulting to
`~/.config/autostart/keylux.desktop`. Unchecking writes a `Hidden=true` override
so a system-wide entry cannot start it either. Linux desktop environments must
support XDG autostart; this starts the GUI after desktop sign-in, not before login.
Neither platform requires administrator permissions.

Keep the executable in its installed folder; after moving it, turn this option
off and on to register its new location.

Use Ctrl/Cmd+1 through 4 to navigate, Ctrl/Cmd+comma for Settings, and Ctrl/Cmd+S
to save a composition. **Keyboard controls** below the canvas provide key
selection and painting without pointer input. See [the design notes](docs/UI-DESIGN.md)
for the UI audit, visual system, accessibility work, and verification scope.

## Build

Needs a Rust toolchain and a C linker (`hidapi` links against system HID
libraries).

- **Windows** — [rustup](https://rustup.rs) plus Visual Studio Build Tools with
  the C++ workload
- **Linux** — rustup, plus the distro-specific packages listed below
- **macOS** — rustup and Xcode command line tools

```bash
cargo build --release
```

### NixOS

The flake packages Keylux, its bundled effects, desktop launcher, and rules for
the supported AULA wired keyboard and receiver. Build or run it with:

```bash
nix build
nix run . -- list
nix run . -- devices --native
nix develop # Rust toolchain and native build dependencies
```

For persistent installation and non-root device access, add this input to your
system flake (use `path:/absolute/path/to/keylux` for a local checkout):

```nix
inputs.keylux.url = "github:SibteProf/keylux";
```

Then add the module to your `nixosSystem` modules:

```nix
modules = [
  inputs.keylux.nixosModules.default
  { programs.keylux.enable = true; }
];
```

Rebuild your system and reconnect the keyboard/receiver. The module grants the
active local desktop user access through `uaccess`; no root GUI or extra group
membership is needed. It does not start lighting automatically. Launch **Keylux**
from your desktop, or run `keylux`. Desktop autostart can be enabled in Settings.

Packaged scripts live in `$out/share/keylux/effects`. Set `KEYLUX_EFFECTS_DIR`
to a writable copy if you want to edit or add scripts. Packaged animations and
compositions are saved under `$XDG_DATA_HOME/keylux/animations` (default
`~/.local/share/keylux/animations`); `KEYLUX_ANIMATIONS_DIR` overrides that path.
Preferences remain under `~/.config/keylux`.

`nix flake check` builds the package and runs the workspace tests plus a check
that the installed command loads every bundled script outside the source tree.
Hardware checks are separate: `keylux diagnose --device 3554:fa09` only enumerates
the receiver, and `keylux devices --native` checks supported-device discovery.
Factory lighting presets are firmware settings, separate from Keylux's effects
and application profiles. `keylux onboard status --native --device 3554:fa09`
reads the supported receiver's current effect and palettes. See Onboard lighting
below; never apply a wired config block to a receiver.

### Other Linux distributions

Building `keylux` needs the GTK 3 development headers and a few system
libraries for HID and input emulation. Install these packages for your
distribution before running `cargo build --release`.

**Debian/Ubuntu**

```bash
sudo apt update
sudo apt install libgtk-3-dev libxdo-dev libudev-dev libusb-1.0-0-dev pkg-config
```

**RHEL/Fedora**

```bash
sudo dnf install gtk3-devel libxdo-devel libudev-devel libusb1-devel pkg-config
```

**Arch/Manjaro**

```bash
sudo pacman -S gtk3 xdotool libusb pkgconf
```

Opening the vendor HID interface needs a udev rule:

```bash
sudo cp packaging/99-aula.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

The rule covers the wired board. A 2.4 GHz receiver has its own id, so
`packaging/99-aula.rules` carries a commented template to fill in with whatever
`scan --deep` reports.

## Cross-brand quick start

1. Install a current [OpenRGB release](https://openrgb.org/releases.html)
   that detects your keyboard. The compatibility reference is 1.0rc3.
2. Connect the keyboard by USB and start OpenRGB's **SDK Server** on
   `127.0.0.1:6742`. Close other applications that are actively controlling its
   lighting, including OpenRGB effects plugins.
3. Run `cargo run -p aula-app`. In **Settings → Keyboard**, rescan and select
   your OpenRGB keyboard. Pick an effect or use the editor's live preview.

```powershell
cargo run -p aula-app -- keyboards
cargo run -p aula-app -- devices --openrgb 127.0.0.1:6742
# Replace the name below with the exact connected name printed by devices.
cargo run -p aula-app -- wave 20 --openrgb 127.0.0.1:6742 --keyboard "Razer BlackWidow V4 Pro"
```

`--openrgb IP:PORT` restricts CLI discovery to that server. `--native` restricts
it to AULA HID. With neither flag, both enabled backends are available and the
GUI's saved selection is respected. Use `--help` for all connection options.

The GUI controls one selected keyboard at a time. OpenRGB support follows the
exact model, firmware, and connection detected by OpenRGB; see the
[full model table](docs/KEYBOARDS.md#model-targets).

## Native AULA F75 quick start

Both **wired USB-C** and the **2.4 GHz receiver** work. Every command below picks
whichever is attached; the wired board wins when both are.

The two are different USB devices speaking different protocols, so they behave
differently and the difference is not hideable:

| | Wired | Receiver |
| --- | --- | --- |
| Rate | 21 FPS | **~8 FPS** full-board; faster for sparse effects |
| Update | whole 378-byte framebuffer, one write | whole frame, ~10 writes of 20 bytes |
| Cost driven by | nothing much | number of **distinct colours** |

Colours on the receiver are **quantised** to a small palette, because a frame
costs 4 bytes per distinct colour and has to fit one message. Wired sends each
key a continuous colour, so a gradient flows through a key where the receiver
steps through it.

The receiver is also **throughput-limited**, to about 64 chunks per second —
measured, and the vendor's own driver uses about a fifth of that. A whole-board
frame is 8 chunks, so it lands at ~8 FPS; a sparse effect costs less and runs
faster. Pushing past it does not fail cleanly: the board flickers between the
new frame and the old, which looks like something fighting the effect for
control of the colours.

So full-board animation is a wired feature in practice, and the receiver is at
its best with static colour and sparse effects — which is what the vendor's own
software does with it. Its smooth wireless effects are **firmware** effects,
rendered on the keyboard with the radio idle. `docs/PROTOCOL.md` has the
arithmetic. Bluetooth exposes nothing on either.

```bash
cargo run --example smoke -- scan             # devices that speak the protocol
cargo run --example smoke -- scan --deep      # also look for a receiver
cargo run --example smoke -- ping             # read the config block, write nothing
cargo run --example smoke -- red              # whole board red
cargo run --example smoke -- wave 20          # streamed rainbow wave for 20 seconds
cargo run --example smoke -- dry              # print packet headers, no hardware needed
```

## Layout

```
crates/
  aula-protocol/   HID transport, device trait, F75 driver, OpenRGB SDK client
  aula-effects/    effect engine, parameters, built-ins, Rhai host
  aula-app/        desktop app
effects/           user-authored .rhai effect scripts
docs/PROTOCOL.md   the full protocol write-up
docs/KEYBOARDS.md  cross-brand compatibility, setup, and validation
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
- [x] System tray on Windows
- [x] Theme switching (dark / light)
- [x] Per-application profiles (Windows and Linux/X11)
- [x] Packaged releases
- [ ] System tray on Linux and macOS
- [x] OpenRGB keyboards behind the same `RgbDevice` trait
- [ ] Physical validation of the additional keyboard targets with keylux

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

## Onboard lighting

Keylux opens with software playback stopped. Connecting does not select a
default effect, change lighting mode, or stream colors. Click **Start Keylux
lighting** to begin software playback, or explicitly enable an editor preview.

For the verified `3554:fa09` receiver, **Play → Onboard lighting** reads the
current effect and its saved settings. Select a native effect, adjust its
controls, and click **Apply onboard lighting**. The keyboard renders the
original firmware effect, with its own palette and timing. Choosing another
mode loads that mode's existing settings. The palette swatches show actual
onboard RGB values; editing those palettes is not yet exposed.

The onboard keyboard preview animates locally using the selected effect,
saved palette, brightness, speed, and multicolor setting. It is an illustrative
simulation, not an exact reproduction of the firmware's animation or timing.
Reactive effects demonstrate simulated key presses. Previewing never streams
frames to the keyboard; **Apply onboard lighting** selects the original firmware
effect. Custom per-key colors are not yet available for preview.

Pausing or quitting a receiver software-playback session restores the native
effect observed just before that session. Restoration uses a fresh receiver
read and preserves other settings. A disconnected receiver cannot be restored
while disconnected; select the desired onboard effect after reconnecting.

```bash
keylux onboard list
keylux onboard status --native --device 3554:fa09
keylux onboard apply "Dream Rainbow" --native --device 3554:fa09
keylux onboard apply Drifting --brightness 2 --speed 4 --multicolor --native --device 3554:fa09
```

`status` is read-only. `apply` updates persistent lighting settings through a
complete, checksum- and signature-validated receiver read. It never substitutes
a wired config block or writes factory defaults over key mappings. The speed
field is four bits (16 levels); brightness has 21 levels. This native interface
is currently verified only for the receiver, not every device sold as F75.

Firmware effects and their stored palettes are supported directly; Keylux does
not extract the firmware's animation code or provide a factory-reset command.

## Application lighting profiles

Open the **Apps** tab to give each application its own lighting:

1. Choose **Default lighting** for applications without a matching profile.
2. Click **Add application profile** and name it, for example *Coding* or *Gaming*.
3. On Windows, enter executable names such as `Code.exe, cursor.exe` or your game's
   `.exe`. **Choose application…** adds a full path to match only that installation.
   On Linux/X11, enter the application's `WM_CLASS`, such as `Code` or `Firefox`.
   Focus the application, return to keylux, and click **Add last active app** to
   use the detected class. You can also run `xprop WM_CLASS` and click a window;
   use the second quoted value. Matches are exact, ignoring case.
4. Choose an effect and adjust its colors, speed, brightness, and other controls.
   **Use current lighting for this profile** copies your selection from Play.
5. Enable **Switch automatically** and focus the application.

The first matching enabled profile wins; **Move up** and **Move down** change
priority. Profiles and the default are saved with your preferences and restored
on startup. Returning to an unmatched application restores the default. Disabling
automatic switching also restores the default, after which Play works manually.

Detection follows the foreground application, so a game running in the background
does not override your editor. It runs on the lighting worker, including while
keylux is in the tray with background lighting enabled. Opening keylux keeps the
current lighting selected so you can edit or copy it. Pause still stops lighting
writes, and the composition editor's live preview takes priority.

Missing effect files and animations blocked by the wireless link use the default
and show a warning. If the default is also unavailable, a solid color is used.
The saved profile stays intact and is used again when its effect or wired
connection becomes available. Automatic foreground detection supports Windows and
Linux X11 sessions. Wayland is not supported, including XWayland applications;
the toggle is disabled there. macOS can save profiles but cannot switch automatically.

The X11 detector is adapted from [headblade-dev's contribution in PR #15](https://github.com/SibteProf/keylux/pull/15).

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
