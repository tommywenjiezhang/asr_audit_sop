# Shared build environment. Dot-source from the other scripts.
# Uses a user-level Rust toolchain (rustup) and, if present, a portable MinGW under ~/tools/mingw64
# (GNU toolchain, no admin rights). With MSVC Build Tools installed, the default MSVC toolchain works too.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
$mingw = Join-Path $env:USERPROFILE "tools\mingw64\bin"
foreach ($p in @($cargoBin, $mingw)) { if ((Test-Path $p) -and ($env:PATH -notlike "*$p*")) { $env:PATH = "$p;$env:PATH" } }
# Keep the (large) Rust build output out of OneDrive.
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA "asr-support-desk-target" }
Set-Location $Root
