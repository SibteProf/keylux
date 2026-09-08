# AULA F75 RGB protocol

Reverse-engineered from USBPcap captures of the vendor driver, and verified on
hardware. No public source documents this; OpenRGB has an
[open, unimplemented request](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/4232)
for the board, and the one published write-up stops before colours render.

## Device

| | |
| --- | --- |
| VID:PID | `258a:010c` (Sinowealth) |
| Connection | **wired USB-C only** — the 2.4 GHz dongle and Bluetooth do not expose this interface |
| Transport | HID **feature reports**, report ID `0x06`, 520 bytes |

### Finding the right interface

Do **not** select by usage page. On this hardware:

- Windows opens the keyboard and mouse top-level collections exclusively for
  the OS, so those handles are unusable despite sharing the VID/PID.
- There are three vendor collections and Windows reports usage page `0xFF00`
  for all of them. Published notes cite `0xFF13`, and the collection index
  varies between machines.

Instead, **probe**: open each vendor-range collection and attempt a 520-byte
feature report on report ID `0x06`. Only the real RGB collection accepts it.
This is read-only.

## Packet layout

```
byte 0      report id, always 0x06
byte 1      command
bytes 2..5  address (00 00 01 00 for colour and config)
bytes 6..7  payload length, little-endian
bytes 8..   payload, zero padded to 512
```

## Three colour paths

This is the part nobody has published. There is no single "set the colours"
command — there are three, with **different payload layouts**.

| Purpose | Cmd | Length | Layout | Slots |
| --- | --- | --- | --- | --- |
| Static per-key (persists) | `0x06` | `0x0180` (384) | **planar**: 128 R, then 128 G, then 128 B | 128 |
| Live streaming (animation) | `0x08` | `0x017a` (378) | **interleaved** RGB | 126 |
| Solid single colour | `0x0a` | `0x0200` (512) | interleaved; one colour painted on every key | 170 |

Channel order is plain **RGB**, proven by decoding the driver's payload at
stride 3 into exact primaries and secondaries (red, green, blue, yellow,
magenta, cyan, white in sequence). At stride 4 it is noise.

### Static vs streaming

`0x06` renders a single frame correctly and the result survives a reboot. But
**repeated** writes to it blank the entire board, charging indicator included.
Animation must use `0x08`.

The two captures that revealed this used different commands because the driver
picks a path based on what you set in its UI — a static per-key apply uses
`0x06`, a music-reactive mode uses `0x08`. One capture was never going to be
enough.

## Config block

Read `0x84`, write `0x04`, 128 bytes at address `00 00 01 00`. A valid block
ends with the signature `5A A5`.

### Lighting mode

Host colours only render in per-key mode:

| Mode | `[9]` | `[10]` | `[59]` |
| --- | --- | --- | --- |
| Per-key | `0x01` | `0x15` | `0x47` |
| Solid | `0x00` | `0x01` | `0x40` |

**All three bytes matter.** With only 9 and 10 set, the board goes completely
dark — indicator included. Byte 59 is the first effect entry's speed/colour
nibble. The safe approach is to write a known-good 128-byte block verbatim.

### Config writes are disruptive

A config write triggers an **asynchronous repaint** inside the firmware,
hundreds of milliseconds later, which overwrites any frame sent in the
meantime. This looks exactly like "the colour write silently failed". Wait
~400 ms after a config write before streaming, and never write config from an
animation loop.

### Config reads can be corrupt

The firmware returns truncated, mostly-zero buffers while busy — reliably
reproducible by polling as the mode knob is pressed. Since mode selection is
read-modify-write, accepting one and writing it back zeroes the whole config
block. Always validate `5A A5` first.

## LED index map

LED order is the keyboard's own key-matrix order, exposed via command `0x83`
(4 bytes per entry, HID usage code in byte 3). It is **column-major**: down the
leftmost physical column (Esc, `` ` ``, Tab, Caps, LShift, LCtrl), then the next
column. 90 of the 128 indices are real keys.

Verified by writing a single `0xFF` byte at buffer offset 1, which lit the
backtick key red — matching index 1 and confirming both the ordering and the
planar layout in one test.

## Timing

The driver streams its own music-reactive modes at **21.5 FPS** — 46.5 ms
between writes, never below 44.7 ms.

Writing faster starves the 8051's key-scanning loop: the keyboard stops
responding to keypresses until it is replugged. **60 FPS is not achievable.**
~21 FPS is the hardware's comfortable rate.

## Things that are true but look wrong

Recorded so nobody re-derives them the hard way:

- **`0x0a` at address `00 00 00 00` is a staging buffer.** It accepts writes and
  reads them back byte-for-byte, and drives nothing at all. The real
  framebuffer is at `00 00 01 00`. A perfect round-trip there proves nothing —
  this is the trap the published notes lead you into.
- **The command selects the region**, not the address. The address only
  distinguishes real from staging.
- **Static is planar, streaming is interleaved.** Same device, same session,
  different layouts. A solid colour or a smooth gradient looks identical either
  way, which hides the difference.
- **A per-key frame sent in solid mode renders as one flat colour.** A rainbow
  came out as uniform yellow — the firmware working as designed, not a failed
  write.
- Published notes describe a 4-byte `RR GG BB 00` slot layout. **No path on this
  board uses that.**

## Safety

- **Never brute-force write commands.** Firmware is readable at several command
  bytes (`0x81`, `0x88`, `0x89`, `0x9d`–`0xa2` return 8051 code), so their write
  counterparts would write firmware. A blind sweep could brick the keyboard.
- **Never stream over `0x06`.**
- **Always validate `5A A5` before a config write.**

## How a second device gets added

The same way this one was: capture the vendor driver with USBPcap while it
applies colours, then decode. Two notes that cost hours here:

1. **Replug the device after starting the capture.** USBPcap hooks a device's
   stack when it *starts*; a device already running yields almost nothing.
2. **Capture both a static apply and an animated mode.** They use different
   commands, and you cannot infer one from the other.
