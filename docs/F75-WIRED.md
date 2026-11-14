# AULA F75 Wired: USB 1A2C:9407

[Issue #17](https://github.com/SibteProf/keylux/issues/17) reports an AULA F75
Wired, revision `0111`, using `1a2c:9407`. Keylux detects this variant.
Per-key colors and custom animations are unavailable.

[NeilJohnnny's report](https://github.com/SibteProf/keylux/issues/17#issuecomment-6529837722)
links to the AULA F75 Wired Version 1.0.3.4 installer. The diagnostic lists two
vendor HID collections on interface 1, with usage pages `ff00` and `ff01`, both
with usage `0001`. It contains no lighting commands or evidence of per-key
color control.

The installer's `config.xml` associates `1a2c:9407` with the single-color F75
Wired and points to `KB-K102.xml`. That layout lists lighting modes and
brightness and speed settings. Whether those settings work on revision
`0111` still needs a hardware check. The native Keylux RGB driver was
verified on `258a:010c`.
[AULA describes the F75 Wired Version as having fixed backlighting](https://aulagear.com/products/aula-f75-wired-version).

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

## Check what the official app can control

If you own this variant, report which lighting controls in the AULA app
visibly change the keyboard. A screenshot of the lighting tab is enough to
start. If a control works, a short USBPcap capture while applying and restoring
that one setting would help identify the command. Capture only the keyboard's
USB traffic and avoid typing during the recording.

Per-key RGB would need evidence that the board can set different colors on
individual keys. Custom animations would also need evidence that it accepts
repeated frame updates from the PC. A brightness change or a built-in effect
would establish support for that specific control.
