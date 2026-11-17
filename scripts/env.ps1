# Shared build environment. Dot-source from the other scripts.
# Toolchain: MSVC Rust (stable-x86_64-pc-windows-msvc) selected per project with `rustup override`,
# linked with Visual Studio 2022 Build Tools. See docs/BUILD_WINDOWS.md.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) { $env:PATH = "$cargoBin;$env:PATH" }
# Keep the (large) Rust build output out of OneDrive. Falcon has allowed builds from this folder.
# Always set (an inherited CARGO_TARGET_DIR from another project/older setup would otherwise win);
# set ASR_CARGO_TARGET_DIR to use a different folder on purpose.
$env:CARGO_TARGET_DIR = if ($env:ASR_CARGO_TARGET_DIR) { $env:ASR_CARGO_TARGET_DIR } else { Join-Path $env:LOCALAPPDATA "asr-support-desk-target-msvc" }
# Where build.ps1 copies the finished installer (local Documents folder, not the OneDrive one).
if (-not $env:ASR_INSTALLER_OUT) { $env:ASR_INSTALLER_OUT = Join-Path $env:USERPROFILE "Documents" }
# Tauri's cached NSIS toolchain.
$TauriNsisDir = Join-Path $env:LOCALAPPDATA "tauri\NSIS"
Set-Location $Root
