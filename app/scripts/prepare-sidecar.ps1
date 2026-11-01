<#
.SYNOPSIS
  Build githop.exe (release) and copy it to where the Tauri bundler expects the
  sidecar: app/src-tauri/binaries/githop-<target-triple>.exe.

.DESCRIPTION
  The installer ships the CLI next to GitHopApp.exe (`bundle.externalBin` in
  app/src-tauri/tauri.release.conf.json). Tauri looks for the file with the
  Rust target triple appended and strips the suffix when bundling, so the
  installed file is plain `githop.exe`.

  Run from anywhere:
    powershell -NoProfile -ExecutionPolicy Bypass -File app/scripts/prepare-sidecar.ps1
  then:
    cd app; npx tauri build --config src-tauri/tauri.release.conf.json
  (`npm run build:installer` in app/ does both.)

  Local only: runs cargo and copies one file. No downloads beyond cargo's own
  crate fetches.

.PARAMETER Target
  Rust target triple. Default: the host triple reported by rustc.

.PARAMETER SkipBuild
  Copy an existing target/<triple>/release/githop.exe without building.
#>
param(
  [string]$Target = "",
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

if (-not $Target) {
  $hostLine = (& rustc -vV) | Where-Object { $_ -like "host:*" }
  if (-not $hostLine) { throw "Couldn't read the host target from 'rustc -vV'." }
  $Target = ($hostLine -replace "^host:\s*", "").Trim()
}

if (-not $SkipBuild) {
  & cargo build --release -p githop-cli --target $Target --manifest-path (Join-Path $repo "Cargo.toml")
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)." }
}

$source = Join-Path $repo "target\$Target\release\githop.exe"
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
  throw "Not found: $source. Build it first or drop -SkipBuild."
}

$binDir = Join-Path $repo "app\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$dest = Join-Path $binDir "githop-$Target.exe"
Copy-Item -LiteralPath $source -Destination $dest -Force
Write-Host "Sidecar ready: $dest"
