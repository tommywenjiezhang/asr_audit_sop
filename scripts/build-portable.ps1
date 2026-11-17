# Builds the PORTABLE distribution: dist\ASR-Audit-Support-Desk-<version>-windows-x64.zip (+ .sha256).
# Recipients extract the ZIP anywhere and run "ASR Audit Support Desk.exe" - no installer, no admin rights,
# no Rust/Node/Python. The NSIS installer (scripts\build.ps1) is unchanged and can still be built.
#
#   scripts\build-portable.ps1               # tests -> worker exe -> release exe -> stage -> ZIP + SHA-256
#   scripts\build-portable.ps1 -SkipTests    # packaging only (not for releases)
#
# ZIP layout (one top folder so "Extract All" creates it):
#   ASR-Audit-Support-Desk-<ver>-windows-x64\
#     ASR Audit Support Desk.exe            the app (SOP templates, catalog, SOP images and UI are embedded)
#     outlook_worker\outlook_worker.exe     Outlook COM worker (PyInstaller; contains Python + pywin32)
#     README.txt, MANIFEST.txt, ASR-Support-Desk-User-Guide.html
# The app finds the worker next to itself (resource dir = exe folder), so any extract path works, spaces included.
# User data stays in %APPDATA%\gov.nj.doh.asrsupportdesk (shared with the installed version, kept on update).
# Run in a NORMAL (non-administrator) PowerShell. See docs/BUILD_WINDOWS.md.
param([switch]$SkipTests)
. "$PSScriptRoot\env.ps1"
. "$PSScriptRoot\worker-exe.ps1"
$started = Get-Date

# ---- version (from the Tauri config; the other manifests must agree)
$version = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
$pkgVersion = (Get-Content package.json -Raw | ConvertFrom-Json).version
$cargoVersion = (Select-String -Path src-tauri\Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if ($version -ne $pkgVersion -or $version -ne $cargoVersion) {
  throw "Version mismatch: tauri.conf.json=$version package.json=$pkgVersion Cargo.toml=$cargoVersion. Make them equal first."
}
$name = "ASR-Audit-Support-Desk-$version-windows-x64"
Write-Host "Building $name" -ForegroundColor Cyan

if (-not (Test-Path node_modules)) { npm ci }

if (-not $SkipTests) {
  & "$PSScriptRoot\test.ps1"
  if ($LASTEXITCODE -ne 0) { throw "Tests failed - not packaging." }
}

# ---- 1. Outlook worker exe (same build as the installer)
$worker = Build-WorkerExe

# ---- 2. release app exe (frontend + Rust); no installer bundle
npx tauri build --no-bundle
if ($LASTEXITCODE -ne 0) { throw "tauri build --no-bundle failed." }
$appExe = Get-Item (Join-Path $env:CARGO_TARGET_DIR "release\asr-support-desk.exe")
if ($appExe.LastWriteTime -lt $started) { throw "Release exe was not rebuilt by this run: $($appExe.FullName)" }

# ---- 3. stage (outside OneDrive, in the cargo target folder; only this script's own folder is replaced)
$stageRoot = Join-Path $env:CARGO_TARGET_DIR "portable"
$stage = Join-Path $stageRoot $name
if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force -Confirm:$false }
New-Item -ItemType Directory -Force (Join-Path $stage "outlook_worker") | Out-Null
Copy-Item -LiteralPath $appExe.FullName -Destination (Join-Path $stage "ASR Audit Support Desk.exe")
Copy-Item -LiteralPath $worker -Destination (Join-Path $stage "outlook_worker\outlook_worker.exe")
$guide = "docs\guide\ASR-Support-Desk-User-Guide.html"
if (Test-Path $guide) { Copy-Item $guide (Join-Path $stage "ASR-Support-Desk-User-Guide.html") }
else { Write-Host "Note: user guide not found ($guide); run scripts\build-guide.ps1 to include it." -ForegroundColor Yellow }
$build = "{0} ({1})" -f (Get-Date -Format "yyyy-MM-dd HH:mm"), ((git rev-parse --short HEAD 2>$null) -join "")
$readme = (Get-Content "$PSScriptRoot\portable\README.txt" -Raw).Replace("{{VERSION}}", $version).Replace("{{BUILD}}", $build)
[IO.File]::WriteAllText((Join-Path $stage "README.txt"), $readme.Replace("`r`n", "`n").Replace("`n", "`r`n"), (New-Object Text.UTF8Encoding $false))

# ---- 4. only expected files may be packaged (no databases, settings, logs, tickets, email content, test data)
$allowed = @("ASR Audit Support Desk.exe", "outlook_worker\outlook_worker.exe", "README.txt", "ASR-Support-Desk-User-Guide.html")
$files = Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object { $_.FullName.Substring($stage.Length + 1) }
$unexpected = $files | Where-Object { $_ -notin $allowed }
if ($unexpected) { throw "Unexpected files in the package: $($unexpected -join ', ')" }
$manifest = @("ASR/Audit Support Desk $version - portable package contents", "Built $build", "", "SHA-256                                                           Size (bytes)  File")
foreach ($f in $files) {
  $item = Get-Item -LiteralPath (Join-Path $stage $f)
  $manifest += "{0}  {1,12}  {2}" -f (Get-FileHash -LiteralPath $item.FullName).Hash, $item.Length, $f
}
[IO.File]::WriteAllText((Join-Path $stage "MANIFEST.txt"), ($manifest -join "`r`n") + "`r`n", (New-Object Text.UTF8Encoding $false))

# ---- 5. ZIP + SHA-256 into dist\
New-Item -ItemType Directory -Force "dist" | Out-Null
$zip = Join-Path (Resolve-Path "dist") "$name.zip"
if (Test-Path $zip) { Remove-Item -LiteralPath $zip -Force -Confirm:$false }
Compress-Archive -LiteralPath $stage -DestinationPath $zip -CompressionLevel Optimal
$hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
[IO.File]::WriteAllText("$zip.sha256", "$hash *$name.zip`r`n", (New-Object Text.UTF8Encoding $false))

# ---- 6. report (and any CrowdStrike Falcon blocks during this build - read-only check)
$falcon = Get-WinEvent -LogName "CrowdStrike-Falcon Sensor-CSFalconService/Operational" -MaxEvents 50 -ErrorAction SilentlyContinue |
  Where-Object { $_.TimeCreated -ge $started -and $_.Id -in 3, 4 }
if ($falcon) {
  Write-Host "WARNING: CrowdStrike Falcon blocked/quarantined $(@($falcon).Count) item(s) during this build. Do not work around this - see docs/BUILD_WINDOWS.md." -ForegroundColor Red
}
$z = Get-Item -LiteralPath $zip
Write-Host ""
Write-Host "Portable ZIP : $($z.FullName)" -ForegroundColor Green
Write-Host ("Size         : {0:N0} bytes ({1:N2} MiB)" -f $z.Length, ($z.Length / 1MB))
Write-Host "SHA-256      : $hash"
Write-Host "Checksum file: $zip.sha256"
Write-Host "Staged folder: $stage"
