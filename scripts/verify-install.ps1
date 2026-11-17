# Installs the newest built installer per-user, launches the app non-elevated and checks it.
#   scripts\verify-install.ps1                       # install + launch + DB check + Outlook connection check
#   scripts\verify-install.ps1 -OutlookDraftTest     # also saves ONE labelled test draft in Outlook (never sent)
#   scripts\verify-install.ps1 -Installer <path>     # a specific installer instead of the newest in %USERPROFILE%\Documents
#   scripts\verify-install.ps1 -NoInstall            # check the already-installed app only
# Run in a NORMAL (non-administrator) PowerShell with classic Outlook open. See docs/BUILD_WINDOWS.md.
param([string]$Installer, [switch]$NoInstall, [switch]$OutlookDraftTest)
. "$PSScriptRoot\env.ps1"
$ErrorActionPreference = "Continue"
$started = Get-Date
$failed = 0

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($admin) { throw "Run this from a normal (non-elevated) PowerShell - Outlook COM fails across elevation." }

if (-not $NoInstall) {
  if (-not $Installer) {
    $Installer = (Get-ChildItem $env:ASR_INSTALLER_OUT -Filter "ASR Audit Support Desk_*-setup.exe" -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
  }
  if (-not $Installer -or -not (Test-Path $Installer)) { throw "No installer found. Run scripts\build.ps1 -WithWorkerExe first." }
  Write-Host "== Install (per-user, silent) ==" -ForegroundColor Cyan
  Write-Host "  $Installer"
  Get-Process asr-support-desk -ErrorAction SilentlyContinue | Stop-Process -Confirm:$false
  # Do not use Start-Process -Wait: it also waits for processes the installer leaves running.
  $p = Start-Process -FilePath $Installer -ArgumentList "/S" -PassThru
  $p.WaitForExit()
  if ($p.ExitCode -ne 0) { throw "Installer exited with $($p.ExitCode)" }
}

$reg = Get-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*" -ErrorAction SilentlyContinue |
  Where-Object DisplayName -eq "ASR Audit Support Desk" | Select-Object -First 1
if (-not $reg) { throw "App is not installed (no uninstall entry under HKCU)." }
$installDir = $reg.InstallLocation.Trim('"')
$exe = Join-Path $installDir "asr-support-desk.exe"
Write-Host "  Installed: $installDir (version $($reg.DisplayVersion))" -ForegroundColor Green
Get-ChildItem -LiteralPath $exe, (Join-Path $installDir "outlook_worker") -ErrorAction SilentlyContinue |
  ForEach-Object { if ($_.PSIsContainer) { Get-ChildItem -LiteralPath $_.FullName } else { $_ } } |
  ForEach-Object { Write-Host ("    {0,-24} {1,12:N0} bytes  {2}" -f $_.Name, $_.Length, $_.LastWriteTime) }

Write-Host "== Launch (non-elevated, via explorer.exe) ==" -ForegroundColor Cyan
Get-Process asr-support-desk -ErrorAction SilentlyContinue | Stop-Process -Confirm:$false
Start-Process explorer.exe -ArgumentList "`"$exe`""
$proc = $null
for ($i = 0; $i -lt 30 -and -not ($proc -and $proc.MainWindowTitle); $i++) {
  Start-Sleep -Seconds 1
  $proc = Get-Process asr-support-desk -ErrorAction SilentlyContinue | Select-Object -First 1
}
if ($proc -and $proc.Responding) { Write-Host "  [ OK ] PID $($proc.Id), window '$($proc.MainWindowTitle)'" -ForegroundColor Green }
else { Write-Host "  [FAIL] app did not start or is not responding" -ForegroundColor Red; $failed++ }

# The log is written asynchronously; wait for a "backend ready" line from THIS launch (log times are UTC).
$launchUtc = $started.ToUniversalTime()
$ready = $null
for ($i = 0; $i -lt 20 -and -not $ready; $i++) {
  $ready = Select-String -Path "$env:APPDATA\gov.nj.doh.asrsupportdesk\logs\*.log" -Pattern "backend ready" -ErrorAction SilentlyContinue |
    Where-Object { $_.Line -match '^(\S+Z)' -and ([DateTime]::Parse($Matches[1]).ToUniversalTime() -ge $launchUtc) } | Select-Object -Last 1
  if (-not $ready) { Start-Sleep -Seconds 1 }
}
if ($ready) { Write-Host "  [ OK ] log: $($ready.Line.Trim())" -ForegroundColor Green }
else { Write-Host "  [FAIL] no 'backend ready' log line from this launch (SQLite/template setup may have failed)" -ForegroundColor Red; $failed++ }
Select-String -Path "$env:APPDATA\gov.nj.doh.asrsupportdesk\logs\*.log" -Pattern " ERROR " -ErrorAction SilentlyContinue |
  Where-Object { $_.Line -match '^(\S+Z)' -and ([DateTime]::Parse($Matches[1]).ToUniversalTime() -ge $launchUtc) } |
  Select-Object -Last 5 | ForEach-Object { Write-Host "  log: $($_.Line.Trim())" -ForegroundColor Yellow }

Write-Host "== Database / SOP templates (read-only) ==" -ForegroundColor Cyan
python -I "$PSScriptRoot\verify_install.py" db
if ($LASTEXITCODE -ne 0) { $failed++ }

Write-Host "== Outlook worker ==" -ForegroundColor Cyan
$worker = Join-Path $installDir "outlook_worker\outlook_worker.exe"
if (-not (Test-Path -LiteralPath $worker)) { $worker = Join-Path $installDir "outlook_worker\outlook_worker.py" }
if (-not (Get-Process OUTLOOK -ErrorAction SilentlyContinue)) {
  Write-Host "  [SKIP] classic Outlook is not running" -ForegroundColor Yellow
} else {
  $wargs = @("worker", $worker); if ($OutlookDraftTest) { $wargs += "--draft" }
  python -I "$PSScriptRoot\verify_install.py" @wargs
  if ($LASTEXITCODE -ne 0) { $failed++ }
}

$falcon = Get-WinEvent -LogName "CrowdStrike-Falcon Sensor-CSFalconService/Operational" -MaxEvents 50 -ErrorAction SilentlyContinue |
  Where-Object { $_.TimeCreated -ge $started -and $_.Id -in 3, 4 }
if ($falcon) { Write-Host "  [FAIL] CrowdStrike Falcon blocked/quarantined $(@($falcon).Count) item(s) during verification" -ForegroundColor Red; $failed++ }

Write-Host ""
Write-Host "Manual check still needed in the app window: Check Now -> open a ticket -> Copy Prompt (compare with preview) -> Open Outlook Reply -> Mark Done on a TEST email." -ForegroundColor Cyan
if ($failed) { Write-Host "$failed check(s) failed." -ForegroundColor Red; exit 1 }
Write-Host "All automated checks passed." -ForegroundColor Green
