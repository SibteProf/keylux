# AULA F75 RGB protocol

Reverse-engineered from USBPcap captures of the vendor driver, and verified on
hardware. No public source documents this; OpenRGB has an
[open, unimplemented request](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/4232)
for the board, and the one published write-up stops before colours render.

## Device

| | |
| --- | --- |
| VID:PID | `258a:010c` (Sinowealth) |
| Connection | HID feature reports over **wired USB-C**. The 2.4 GHz receiver is a separate USB device under its own VID/PID and speaks a **different protocol** — decoded [below](#wireless-24-ghz--a-different-protocol-on-a-different-device). Bluetooth exposes neither. |
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

Accepting the report ID is **not** enough to identify the collection, and
neither is the width of the reply. Ask it something: send `0x84` and require the
`5A A5` config signature. That is the only test that works.

Width looks like it should work and does not. A bare feature read — one sent
before any command — returns **9 bytes** from the real keyboard, the same as the
receiver's status collection returns. Rejecting short replies therefore rejects
the keyboard itself. This cost a working wired setup once; do not reintroduce it.

### Wireless (2.4 GHz) — a different protocol, on a different device

The receiver does **not** carry the wired protocol. It carries its own, and the
two have nothing in common but the keyboard at the far end.

Measured on an AULA receiver, `3554:fa09` (Compx), bus-reported "2.4G Wireless
Receiver", keyboard associated and the cable unplugged. Decoded from a USBPcap
capture of AULA F75 v2.0 driving it.

#### Why probing alone gives the wrong answer

The receiver presents two vendor collections:

| Collection | Usage page | Report ID | Direction | Width |
| --- | --- | --- | --- | --- |
| `MI_01 Col01` | `0xFF02` | `0x13` | Input + **Output** | 19 bytes |
| `MI_01 Col06` | `0xFF04` | `0x06` | Feature | 7 bytes |

`Col06` is a trap. It uses **the same report ID as the wired protocol** (`0x06`)
and accepts a feature read, so a probe asking only "did this take report `0x06`?"
says yes — then every read returns the same constant,
`06 84 02 00 e5 00 46 29 bd`. It is a status report, not a colour pipe.

The tempting fix is to reject it on width — 7 bytes cannot be a 520-byte pipe.
**That does not work**, and trying it broke wired support: a bare feature read
returns 9 bytes from the real keyboard too. What separates them is the reply to
`0x84`, which is a valid config block from the keyboard and the same unchanging
constant from `Col06`.

`Col01` is the real channel, and a feature-report probe can never find it: it has
no feature reports at all (`HidD_GetFeature` returns `Incorrect function`). It
takes **output** reports. Only a capture reveals it.

#### The frame

20 bytes, sent as HID `SET_REPORT` (output, report `0x13`) on interface 1:

```
byte 0      report id, always 0x13
byte 1      command family (0x88 = lighting)
byte 2      total chunks in this message
byte 3      chunk index, 0-based
byte 4      16 + payload length in this chunk
bytes 5..18 payload, zero padded
byte 19     checksum: sum of bytes 0..18, mod 256
```

The checksum held on **732 of 732** captured packets.

Messages longer than 14 bytes are split across chunks, all but the last carrying
a full 14 (`byte 4` = `0x1e`). The largest seen is 6 chunks = **84 payload
bytes**; there is no wider path in the capture.

#### Lighting payload

A sequence of groups, each `[R, G, B, count, index * count]` — "paint these LEDs
this colour". A single-key edit is one group of one, which is what the vendor UI
sends per click:

```
13 88 01 00 15  00 ff ff  01 37  00.. e7     LED 0x37 = cyan
13 88 01 00 16  00 00 ff  02 24 2a  00.. 01  LEDs 0x24,0x2a = blue
13 88 01 00 1a  00 ff ff  01 43  ff ff 00 01 27  00.. 1e   two groups in one frame
```

**The LED indices are the same ones the wired protocol uses** — the 6-stride
column-major order in [LED index map](#led-index-map). A whole-board apply is one
group with `count = 0x50` (80 keys) and 80 indices, which is exactly the 84 bytes
that fill 6 chunks.

#### Animation: deltas on a tick

The capture includes a music-reactive mode, and it settles the question — the
host **does** stream. It is just a different shape from the wired path.

There is no framebuffer command. Each tick sends only the LEDs whose colour
changed, grouped into runs sharing a colour. That is the only reason a board's
worth of animation fits down a 14-byte pipe.

Measured over the streaming window:

| | |
| --- | --- |
| Message period, **animating** | **~46-47 ms** → ~21 FPS, the same rate as wired |
| Message period, idle / music-reactive | ~108 ms, event-driven |
| Frames per message | 1-4 animating, up to 6 for a whole-board apply |
| Palette | ~4 levels per channel while animating, e.g. `00` `33` `66` `b2` `ff` |
| Spacing within a message | ~11-12 ms |
| Nothing changed | `13 88 01 00 23 00..00` — meaning UNVERIFIED, see below |

The `0x23` frame is **not** understood, and guessing cost a working solid
colour. It appears 176 times in a music-reactive capture, which made "nothing
changed" a tempting reading — but sending it on every idle tick made a solid
colour flash light/dark/light, so it may equally mean "blank". Music-reactive is
exactly the mode where keys go dark constantly, so the capture cannot tell the
two apart. This driver stays silent when it has nothing to say rather than
sending a frame whose meaning is a guess.

**Measure the rate on a window where something is actually animating.** Taking
it from an idle or music-reactive stretch gives ~108 ms, and a driver built on
that number is wrong twice over: half the frame rate, and twice the delta per
frame, because more has moved between frames. Animations look terrible for both
reasons and the cause is not obvious from the code.

That the animating rate matches the wired floor is not a coincidence — it is the
same firmware and the same key-scanning 8051 at the far end. The tick stretches
as a message grows: 1-2 chunks holds ~46 ms, 3 pushes the next message to
~62 ms, 4 to ~104 ms.

The consequence for effects is worth stating plainly. Cost is per *distinct
colour*, not per key: a solid board is one group of 80 and fits in six chunks,
while a smooth 80-key gradient is ~80 groups and cannot fit in any tick.

So **quantise before grouping**, as the vendor does — its animation payloads use
a coarse palette (`00`, `33`, `66`, `80`, `b2`, `ff`), never arbitrary values.
Banding pays twice: the repaint gets small enough to send, and a key stops
counting as changed until it crosses a band, so a moving effect sends a handful
of keys per frame instead of all eighty.

Do not let a lit key quantise to **black**. A coarse value grid rounds dim
colours to zero — at four value steps everything below 12.5% brightness
disappeared, so a sweep effect showed only its bright line and the keys around
it while the dim background vanished. It reads as the effect failing to paint,
not as a palette being coarse, which is what makes it hard to place. Floor a
non-black colour at one step instead.

Quantise in **HSV, not per RGB channel**. The two are not equivalent at the same
group count: a 4-level RGB grid spreads its 64 colours unevenly around the wheel,
so a hue sweep lands on muddy, unequal steps and the motion reads as lurching.
Snapping hue on its own grid — 12 steps, with saturation and value coarser —
gives evenly spaced colours *and* smaller deltas, since a key only changes when
it crosses a hue boundary. Wrap the hue grid: without it, 0.98 and 0.02 land in
different bands despite being the same red, and the wrap point flickers on every
pass of a rainbow.

Order what you send by AGE, oldest first — and age the LEDs inside a group,
not just the groups. A full-board effect never fits one frame, so something is
always left over, and ordering by group size starves the small groups. On a
moving effect the small groups are its leading and trailing edges, so the bulk
of the board updates every frame while the moving part stutters: it reads as
jitter and is indistinguishable from a timing fault. Groups alone are not
enough either, because an oversized group is truncated to a prefix and the tail
of its list would never ship.

And send COMPLETE frames, even at the cost of frame rate. A truncated frame
strands different keys at different moments of the animation, so the board shows
several timestamps at once — which reads as skipping and "moving too fast", not
as a low frame rate, and no amount of extra FPS fixes it.

The numbers are unforgiving. Simulating a rainbow wave at 21 FPS: ~61 of 80 keys
cross a band every frame, needing ~140 bytes against a 42-byte budget — 0 of 200
frames complete, only ~23% of changed keys sent. Widening to a whole 84-byte
message and coarsening to a 4-level palette completes 98%. The palette cliff is
sharp — five levels completes 0%, four completes 98% — because every extra band
is another group paying a 4-byte header before its first LED index.

Counter-intuitively, **lowering the frame rate makes this worse**: more time
passes between frames, so more keys have moved (73 keys/frame at 10 FPS against
41 at 21). Coherence comes from a bigger budget and a coarser palette, never
from slowing down. Six chunks cost 60 ms of gaps and settle around 14 FPS; that
is the right trade, and the vendor makes it too, letting its tick run to ~104 ms
for a 4-chunk message.


#### Per-key mode, and the effect that fights you

Command `0x04` carries **the same 128-byte config block the wired path writes**,
under the same command byte, chunked into ten frames:

```
13 04 0a 00 0e  00 03 03 01 00 00 04 04 07 00 03 20 01 00   chunk 0 of 10
13 04 0a 09 02  5a a5                                        chunk 9, the signature
```

Its length byte is the **raw** payload length — `0x0e` for a full chunk, `0x02`
for the tail. Only the `0x88` lighting family biases the length by `0x10`. Mix
them up and the firmware ignores the frame without complaint.

The mode bytes are the same ones documented under [Lighting mode](#lighting-mode).
A 70-second capture of the vendor's **rainbow** shows `[9] [10] = 00 03` and
**not one `0x88` packet** — the rainbow is a firmware effect the keyboard renders
itself, which is why it is perfectly smooth and costs no radio at all.

The trap for a host driver: leaving the board in an effect mode means the
firmware is painting keys at the same time you are. A solid colour visibly snaps
back a second later, and an animation fights the firmware for every frame.

**Do not "fix" that by writing this block.** The obvious next step — send the
wired `PER_KEY_CONFIG` through `0x04` and force per-key mode — was tried, and it
**stopped the keyboard typing; recovery took a firmware reflash.** Knowing that
the block is 128 bytes ending in `5A A5`, and that the wired block's mode bytes
sit at 9, 10 and 59, is not the same as knowing the two layouts are identical.
This is non-volatile storage on a device that also holds the key matrix, and
there is no known readback on this link to validate against.

Doing it safely needs, in order:

1. A **readback** for the config over `0x13`. The 20-byte interrupt IN reports
   are the likely carrier and are not yet decoded.
2. **Read-modify-write of only the mode bytes** — never a whole block sourced
   from the other transport.
3. `5A A5` validation before the write, exactly as the wired path does.

Until then, leave the board in whatever mode the user set. A firmware effect
repainting over host colours is a cosmetic problem; the alternative is not.


##### The wired and wireless config blocks are NOT the same

Captured from the vendor putting the board into per-key mode on each link, then
diffed byte for byte. Both are 128 bytes ending in `5A A5`, both carry the same
mode bytes at 9, 10 and 59 — and **8 of 128 bytes differ**:

| Byte | Wireless | Wired | |
| --- | --- | --- | --- |
| 14 | `01` | `00` | isolated flag in the header; plausibly "which link is live" |
| 89, 96, 97, 101, 114, 115, 117 | | | the effect table is shifted by one entry — wireless has an extra `09 47` |

Byte 14 is the dangerous one. Writing the wired block over the radio sets it to
`00`, which looks a great deal like telling a keyboard on 2.4 GHz that it is
plugged in. That is what was written, and the keyboard stopped typing; recovery
took a firmware reflash.

The lesson is not "use byte 14 = 01 instead". It is that **the first fourteen
bytes matching is not evidence the other 114 do**, and a block from one
transport is not a block for another. Even with these bytes in hand, writing
them is a non-volatile write of a config captured from *one* keyboard — byte 14
may well be per-device state rather than a constant.

If a user needs per-key mode on the receiver, the safe answer today is to let
the **vendor software** set it. It writes the correct block for its own
hardware, and keylux then streams colours into that mode without touching
config at all.


##### The frame-rate ceiling is arithmetic, not tuning

A full frame costs `4 bytes per distinct colour + 1 per lit key`, split into
14-byte chunks sent **13 ms apart** — the vendor's own measured minimum across
90 intra-message gaps (mean 14 ms). Going faster than the vendor is not a lever;
it is the key-scan starvation the wired path documents.

For an 80-key board:

| Colours | Bytes | Chunks | Ceiling |
| --- | --- | --- | --- |
| 1 (solid) | 84 | 6 | ~15 FPS |
| 8 | 112 | 8 | ~11 FPS at the vendor's 13 ms gap; ~21 FPS at 6 ms |
| 12 | 128 | 10 | ~8 FPS |
| 20 | 160 | 12 | ~7 FPS |

Space the writes 13 ms **apart**, not 13 ms of sleep between them. A control
transfer costs a couple of milliseconds itself, so sleeping the full gap and
then writing puts ~15 ms on the wire — slower than the vendor, whose 13 ms was
measured as observed spacing. Over a ten-chunk frame that pure overhead cost
more than a whole frame of latency. Verified by capturing this driver's own
output: min 13.0 ms, mean 13.1 ms, against the vendor's min 13.0, mean 14.0.

With that corrected, an 8-chunk frame measures 93.5 ms end to end, of which
91.7 ms is the seven gaps. The transmission IS the frame time; there is no
overhead left to find.

The 80 key indices are irreducible, so **colour count is the only lever** and
six chunks is a floor no full-board frame can beat. Roughly 1.1 kB/s against the
wired path's 8.2 kB/s, because wired ships a 378-byte framebuffer in a single
write where this needs ten sequential ones.

So a host-streamed full-board animation over the receiver is inherently around
8-11 FPS and will step visibly next to the same effect on the cable. That is the
link, not the driver. The vendor sidesteps it entirely: its smooth wireless
effects are **firmware** effects, selected by a config write and rendered on the
keyboard, with the radio idle.


##### The link's real limit is throughput, and it fails ugly

Measured by bisection: a full-board frame is 8 chunks, and streaming it at
**8 FPS (64 chunks/s) is clean**, while 12 FPS (96) and 16 FPS (128) visibly
fight — the board flickers between the frame being sent and the one it had,
because frames arrive faster than it can apply them.

For scale, the vendor's own streaming runs at about **14 chunks/s**: 1-2 chunks
every ~107 ms, and it never streams a whole board. That is not modesty, it is
the budget.

Budget **chunks per second, not frames**. The cost of a frame is its chunk
count, so a 2-chunk sparse effect can run four times as often as an 8-chunk
full-board one on the same link. Capping frames instead charges the cheap effect
the expensive one's price.

Two mistakes this cost, both worth avoiding:

- **The frame rate and the flicker are the same phenomenon.** Chasing FPS by
  shortening the inter-chunk gap reached 21 FPS full-board, matching the cable
  — and every frame of it fought the board. A frame-rate counter cannot see
  this; only looking at the keyboard can.
- **It is invisible in a capture.** The host's writes go out perfectly
  regularly, so the transmit side looks flawless while the board looks broken.

The practical consequence: **full-board animation belongs on the cable.** Over
the receiver, static colour and sparse effects work well and a whole-board
effect runs at single digits. Both of the vendor's wireless modes respect this
— music-reactive is sparse, and its smooth rainbow is a firmware effect with the
radio idle.

#### Command families seen

`0x88` lighting and `0x04` config are decoded. Still unknown: `0x09` (37-chunk
messages of mostly `00`/`ff`, ~2/s while an effect runs — plausibly a per-key
colour table for firmware effects), `0x44`, `0x07` and `0x05` (one each at
startup).

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

Starvation is not all-or-nothing, and the partial case is easy to misread. At
25 FPS the board lights perfectly and looks entirely healthy — but it starts
dropping the occasional keypress and reporting others twice. It reads as a
failing switch or a debounce problem, not as a lighting bug, so **treat any new
typing weirdness as a pacing regression first**. Keep at or below the vendor
driver's own floor of 44.7 ms; there is no visible benefit above it.

Two cheap ways to give the scan loop more room, both worth having:

- **Do not resend an unchanged frame.** A still image or a paused effect then
  costs nothing at all. Resend every couple of seconds anyway, so the board
  recovers by itself if the firmware ever repaints behind you.
- **Pausing should stop writing**, not stream black. Blanking the board and
  then holding it there is the full cost for none of the benefit.

### Over the 2.4 GHz receiver

None of the above is measured on the radio — there is no capture of the vendor
driver over the dongle. The link adds latency and retries on top of the same
8051 that still has to scan the key matrix, so the driver starts at **double the
wired gap (92 ms, ~10 FPS)** and treats that as a ceiling rather than a target.

The rule the code enforces is that a link may only ever be *gentler* than the
measured wired floor, never faster. Lowering the wireless gap needs the same
evidence the wired one has: a capture, or a careful typing test at each step
down. The partial-starvation failure — occasional dropped and doubled
keypresses while the lighting looks perfect — is just as available wirelessly,
and just as easy to misread as a failing switch.

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
- **Never write a config block you have not read back first.** This one was
  learned the hard way: the 2.4 GHz driver wrote the *wired* `PER_KEY_CONFIG`
  through the receiver's `0x04` command to force per-key mode, on the inference
  that both links share a byte-identical layout. Their first fourteen bytes do
  look alike, which is what made it tempting. **It stopped the keyboard typing
  and needed a firmware reflash to recover.**

  The config block is non-volatile storage on a device that also holds the key
  matrix. "The mode bytes are at offsets 9, 10 and 59" is knowledge about the
  *wired* block; nothing established that the wireless block is laid out the
  same, and there is no known readback on that link to check against. The wired
  path never had this bug because it reads, validates, then writes — do the
  same everywhere, or do not write at all.
- **Discovery is read-only.** It opens only collections the OS has not claimed,
  and identifies a board by *reading* its config block (`0x84`, a command the
  vendor driver issues itself). Never widen discovery to write commands, and
  never identify hardware by trying unknown command bytes — see the first bullet
  for what that risks.

## How a second device gets added

The same way this one was: capture the vendor driver with USBPcap while it
applies colours, then decode. Two notes that cost hours here:

Decode the result with:

```bash
cargo run --example parse_capture -- capture.pcap --min-len 8
```

It reads USBPcap's `.pcap` directly and assumes nothing about report IDs or
packet sizes, which matters because a receiver uses neither of the wired
protocol's. Its traffic-shape table is the answer at a glance: if the driver
drives colours over a device, there is a high-volume host-to-device shape wide
enough to hold them.

1. **Replug the device after starting the capture.** USBPcap hooks a device's
   stack when it *starts*; a device already running yields almost nothing.
2. **Capture both a static apply and an animated mode.** They use different
   commands, and you cannot infer one from the other.

3. **For a receiver, set the keyboard's side switch to 2.4 GHz first.** A
   capture taken while the cable is driving the board tells you nothing about
   the radio path, and the two are different USB devices.

## Using this

These are observations about how the hardware behaves, not an implementation.
Implement them freely in any project under any licence — including
[OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB), which is GPLv2 and for
which an F75 driver would be a genuinely useful addition. Nothing here needs
to be copied from this repository's Rust code, so its permissive licence is
not a constraint.
