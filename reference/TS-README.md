# aula-f75-rgb

Per-key RGB control for the **AULA F75** (Sinowealth `258a:010c`) over USB HID.
TypeScript, stock firmware, no firmware modification.

**Working:** every key set to an arbitrary colour, and smooth animation at
~21 FPS. Both verified on hardware.

```bash
npm install
npm run info      # find the keyboard, show which HID collection is used
npm run red       # every key red
npm run wave      # animated rainbow wave
npm run sweep     # bright bar sweeping across the board
```

## The protocol

All traffic is HID **feature reports**, report ID `6`, 520 bytes, on the
keyboard's vendor collection. Wired USB-C only — the 2.4 GHz dongle and
Bluetooth do not expose this interface.

```
byte 0      report id, always 0x06
byte 1      command
bytes 2..5  address
bytes 6..7  payload length, little-endian
bytes 8..   payload, zero padded to 512
```

### Three separate colour paths

This is the thing that makes the F75 confusing, and it is not documented
anywhere public. There is no single "set the colours" command — there are
three, and they use **different layouts**:

| Purpose | Command | Length | Layout | Slots |
| --- | --- | --- | --- | --- |
| Static per-key (persists) | `0x06` | `0x0180` (384) | **planar** — 128 R, then 128 G, then 128 B | 128 |
| Live streaming (animation) | `0x08` | `0x017a` (378) | **interleaved** RGB | 126 |
| Solid single colour | `0x0a` | `0x0200` (512) | interleaved, one colour used for all keys | 170 |

All three use address `00 00 01 00`.

Use `0x06` to set a static picture. Use `0x08` for anything that updates
repeatedly. **Do not animate over `0x06`** — it renders a single frame
correctly but blanks the whole board, charging indicator included, when
written continuously.

### Lighting mode

Host colours only render while the board is in the right lighting mode, which
lives in the config block (`cmd 0x84` read / `0x04` write, 128 bytes,
address `00 00 01 00`, ends with a `5a a5` signature):

| Mode | config[9] | config[10] | config[59] |
| --- | --- | --- | --- |
| Per-key | `0x01` | `0x15` | `0x47` |
| Solid | `0x00` | `0x01` | `0x40` |

Patching only bytes 9 and 10 is **not** enough — with byte 59 left wrong the
board goes completely dark. `device.ts` therefore writes a full known-good
128-byte block (`PERKEY_CONFIG`), captured from the vendor driver.

A config write triggers an **asynchronous repaint** ~100s of ms later that
overwrites whatever frame you just sent, so `ensureCustomMode()` blocks until
it settles, and only writes when the mode is actually wrong.

### LED index map

LED order is the keyboard's own key-matrix order, which the firmware exposes
via `cmd 0x83` (4 bytes per entry, HID usage code in byte 3). It is
column-major: down the leftmost column (Esc, `` ` ``, Tab, Caps, LShift,
LCtrl), then the next column. 90 of the 128 indices are real keys.

`src/keymap.ts` has the decoded order plus the physical geometry, so effects
sample real positions and sweep across the board in space rather than in
wiring order.

## Timing

The driver's own music-reactive modes stream at **21.5 FPS** (46.5 ms between
writes, never below 44.7 ms), and that is what this matches.

60 FPS is not achievable. A write costs a fixed ~14 ms on the streaming path
regardless of payload size, and the firmware needs idle time between writes to
service its key-scanning loop — drive it harder and the keyboard stops
responding to keypresses until it is replugged. ~21 FPS is the hardware's
comfortable rate, and animation phase comes from the wall clock so effects
travel at the correct real-world speed regardless.

## Commands

| Command | Purpose |
| --- | --- |
| `info` | list HID collections, show which one was selected |
| `red` | every key red (static path) |
| `wave` | animated rainbow wave (streaming path) |
| `sweep` | bright bar sweeping across the board |
| `dump --out baseline.bin` | read config + colour table, hexdump, save |
| `layout` | print the parsed key geometry and LED indices |

Flags: `--duration`, `--fps`, `--brightness`, `--speed`, `--cycles`, `--tilt`,
`--verbose`, `--order`, `--path`.

Helpers: `npm run bench` (write-cost timing), `npm run restore` (restore
`baseline.bin`), `npm run enum-commands` (enumerate read commands),
`npm run parse-capture <file.json>` (decode a USBPcap capture).

## Safety

- **Never brute-force write commands.** Firmware is readable at several
  command bytes, so their write counterparts would write firmware. A blind
  sweep could brick the keyboard.
- **Never stream over `0x06`.** Repeated writes on the static path blank the
  board and drop the indicator.
- **Config writes are disruptive.** They knock the board out of per-key mode
  and trigger an async repaint. `writeConfig()` refuses to run unless
  explicitly allowed, and validates the `5a a5` signature first — the firmware
  returns truncated all-zero reads while busy, and writing one back would
  corrupt the config block.
- `dump --out baseline.bin` before experimenting; `npm run restore` puts it
  back.

## Things that are true but look wrong

Recorded so nobody re-derives them the hard way:

- `0x0a` at address `00 00 00 00` is a **staging buffer**. It accepts writes
  and reads them back byte-for-byte, and drives nothing. The real framebuffer
  is at `00 00 01 00`. A perfect round-trip here proves nothing.
- The **command** selects the region; the address only distinguishes real from
  staging.
- Static is **planar**, streaming is **interleaved**. Same device, same
  session, different layouts.
- A rainbow written in solid mode renders as one flat colour, which looks like
  "the write failed" but is the firmware working as designed.
- Published notes describe a 4-byte `RR GG BB 00` slot layout. No path on this
  board uses that.

## How it was worked out

Public sources (below) get as far as the packet header and stop at the latch.
Everything past that came from USBPcap captures of the vendor driver:

1. Driver colouring three keys → static path, planar layout, LED index 0 = Esc.
2. Driver setting all keys one colour → solid path, and the mode bytes.
3. Driver running music-reactive modes → **the streaming path**, and the real
   frame rate.

Capture 3 was the one that mattered: nothing about `cmd 0x08` is inferable
from the static captures, because the driver only uses it while animating.

## Sources

- [Reverse Engineering My AULA F75 Keyboard on Linux — Xevrion](https://xevrion.dev/blogs/aula-f75-linux-reverse-engineering) — packet header and command bytes
- [OpenRGB issue #4232 — AULA F75](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/4232) — VID/PID; unimplemented
- [Punkster81/AULA-F108-Driver](https://github.com/Punkster81/AULA-F108-Driver) — sibling Sinowealth board
- [carlossless/sinowisp](https://github.com/carlossless/sinowealth-kb-tool) — Sinowealth 8051 HID background
