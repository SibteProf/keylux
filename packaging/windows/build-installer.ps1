param(
    [string]$Compiler,
    [string]$BinaryDir,
    [string]$OutputDir,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $BinaryDir) { $BinaryDir = Join-Path $repo 'target/release' }
if (-not $OutputDir) { $OutputDir = Join-Path $repo 'target/installer' }
if (-not $SkipBuild) {
    Push-Location $repo
    try {
        cargo build --release -p aula-app
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    } finally { Pop-Location }
}
if (-not $Compiler) {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 7/ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 6/ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 7/ISCC.exe')
    )
    $Compiler = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $Compiler) { $Compiler = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source }
}
if (-not $Compiler) { throw 'Install Inno Setup 6 or 7, or pass -Compiler with the path to ISCC.exe.' }
$manifest = Get-Content (Join-Path $repo 'Cargo.toml') -Raw
$version = [regex]::Match($manifest, '(?m)^version = "([^"]+)"').Groups[1].Value
if (-not $version) { throw 'Could not read workspace version' }
$versionFile = [IO.Path]::GetTempFileName()
try {
    $process = Start-Process -FilePath (Join-Path $BinaryDir 'keylux.exe') -ArgumentList '--version' -RedirectStandardOutput $versionFile -WindowStyle Hidden -PassThru -Wait
    $binaryVersion = (Get-Content $versionFile -Raw).Trim()
} finally { Remove-Item -LiteralPath $versionFile }
if ($process.ExitCode -ne 0 -or $binaryVersion -ne "keylux $version") {
    throw "Release binary does not match workspace version $version. Build the release first."
}
& $Compiler "/DAppVersion=$version" "/DSourceDir=$BinaryDir" "/DOutputDir=$OutputDir" (Join-Path $PSScriptRoot 'keylux.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
