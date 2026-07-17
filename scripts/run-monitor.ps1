# Start Bony Monitor (architecture + change impact dashboard).
# Usage: powershell -ExecutionPolicy Bypass -File .\scripts\run-monitor.ps1

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$CargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $CargoBin) {
    $env:Path = "$CargoBin;$env:Path"
}

$env:CARGO_TARGET_DIR = Join-Path $RepoRoot "target"

Write-Host "Syncing monitor catalog..." -ForegroundColor Cyan
powershell -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "sync-monitor-catalog.ps1")

Write-Host "Building bony-monitor..." -ForegroundColor Cyan
cargo run -p bony-monitor -- --repo $RepoRoot --bind 127.0.0.1:8787
