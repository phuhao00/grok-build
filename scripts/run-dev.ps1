# Grok Build local dev launcher (Windows)
# Usage:
#   cd c:\Users\HHaou\grok-build
#   powershell -ExecutionPolicy Bypass -File .\scripts\run-dev.ps1

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$CargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $CargoBin) {
    $env:Path = "$CargoBin;$env:Path"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "cargo not found. Install Rust first:" -ForegroundColor Red
    Write-Host "  winget install Rustlang.Rustup"
    exit 1
}

$ProtocExe = Join-Path $RepoRoot ".tools\protoc\bin\protoc.exe"
if (Test-Path $ProtocExe) {
    $env:PROTOC = $ProtocExe
}

$env:CARGO_TARGET_DIR = Join-Path $RepoRoot "target"

Write-Host "Rust: $(rustc --version)"
Write-Host "Cargo target: $($env:CARGO_TARGET_DIR)"
if ($env:PROTOC) { Write-Host "PROTOC: $($env:PROTOC)" }

Write-Host ""
Write-Host "Building xai-grok-pager-bin (first build may take a while)..." -ForegroundColor Cyan
cargo build -p xai-grok-pager-bin
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Build failed." -ForegroundColor Yellow
    Write-Host "If you see os error 4551 (application control policy blocked):"
    Write-Host "  1. Run this script in Windows Terminal / PowerShell outside the IDE"
    Write-Host "  2. Or disable Smart App Control in Windows Security settings"
    Write-Host "  3. Or install the official binary:"
    Write-Host "       irm https://x.ai/cli/install.ps1 | iex"
    Write-Host "     then run: grok"
    exit $LASTEXITCODE
}

$Bin = Join-Path $env:CARGO_TARGET_DIR "debug\xai-grok-pager.exe"
Write-Host ""
Write-Host "Starting Grok Build TUI..." -ForegroundColor Green
Write-Host "First launch opens the browser for grok.com login."
Write-Host ""
& $Bin @args
