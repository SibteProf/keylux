# Keyboard compatibility

Checked **30 September 2026**. Keylux has two control paths:

- **Native AULA F75:** the existing HID driver, with its verified wiring map,
  wired/receiver pacing, and permanent-frame command.
- **OpenRGB:** a TCP SDK client that sends keylux effects to keyboards detected
  by a separately running OpenRGB server. This supports multiple brands through
  their existing OpenRGB drivers.

The SDK integration is implemented in the desktop app, CLI, effect engine,
and editor. **The additional models have upstream Direct-mode support, but
have not been physically tested with keylux in this change.** Automated tests
verify the SDK protocol and application behavior; they cannot establish actual
LED colors, typing reliability, or firmware compatibility on absent hardware.

## Model targets

These 15 conventional mechanical RGB models cover premium, compact, wireless,
and full-size choices across five brands. They include current flagships and
established models still relevant to RGB control. Selection was informed by
current [RGB keyboard reviews](https://www.rtings.com/keyboard/reviews/best/rgb)
and [gaming keyboard reviews](https://www.pcgamer.com/best-gaming-keyboard/).
“Top” depends on layout, switch feel, gaming features, and budget; the table is
a compatibility selection rather than a ranked buying guide.

Every additional row below is listed with Direct support in OpenRGB's
[1.0rc3 device data](https://openrgb.org/data/supported_devices_1.0rc3.csv).
USB IDs identify the exact variants in that source; keylux does not send HID
commands to these IDs. OpenRGB owns hardware discovery and LED transport.

| Model | USB VID:PID | Initial connection to use |
| --- | --- | --- |
| Corsair K100 RGB, Cherry MX Red | `1b1c:1b7d` | USB |
| Corsair K70 RGB PRO / PRO V2 | `1b1c:1bc4`, `1b1c:1bb3` | USB |
| Corsair K95 RGB Platinum XT | `1b1c:1b89` | USB |
| Razer BlackWidow V4 Pro | `1532:028d` | USB |
| Razer BlackWidow V4 75% | `1532:02a5` | USB |
| Razer BlackWidow V4 Pro 75% | `1532:02b3` | USB |
| ASUS ROG Azoth | `0b05:1a83` | USB |
| ASUS ROG Strix Scope II 96 Wireless | `0b05:1aae` | USB |
| Logitech G915 | `046d:c33e` | USB |
| Logitech G915 TKL | `046d:c343` | USB |
| HyperX Alloy Origins | `0951:16e5`, `03f0:0591` | USB |
| HyperX Alloy Origins Core | `0951:16e6`, `03f0:098f` | USB |
| HyperX Alloy Origins 65 | `03f0:038f` | USB |
| HyperX Alloy Elite 2 | `0951:1711`, `03f0:058f` | USB |
| ASUS ROG Strix Scope II | `0b05:1ab3` | USB |

Additional optical/magnetic choices in the same upstream source:

| Model | Switch technology | USB VID:PID |
| --- | --- | --- |
| SteelSeries Apex Pro TKL Gen 3, wired | Hall effect | `1038:1642` |
| Razer Huntsman V3 Pro | Analog optical | `1532:02a6` |

The native AULA F75 remains an additional supported model. `keylux keyboards`
prints the reference targets; `keylux devices` prints actual connected devices.

The backend also discovers other OpenRGB keyboards that expose a **Direct**
mode with per-LED color capability and a usable LED list. It excludes mice,
motherboards, and other device types. A family name alone is insufficient to
establish support for a new generation or receiver. For example, G915 support
does not establish G915 X support; RGB or QMK branding alone does not establish
stock-firmware support for a Keychron keyboard.

## Setup

1. Install a current [OpenRGB build](https://openrgb.org/releases.html) with a
   driver for the exact keyboard. The table above uses 1.0rc3. The SDK wire
   format requires protocol 3 or newer (0.7+), but older releases may lack
   the model's driver.
2. Connect by USB first. Verify that OpenRGB detects the keyboard and exposes
   per-key **Direct** mode. Set its brightness above zero.
3. Stop competing lighting applications and effects plugins for that keyboard.
4. Start the **SDK Server** in OpenRGB, normally at `127.0.0.1:6742`, and keep
   OpenRGB running while keylux controls the board.
5. Start keylux, open **Settings → Keyboard**, enable OpenRGB, and click
   **Rescan keyboards**. Select the desired board in the device picker.
6. Choose an effect in **Play**, or paint a frame in **Create** and enable
   **Send to keyboard**.

The SDK address can be changed in Settings and applied without restarting.
Use a numeric IP and port; IPv6 uses `[::1]:6742`. The default endpoint is local.
The **Scan for receivers** button belongs to the native AULA discovery path;
OpenRGB handles other manufacturers' receivers itself.

On Linux, install OpenRGB's [udev rules](https://openrgb.org/udev.html) for the
other keyboards. Keylux's `packaging/99-aula.rules` only covers its native HID
driver. The OpenRGB backend itself needs a TCP connection, not HID access.

### CLI

```powershell
# Reference targets; no hardware access.
cargo run -p aula-app -- keyboards

# Read-only SDK discovery; no native HID discovery with this option.
cargo run -p aula-app -- devices --openrgb 127.0.0.1:6742

# Copy the exact connected name from the previous command.
cargo run -p aula-app -- solid 10 --openrgb 127.0.0.1:6742 --keyboard "YOUR EXACT DEVICE NAME" --color "#ff0088"
cargo run -p aula-app -- wave 20 --openrgb 127.0.0.1:6742 --keyboard "YOUR EXACT DEVICE NAME" --fps 10

# Preserve the direct AULA path.
cargo run -p aula-app -- wave 20 --native --device 258a:010c
```

`--keyboard` requires exactly one matching connected name. If two keyboards
have the same name, use the full `--device` selector from `devices` instead.
Selectors encode the server's name, vendor, serial, and location into shell-safe
hexadecimal fields and are re-resolved
on every connection. Moving a board to a different USB port can change its
location and require selecting it again. Identical, ambiguous identities are
rejected. A missing saved selection stays missing rather than choosing another
board. Choose **Automatic** in the GUI to clear a saved selection.

With no backend flag, the CLI uses enabled backends from settings and honors
the saved GUI selection. `--openrgb IP:PORT` restricts it to that server;
`--native` restricts it to AULA. CLI overrides do not modify saved settings.

## Behavior and limits

- **One selected keyboard at a time.** Multiple boards can appear in the picker.
- **Frames and layout come from the selected device.** The SDK supplies LED
  count, order, key labels, and matrix geometry. ANSI/ISO and model differences
  use the server's actual layout. Matrix coordinates are a grid approximation,
  not an exact measurement of keycap spacing.
- **Missing geometry:** the preview explicitly says “LED grid.” Per-LED control
  still works, but spatial effects follow the displayed grid. Auxiliary LEDs
  outside a matrix appear in additional rows.
- **Streaming ceiling:** 20 FPS by default, adjustable downward. This is a
  conservative software cap, not a hardware measurement for every model.
  Slower keyboards/receivers may require a lower rate. The server can also
  impose its own update rate.
- **Connection modes:** use the table's USB connection first. Receiver support
  depends on the exact OpenRGB driver. Bluetooth control is not promised.
- **Persistence:** keylux uses live Direct-mode updates through OpenRGB and
  disables permanent NVRAM writes for that backend. It does not call SDK
  SaveMode, profile saving, zone resizing, or firmware update operations.
- **Effects:** built-ins, Rhai scripts, image/GIF imports, editor previews, and
  procedural composition layers use the selected board. Generator caches
  refresh when geometry, LED order, or LED count changes.
- **Saved painted frames:** existing animation files store LED indices rather
  than portable key identities. Reimport/recreate them for a different board.
  Different-size editor previews are blocked; switching devices stops live
  preview and retains the composition. Save it, then choose **New composition
  for this keyboard**. Same-size files can still have different LED ordering.
- **Failures:** malformed/truncated packets, wrong frame sizes, changed device
  identities, and device-list-change notices stop that connection before further
  lighting writes. The GUI retries discovery. CLI runs return an error.
  Protocol 3 uses list indices, so avoid rescanning OpenRGB's hardware while
  streaming; keylux revalidates the target before each frame but the old SDK
  cannot make a read-and-write pair atomic.
- **Pause/exit:** stops sending lighting updates. The last frame can remain
  visible; automatic restoration of the previous vendor effect is not provided.

## Validation

Automated validation covers:

- Protocol negotiation with newer SDK servers while requesting version 3 data.
- Partial TCP reads, RGB channel order, RGBX padding, exact packet/frame sizes,
  Direct-mode selection, deduplication, and write-rate bounds.
- Matrix gaps, repeated wide-key cells, zone-relative LED indices, extra LEDs,
  fallback grids, invalid indices, malformed packets, and truncated data.
- Changed/reused controller indices, hotplug notices, connection invalidation,
  unavailable servers, duplicate device identity rejection, and keyboard-only
  discovery.
- An actual CLI subprocess discovering and streaming against a local test
  server, without accessing HID devices.
- Native selection priority, saved-settings migration, CLI options, and
  composition caches across same-size boards with different LED ordering.

Run the repository checks:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
cargo build --workspace --release
```

Physical validation for each model remains outstanding: check distinct red,
green, and blue keys; verify matrix positions; run motion while typing; then
test unplug/replug, pause, and the intended connection mode. Record the keyboard
firmware, layout/region, and OpenRGB build when reporting results.

## Protocol and compatibility sources

- [OpenRGB SDK documentation](https://github.com/CalcProgrammer1/OpenRGB/blob/af36b81a7927d94872f647e2056c0fedb9fc52ec/Documentation/OpenRGBSDK.md)
- [OpenRGB controller definitions](https://github.com/CalcProgrammer1/OpenRGB/blob/af36b81a7927d94872f647e2056c0fedb9fc52ec/RGBController/RGBControllerInterface.h)
- [OpenRGB SDK server](https://github.com/CalcProgrammer1/OpenRGB/blob/af36b81a7927d94872f647e2056c0fedb9fc52ec/NetworkServer.cpp)
- [OpenRGB 1.0rc3 compatibility table](https://openrgb.org/devices_1.0rc3.html)

The downloaded 1.0rc3 CSV had SHA-256
`b3ec8c78048b73ddf68a344fdd530681aaa272fd775571b32d50c1d0da69df29`.
The SDK client is an independent implementation of the documented network
format. OpenRGB remains a separate GPL-licensed application; no hardware-driver
source was copied into keylux.
