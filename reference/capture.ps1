# Capture the AULA F75 vendor driver's USB traffic.
#
# MUST be run as Administrator: USBPcap needs the driver service started and
# raw capture privileges.
#
#   Right-click PowerShell > Run as Administrator, then:
#     cd "D:\Coding\aula f75"
#     powershell -ExecutionPolicy Bypass -File .\capture.ps1
#
# While it counts down, use the AULA driver to set a few keys to distinct
# colours (pure red on Esc, pure green on Q, pure blue on Space) and click
# APPLY. Distinct colours on known keys let us decode channel order and the
# LED index map from the same capture.

param(
    [int]$Seconds = 120,
    [string]$Out = "capture3.pcapng"
)

$ErrorActionPreference = "Stop"

$tshark = "C:\Program Files\Wireshark\tshark.exe"
if (-not (Test-Path $tshark)) { throw "tshark not found at $tshark" }

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "ERROR: not elevated. Re-run this from an Administrator PowerShell." -ForegroundColor Red
    exit 1
}

Write-Host "Starting USBPcap driver service..." -ForegroundColor Cyan
try { Start-Service USBPcap -ErrorAction Stop; Write-Host "  started." }
catch { Write-Host "  could not start service: $($_.Exception.Message)" -ForegroundColor Yellow }

# USBPcap exposes one control device per USB root hub, as \\.\USBPcapN.
$ifaces = & $tshark -D 2>&1 | Select-String -Pattern "USBPcap" | ForEach-Object {
    if ($_.Line -match "(\\\\\.\\USBPcap\d+)") { $Matches[1] }
}
$ifaces = $ifaces | Select-Object -Unique

if (-not $ifaces) {
    Write-Host ""
    Write-Host "No USBPcap interfaces found." -ForegroundColor Red
    Write-Host "Wireshark is installed but the USBPcap driver is not active."
    Write-Host "Re-run the Wireshark installer and tick USBPcap, then REBOOT."
    exit 1
}

Write-Host ""
Write-Host "Capturing from: $($ifaces -join ', ')" -ForegroundColor Cyan

# Capture every USB root hub at once so we cannot pick the wrong one.
$tsArgs = @()
foreach ($i in $ifaces) { $tsArgs += @("-i", $i) }
$tsArgs += @("-a", "duration:$Seconds", "-w", $Out)

Write-Host ""
Write-Host "=============================================================" -ForegroundColor Green
Write-Host " RECORDING FOR $Seconds SECONDS - DO THIS IN ORDER:" -ForegroundColor Green
Write-Host "" -ForegroundColor Green
Write-Host "   1. UNPLUG the keyboard's USB-C cable, wait 2s, PLUG IT BACK" -ForegroundColor Yellow
Write-Host "      (essential: USBPcap only captures a device fully if it" -ForegroundColor Yellow
Write-Host "       starts AFTER the capture began)" -ForegroundColor Yellow
Write-Host "   2. Wait ~5s for Windows to re-enumerate it" -ForegroundColor Green
Write-Host "   3. In the AULA driver, start an ANIMATED effect that streams" -ForegroundColor Green
Write-Host "      from the PC - a music / audio-reactive mode is ideal." -ForegroundColor Green
Write-Host "      Let it run and visibly animate for ~30 seconds." -ForegroundColor Green
Write-Host "   4. Let the capture run out." -ForegroundColor Green
Write-Host "=============================================================" -ForegroundColor Green
Write-Host ""

Write-Host "Running: tshark $($tsArgs -join ' ')" -ForegroundColor DarkGray
Write-Host ""
& $tshark @tsArgs

if (Test-Path $Out) {
    $size = (Get-Item $Out).Length
    Write-Host ""
    Write-Host ("Saved {0} ({1:N0} bytes)" -f $Out, $size) -ForegroundColor Cyan

    # Verify we actually captured USB, not network traffic. A previous version
    # of this script silently fell back to the default interface (Wi-Fi) and
    # produced a capture with no USB in it at all.
    $enc = & $tshark -r $Out -T fields -e frame.protocols -c 20 2>$null
    $usbFrames = ($enc | Where-Object { $_ -match "usb" }).Count
    if ($usbFrames -gt 0) {
        Write-Host "Verified: USB traffic present." -ForegroundColor Green
        Write-Host "Capture complete. Decode it with: npx tsx src/parse-capture.ts" -ForegroundColor Cyan
    } else {
        Write-Host "WARNING: no USB frames found - this is not a USB capture." -ForegroundColor Red
        Write-Host "First protocols seen: $($enc | Select-Object -First 3)" -ForegroundColor Yellow
    }
    # Count the 520-byte report transfers this protocol uses. The last capture
    # had only 2, which is why it was useless.
    $big = (& $tshark -r $Out -Y "usb.data_len > 400 && usb.data_len < 600" -T fields -e frame.number 2>$null | Measure-Object).Count
    Write-Host ("520-byte-ish transfers in capture: {0}" -f $big) -ForegroundColor Cyan
    if ($big -lt 5) {
        Write-Host "Very few large transfers - the colour writes were probably missed again." -ForegroundColor Yellow
        Write-Host "Did you REPLUG the keyboard after the capture started?" -ForegroundColor Yellow
    }
} else {
    Write-Host "Capture file was not created." -ForegroundColor Red
}


