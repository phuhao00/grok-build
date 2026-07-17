# Build and run the Grok Desktop client.
# Usage: powershell -ExecutionPolicy Bypass -File .\scripts\run-desktop.ps1

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$CargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $CargoBin) {
    $env:Path = "$CargoBin;$env:Path"
}

# Ensure npm global `grok` is findable (GUI apps often miss Roaming\npm).
$NpmBins = @(
    (Join-Path $env:APPDATA "npm"),
    (Join-Path $env:LOCALAPPDATA "npm")
) | Where-Object { $_ -and (Test-Path $_) }
if ($NpmBins.Count -gt 0) {
    $env:Path = (($NpmBins -join ";") + ";" + $env:Path)
}

$ProtocExe = Join-Path $RepoRoot ".tools\protoc\bin\protoc.exe"
if (Test-Path $ProtocExe) {
    $env:PROTOC = $ProtocExe
}

$env:CARGO_TARGET_DIR = Join-Path $RepoRoot "target"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "cargo not found. Install Rust: winget install Rustlang.Rustup" -ForegroundColor Red
    exit 1
}

if (-not (Get-Command grok -ErrorAction SilentlyContinue)) {
    Write-Host "grok not found on PATH. Install with: npm i -g @xai-official/grok" -ForegroundColor Yellow
}

Write-Host "Building xai-grok-desktop..." -ForegroundColor Cyan
if ($args.Count -gt 0) {
    cargo run -p xai-grok-desktop -- @args
} else {
    cargo run -p xai-grok-desktop
}
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Build failed. On Windows os error 4551, disable Smart App Control" -ForegroundColor Yellow
    Write-Host "or run this script from Windows Terminal outside restricted sandboxes." -ForegroundColor Yellow
    exit $LASTEXITCODE
}
