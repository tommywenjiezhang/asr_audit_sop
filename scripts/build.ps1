# Builds the Windows installer (NSIS, per-user install - no admin rights needed).
#   scripts\build.ps1                  # tests + installer (worker runs from outlook_worker.py, needs Python on target PC)
#   scripts\build.ps1 -WithWorkerExe   # also bundles outlook_worker.exe (PyInstaller) - target PCs need no Python
#   scripts\build.ps1 -SkipTests       # packaging only (not for release builds)
#   scripts\build.ps1 -OutputDir D:\x  # copy the installer somewhere other than %USERPROFILE%\Documents
# Run in a NORMAL (non-administrator) PowerShell. See docs/BUILD_WINDOWS.md.
param([switch]$WithWorkerExe, [switch]$SkipTests, [string]$OutputDir)
. "$PSScriptRoot\env.ps1"
if (-not $OutputDir) { $OutputDir = $env:ASR_INSTALLER_OUT }
$started = Get-Date

function Repair-TauriNsisCache {
  # Tauri downloads NSIS, extracts it to %LOCALAPPDATA%\tauri\nsis-3.xx and renames that folder to NSIS.
  # On this PC the rename can fail with "Access is denied" while freshly extracted files are still open
  # (scanner handles). Finishing the rename ourselves is all that is needed.
  if (Test-Path $TauriNsisDir) { return $false }
  $extracted = Get-ChildItem (Split-Path $TauriNsisDir) -Directory -Filter "nsis-*" -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $extracted) { return $false }
  Write-Host "Completing Tauri NSIS cache setup: $($extracted.Name) -> NSIS" -ForegroundColor Yellow
  Rename-Item $extracted.FullName "NSIS"
  return $true
}

function Invoke-TauriBuild([string[]]$tauriArgs) {
  npx tauri build @tauriArgs
  if ($LASTEXITCODE -eq 0) { return }
  if (Repair-TauriNsisCache) {
    Write-Host "Retrying tauri build after NSIS cache repair..." -ForegroundColor Yellow
    npx tauri build @tauriArgs
    if ($LASTEXITCODE -eq 0) { return }
  }
  throw "tauri build failed (exit $LASTEXITCODE) - no installer produced. Run scripts\doctor.ps1 and check docs/BUILD_WINDOWS.md (Troubleshooting)."
}

if (-not (Test-Path node_modules)) { npm ci }
[void](Repair-TauriNsisCache)

if (-not $SkipTests) {
  & "$PSScriptRoot\test.ps1"
  if ($LASTEXITCODE -ne 0) { throw "Tests failed - not packaging." }
}

if ($WithWorkerExe) {
  # Package the Outlook worker as outlook_worker.exe so target PCs do not need Python.
  . "$PSScriptRoot\worker-exe.ps1"
  [void](Build-WorkerExe)
  $conf = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
  $conf.bundle.resources | Add-Member -NotePropertyName "../outlook_worker/outlook_worker.exe" -NotePropertyValue "outlook_worker/outlook_worker.exe" -Force
  $conf | ConvertTo-Json -Depth 20 | Set-Content src-tauri/tauri.conf.windows.json -Encoding utf8
  Invoke-TauriBuild @("--config", "src-tauri/tauri.conf.windows.json")
} else {
  Invoke-TauriBuild @()
}

# Pick the installer produced by THIS run (never an older leftover).
$bundleDir = Join-Path $env:CARGO_TARGET_DIR "release\bundle\nsis"
$installer = Get-ChildItem $bundleDir -Filter "*-setup.exe" | Where-Object LastWriteTime -ge $started | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { throw "tauri build reported success but no new *-setup.exe was found in $bundleDir" }

New-Item -ItemType Directory -Force $OutputDir | Out-Null
$dest = Join-Path $OutputDir $installer.Name
Copy-Item $installer.FullName $dest -Force
$out = Get-Item $dest

# Report any CrowdStrike Falcon blocks that happened during this build (read-only check).
$falcon = Get-WinEvent -LogName "CrowdStrike-Falcon Sensor-CSFalconService/Operational" -MaxEvents 50 -ErrorAction SilentlyContinue |
  Where-Object { $_.TimeCreated -ge $started -and $_.Id -in 3, 4 }
if ($falcon) {
  Write-Host "WARNING: CrowdStrike Falcon blocked/quarantined $(@($falcon).Count) item(s) during this build." -ForegroundColor Red
  Write-Host "Do not work around this - see docs/BUILD_WINDOWS.md, 'CrowdStrike Falcon'." -ForegroundColor Red
}

Write-Host ""
Write-Host "Installer : $($out.FullName)" -ForegroundColor Green
Write-Host ("Size      : {0:N0} bytes ({1:N2} MiB)" -f $out.Length, ($out.Length / 1MB))
Write-Host "Built     : $($installer.LastWriteTime)"
Write-Host "SHA256    : $((Get-FileHash $out.FullName).Hash)"
Write-Host "Original  : $($installer.FullName)"
