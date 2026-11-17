# Checks (and with -Fix, repairs the user-level parts of) the Windows build environment.
#   scripts\doctor.ps1        # report only
#   scripts\doctor.ps1 -Fix   # also install the MSVC Rust toolchain + set the project override (no admin needed)
# It never changes system settings, security software, or anything that needs elevation.
# See docs/BUILD_WINDOWS.md.
param([switch]$Fix)
. "$PSScriptRoot\env.ps1"
$ErrorActionPreference = "Continue"
$Toolchain = "stable-x86_64-pc-windows-msvc"
$script:problems = 0
function Pass($m) { Write-Host "  [ OK ] $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:problems++ }

Write-Host "== Session ==" -ForegroundColor Cyan
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($admin) { Fail "PowerShell is elevated. Build and test from a normal PowerShell (Outlook COM fails across elevation: 0x80080005)." } else { Pass "Not elevated" }
Pass "Project root: $Root"
if ($Root -like "$env:WINDIR*") { Fail "Project root is under the Windows folder" }

Write-Host "== Rust ==" -ForegroundColor Cyan
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
  Fail "rustup not found. Install from https://rustup.rs (user-level), then reopen PowerShell."
} else {
  $toolchains = rustup toolchain list
  if (-not ($toolchains -match [regex]::Escape($Toolchain))) {
    if ($Fix) { rustup toolchain install $Toolchain --profile minimal; $toolchains = rustup toolchain list }
  }
  if ($toolchains -match [regex]::Escape($Toolchain)) { Pass "$Toolchain installed" } else { Fail "$Toolchain missing (run with -Fix)" }
  Pass "Global default: $(rustup default)  (left unchanged on purpose)"
  foreach ($dir in @($Root, (Join-Path $Root "src-tauri"))) {
    Push-Location $dir
    $active = (rustup show active-toolchain) -join " "
    if ($active -notmatch "msvc" -and $Fix) { rustup override set $Toolchain | Out-Null; $active = (rustup show active-toolchain) -join " " }
    if ($active -match "msvc") { Pass "Active in $(Split-Path $dir -Leaf): $active" } else { Fail "Active in $(Split-Path $dir -Leaf) is not MSVC: $active (run with -Fix)" }
    Pop-Location
  }
  Push-Location (Join-Path $Root "src-tauri")
  Pass "$(rustc --version) / $(cargo --version)"
  Pop-Location
}

Write-Host "== MSVC Build Tools / Windows SDK ==" -ForegroundColor Cyan
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = if (Test-Path $vswhere) { & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
if (-not $vs) {
  Fail "Visual Studio Build Tools with 'Desktop development with C++' not found. Install from Software Center (winget is disabled by policy)."
} else {
  Pass "Build Tools: $vs"
  $link = Get-ChildItem "$vs\VC\Tools\MSVC\*\bin\Hostx64\x64\link.exe" -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | Select-Object -First 1
  if ($link) { Pass "MSVC linker: $($link.FullName)" } else { Fail "MSVC link.exe (Hostx64\x64) not found" }
}
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\Lib" -Directory -ErrorAction SilentlyContinue |
  Where-Object { Test-Path (Join-Path $_.FullName "um\x64\kernel32.lib") } | Sort-Object Name -Descending | Select-Object -First 1
if ($sdk) { Pass "Windows SDK: $($sdk.Name)" } else { Fail "Windows 10/11 SDK (x64 libs) not found" }
$pathLink = (Get-Command link.exe -ErrorAction SilentlyContinue).Source
if ($pathLink -and $pathLink -notlike "*Microsoft Visual Studio*") {
  Warn "PATH has a non-MSVC link.exe first ($pathLink). rustc still finds the MSVC linker itself; only matters if you call link.exe by hand."
}
$pathCmd = (Get-Command cmd -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
if ($pathCmd -and $pathCmd -notlike "$env:WINDIR*") { Warn "PATH resolves 'cmd' to $pathCmd. Use `$env:WINDIR\System32\cmd.exe explicitly." }

Write-Host "== Node / Python ==" -ForegroundColor Cyan
if (Get-Command node -ErrorAction SilentlyContinue) { Pass "Node $(node --version)" } else { Fail "Node.js not found (20+ required)" }
if (Test-Path (Join-Path $Root "node_modules")) { Pass "node_modules present" } else { Warn "node_modules missing (build.ps1 runs npm ci)" }
if (Get-Command python -ErrorAction SilentlyContinue) {
  Pass "$(python --version 2>&1)"
  python -c "import win32com.client" 2>$null
  if ($LASTEXITCODE -eq 0) { Pass "pywin32 importable" } else { Warn "pywin32 not installed (build.ps1 -WithWorkerExe installs it with pip --user)" }
} else { Fail "Python not found (3.9+ required)" }

Write-Host "== Output folders ==" -ForegroundColor Cyan
Pass "CARGO_TARGET_DIR  = $env:CARGO_TARGET_DIR"
if ($env:CARGO_TARGET_DIR -like "*OneDrive*") { Warn "CARGO_TARGET_DIR is inside OneDrive - move it to %LOCALAPPDATA%" }
Pass "Installer copy to = $env:ASR_INSTALLER_OUT"
if (Test-Path $TauriNsisDir) { Pass "Tauri NSIS cache ready: $TauriNsisDir" }
elseif (Get-ChildItem (Split-Path $TauriNsisDir) -Directory -Filter "nsis-*" -ErrorAction SilentlyContinue) { Warn "Half-finished Tauri NSIS cache (nsis-* not renamed to NSIS). build.ps1 repairs this automatically." }
else { Pass "Tauri NSIS cache not downloaded yet (first build downloads it)" }

Write-Host "== CrowdStrike Falcon (read-only, last 24 h) ==" -ForegroundColor Cyan
$ev = Get-WinEvent -LogName "CrowdStrike-Falcon Sensor-CSFalconService/Operational" -MaxEvents 200 -ErrorAction SilentlyContinue |
  Where-Object { $_.TimeCreated -gt (Get-Date).AddHours(-24) -and $_.Id -in 3, 4 }
if (-not (Get-Service CSFalconService -ErrorAction SilentlyContinue)) { Pass "Falcon sensor not present" }
elseif ($ev) {
  Warn "$(@($ev).Count) block/quarantine event(s) in the last 24 h (most recent $($ev[0].TimeCreated)). If they line up with a build, see docs/BUILD_WINDOWS.md."
} else { Pass "No block/quarantine events in the last 24 h" }

Write-Host ""
if ($script:problems) { Write-Host "$script:problems problem(s) found." -ForegroundColor Red; exit 1 }
Write-Host "Environment ready. Next: scripts\build.ps1 -WithWorkerExe" -ForegroundColor Green
