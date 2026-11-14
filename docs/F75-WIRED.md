# AULA F75 Wired: USB 1A2C:9407

Issue [#17](https://github.com/SibteProf/keylux/issues/17) reports an AULA F75
Wired, revision `0111`, described by Windows as USB Gaming Keyboard, using
`1a2c:9407`. Keylux recognizes this reported variant, but **does not yet support
its lighting protocol**. Per-key colors and custom animations are unavailable.

The native driver was verified on `258a:010c`. The shared F75 name and the fact
that the official application detects this variant do not establish compatible
commands, RGB capabilities, LED ordering, or streaming limits. Adding the new
ID to that driver would claim support without evidence. OpenRGB support for
other keyboards does not establish support for this variant either.

## Collect diagnostics from the release package

Extract the package, open PowerShell in its folder, and run:

```powershell
.\keylux.exe diagnose --device 1a2c:9407 --output f75-wired-diagnostics.txt
```

On Linux use `./keylux diagnose --device 1a2c:9407 --output f75-wired-diagnostics.txt`.
The command enumerates HID metadata: USB IDs, revision, interface numbers, usage
pages, product, and manufacturer. It does not open device handles, read input,
request feature reports, or send protocol commands. Serial numbers and HID
paths are omitted. No matching collections produces a report and a nonzero exit
status. Review the text before sharing it in #17.

Normal discovery, explicit ID selection, and deep receiver scans exclude this
variant from the Sinowealth protocol probe, including when it is allow-listed.
The GUI and `keylux devices --native` explain the unsupported variant when it
is connected. The device picker only includes keyboards that can be controlled.

## What is needed for a driver

A volunteer with this exact device needs to provide the diagnostic report,
the official software version/download source, and USBPcap captures of its
available lighting operations. Capture only this keyboard's USB traffic and
avoid typing sensitive information while recording. Useful separate captures:

1. Open the official software and read the current settings.
2. If per-key editing exists, change one known key to red, green, then blue.
3. Change the whole board to a solid color and restore it.
4. If the software streams host-generated effects, record a short animation.

The presence of a per-key editor or host streaming must be checked; fixed
firmware effects alone do not prove either capability. Do not try firmware
updates, guessed commands, the existing driver's writes, or random report IDs.
The implementation needs verified packet fixtures, a physical LED map, and
hardware tests for colors, typing reliability, reconnects, and write pacing
before this variant can be advertised as supported. Issue #17 remains open
until those requirements are met.
