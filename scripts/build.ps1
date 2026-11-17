# Builds the Windows installer (NSIS, per-user install - no admin rights needed).
# Output: $env:CARGO_TARGET_DIR\release\bundle\nsis\*.exe
param([switch]$WithWorkerExe)
. "$PSScriptRoot\env.ps1"
if (-not (Test-Path node_modules)) { npm ci }

& "$PSScriptRoot\test.ps1"
if ($LASTEXITCODE -ne 0) { throw "Tests failed - not packaging." }

if ($WithWorkerExe) {
  # Optional: package the Outlook worker as outlook_worker.exe so target PCs do not need Python.
  python -m pip install --user pyinstaller pywin32
  python -m PyInstaller --onefile --noconsole --name outlook_worker --distpath outlook_worker outlook_worker/outlook_worker.py
  $conf = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
  $conf.bundle.resources | Add-Member -NotePropertyName "../outlook_worker/outlook_worker.exe" -NotePropertyValue "outlook_worker/outlook_worker.exe" -Force
  $conf | ConvertTo-Json -Depth 20 | Set-Content src-tauri/tauri.conf.windows.json -Encoding utf8
  npx tauri build --config src-tauri/tauri.conf.windows.json
} else {
  npx tauri build
}
Write-Host "Installer: $env:CARGO_TARGET_DIR\release\bundle\nsis" -ForegroundColor Green
