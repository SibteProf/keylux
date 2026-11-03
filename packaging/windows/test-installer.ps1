# Runs only on a machine with no registered Keylux installation (for CI).
param([Parameter(Mandatory)][string]$Installer)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this installer smoke test from an elevated PowerShell session.'
}
$uninstallKey = 'HKLM:/Software/Microsoft/Windows/CurrentVersion/Uninstall/{B6C30245-3979-41B1-971D-4B87DA24C487}_is1'
if (Test-Path $uninstallKey) { throw 'Use a clean account: Keylux is already installed.' }
if (Test-Path ($uninstallKey -replace '^HKLM:', 'HKCU:')) { throw 'Use a clean account: a per-user Keylux installation exists.' }
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('keylux-setup-' + [guid]::NewGuid())
$installDir = Join-Path $testRoot 'Custom install folder'
$originalAppData = $env:APPDATA
$runKey = 'HKCU:/Software/Microsoft/Windows/CurrentVersion/Run'
$originalStartup = (Get-ItemProperty $runKey -Name Keylux -ErrorAction SilentlyContinue).Keylux
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
function Invoke-Setup([string[]]$SetupArgs) {
    $p = Start-Process -FilePath $installerPath -ArgumentList $SetupArgs -WindowStyle Hidden -PassThru -Wait
    if ($p.ExitCode -ne 0) { throw "Setup exit: $($p.ExitCode)" }
}
function Invoke-Keylux([string]$CliArgument) {
    $output = Join-Path $testRoot 'cli-output.txt'
    $p = Start-Process -FilePath (Join-Path $installDir 'keylux.exe') -ArgumentList $CliArgument -RedirectStandardOutput $output -WindowStyle Hidden -PassThru -Wait
    if ($p.ExitCode -ne 0) { throw "Installed CLI failed: $CliArgument" }
    Get-Content $output -Raw
}
try {
    $env:APPDATA = Join-Path $testRoot 'AppData'
    New-Item -ItemType Directory -Force $env:APPDATA | Out-Null
    New-Item -Path $runKey -Force | Out-Null
    Set-ItemProperty $runKey -Name Keylux -Value '"C:\Old portable folder\keylux.exe"'
    Invoke-Setup @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',('/DIR="' + $installDir + '"'),('/LOG="' + (Join-Path $testRoot 'install.log') + '"'))
    $exe = Join-Path $installDir 'keylux.exe'
    if (-not (Test-Path $exe)) { throw 'Custom installation folder was not used' }
    $version = (Invoke-Keylux '--version').Trim()
    if ((Get-ItemProperty $runKey).Keylux -ne ('"' + $exe + '"')) { throw 'Startup was not migrated' }
    $listing = Invoke-Keylux 'list'
    if ($listing -notmatch 'spiral') { throw 'Installed effects did not load' }
    $userEffects = Join-Path $env:APPDATA 'keylux/content/effects'
    $edited = Join-Path $userEffects 'breathe.rhai'
    Add-Content $edited '// preserved user edit'
    $hash = (Get-FileHash $edited).Hash
    $animations = Join-Path $env:APPDATA 'keylux/content/animations'
    Set-Content (Join-Path $animations 'user-file.txt') 'preserve me'
    Invoke-Setup @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART')
    if (-not (Test-Path $exe)) { throw 'Upgrade did not keep the previous directory' }
    $null = Invoke-Keylux 'list'
    if ((Get-FileHash $edited).Hash -ne $hash) { throw 'Upgrade changed user content' }
    $uninstaller = Join-Path $installDir 'unins000.exe'
    $p = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART' -WindowStyle Hidden -PassThru -Wait
    if ($p.ExitCode -ne 0) { throw 'Uninstall failed' }
    # The uninstaller delegates cleanup to a temporary process.
    for ($attempt = 0; $attempt -lt 30 -and (Test-Path $exe); $attempt++) { Start-Sleep -Milliseconds 200 }
    if ((Test-Path $exe) -or (Test-Path $uninstallKey)) { throw 'Uninstall left application registration or binary' }
    if ((Get-FileHash $edited).Hash -ne $hash -or -not (Test-Path (Join-Path $animations 'user-file.txt'))) { throw 'Uninstall removed user content' }
    if ((Get-ItemProperty $runKey -Name Keylux -ErrorAction SilentlyContinue).Keylux) { throw 'Uninstall left its startup entry' }
    Write-Output "PASS: $version custom path, content seeding, startup migration, upgrade, uninstall, user content preservation. Logs: $testRoot"
} finally {
    $env:APPDATA = $originalAppData
    if ($null -ne $originalStartup) { Set-ItemProperty $runKey -Name Keylux -Value $originalStartup }
    else { Remove-ItemProperty $runKey -Name Keylux -ErrorAction SilentlyContinue }
}
