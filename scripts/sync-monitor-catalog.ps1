# Scan workspace modules into bony-monitor/catalog/discovered.json
# Usage: powershell -ExecutionPolicy Bypass -File .\scripts\sync-monitor-catalog.ps1

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$OutDir = Join-Path $RepoRoot "crates\codegen\bony-monitor\catalog"
$OutFile = Join-Path $OutDir "discovered.json"

function Get-RelModules([string]$AbsDir, [string]$CrateName) {
    if (-not (Test-Path $AbsDir)) { return @() }
    Get-ChildItem -Path $AbsDir -Recurse -Filter *.rs -File | ForEach-Object {
        $rel = $_.FullName.Substring($RepoRoot.Length).TrimStart('\', '/').Replace('\', '/')
        [pscustomobject]@{
            path       = $rel
            crate_name = $CrateName
            stem       = $_.BaseName
        }
    }
}

$modules = @()
$modules += Get-RelModules (Join-Path $RepoRoot "crates\codegen\bony-build\src") "bony-build"
$modules += Get-RelModules (Join-Path $RepoRoot "crates\codegen\bony-monitor\src") "bony-monitor"

foreach ($script in @("scripts/run-desktop.ps1", "scripts/run-monitor.ps1")) {
    $p = Join-Path $RepoRoot ($script -replace '/', '\')
    if (Test-Path $p) {
        $modules += [pscustomobject]@{
            path       = $script
            crate_name = "scripts"
            stem       = [System.IO.Path]::GetFileNameWithoutExtension($script)
        }
    }
}

$modules = $modules | Sort-Object path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$payload = [ordered]@{
    generated_at = (Get-Date).ToString("o")
    modules      = @($modules)
}
$payload | ConvertTo-Json -Depth 6 | Set-Content -Path $OutFile -Encoding UTF8

Write-Host ("Synced {0} modules → {1}" -f $modules.Count, $OutFile) -ForegroundColor Green
