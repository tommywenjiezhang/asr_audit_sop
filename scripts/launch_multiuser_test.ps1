<#
.SYNOPSIS
  Launch two independent ASR/Audit Support Desk instances on this computer for multiuser testing.

.DESCRIPTION
  Each instance ("Workstation A" and "Workstation B") gets its own data folder, which holds its own
  local SQLite database, settings.json, device id, "I am" staff identity, sync queue, logs, backups and
  WebView2 cache. Both point at the SAME shared Access database. Outlook is replaced by a shared mock
  mailbox (no real email is moved, flagged or sent) unless -Outlook live is given explicitly.

  Environment variables are set for each child process only (this window's environment is restored).
  Run from a normal (non-administrator) PowerShell.

.EXAMPLE
  scripts\launch_multiuser_test.ps1                      # build output exe, C:\ASR_Dev, team default database
  scripts\launch_multiuser_test.ps1 -Reset               # fresh profiles (local test folders only)
  scripts\launch_multiuser_test.ps1 -SharedDb D:\test\asr_test.accdb -SyncMinutes 1
  scripts\launch_multiuser_test.ps1 -Stop                 # close both test instances
#>
[CmdletBinding()]
param(
    [string]$Exe = "",
    [string]$Root = "C:\ASR_Dev",
    [string]$SharedDb = "\\epi-qb-5\VPD_Shared\PgmDocs\Assessment and Coverage\Annual Immunization Status Report\ASR Master List Revamp--2023\asr_email_support.accdb",
    [string]$StaffA = "Test Staff A",
    [string]$StaffB = "Test Staff B",
    [int]$PortA = 9301,
    [int]$PortB = 9302,
    [ValidateSet("mock", "live")][string]$Outlook = "mock",
    [int]$SyncMinutes = 1,
    [string]$MailboxName = "",
    [switch]$Reset,
    [switch]$NoLaunch,
    [switch]$Stop,
    # launch / stop only this profile (WorkstationA or WorkstationB), e.g. to restart one instance
    [string]$Only = ""
)
$ErrorActionPreference = "Stop"

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Warning "This PowerShell runs as administrator. Outlook COM fails across elevation levels; use a normal window."
}

$stateFile = Join-Path $Root "instances.json"
if ($Stop) {
    if (Test-Path $stateFile) {
        # (parentheses: Windows PowerShell 5.1 passes a JSON array down the pipeline as ONE object)
        foreach ($i in (@((Get-Content $stateFile -Raw | ConvertFrom-Json)) | Where-Object { -not $Only -or $_.name -eq $Only })) {
            try { Stop-Process -Id $i.pid -ErrorAction Stop; Write-Host "Stopped $($i.name) (pid $($i.pid))" } catch { Write-Host "$($i.name) (pid $($i.pid)) was not running" }
        }
    }
    return
}

# ---------------------------------------------------------------- executable
if (-not $Exe) {
    $target = if ($env:ASR_CARGO_TARGET_DIR) { $env:ASR_CARGO_TARGET_DIR } else { Join-Path $env:LOCALAPPDATA "asr-support-desk-target-msvc" }
    $candidates = @((Join-Path $target "release\asr-support-desk.exe"), (Join-Path $target "debug\asr-support-desk.exe"))
    $Exe = $candidates | Where-Object { Test-Path $_ } | Sort-Object { (Get-Item $_).LastWriteTime } -Descending | Select-Object -First 1
    if (-not $Exe) { throw "No built app found. Build it first: npx tauri build --debug --no-bundle (or scripts\build.ps1), or pass -Exe <path to the installed ASR Audit Support Desk.exe>." }
}
if (-not (Test-Path $Exe)) { throw "Executable not found: $Exe" }
Write-Host "App: $Exe" -ForegroundColor Cyan

# ---------------------------------------------------------------- shared database
if ($SharedDb.ToLower().Contains("onedrive")) { throw "The shared database must not be in a OneDrive folder: $SharedDb" }
if (-not (Test-Path -LiteralPath $SharedDb)) { Write-Warning "Shared database not reachable right now: $SharedDb (instances start offline and save locally)." }

# ---------------------------------------------------------------- profiles
$mock = Join-Path $Root "OutlookMock"
$profiles = @(
    @{ name = "WorkstationA"; staff = $StaffA; port = $PortA },
    @{ name = "WorkstationB"; staff = $StaffB; port = $PortB }
)
if ($Reset) {
    if (Get-Process -Name "asr-support-desk" -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Resolve-Path $Exe).Path }) {
        throw "Close the running test instances first (-Stop)."
    }
    foreach ($p in $profiles) {
        $d = Join-Path $Root $p.name
        if (Test-Path $d) { Remove-Item -Recurse -Force $d; Write-Host "Removed local test profile $d" }
    }
    if (Test-Path $mock) { Remove-Item -Recurse -Force $mock; Write-Host "Removed mock mailbox $mock" }
}
New-Item -ItemType Directory -Force $mock | Out-Null

$inbox = @{ store_id = "MOCKSTORE"; entry_id = "MOCK-Inbox"; path = "Inbox" }
$done = @{ store_id = "MOCKSTORE"; entry_id = "MOCK-Completed"; path = "Inbox/Completed" }
foreach ($p in $profiles) {
    $dir = Join-Path $Root $p.name
    New-Item -ItemType Directory -Force $dir, (Join-Path $dir "logs") | Out-Null
    $settingsPath = Join-Path $dir "settings.json"
    $s = if (Test-Path $settingsPath) { Get-Content $settingsPath -Raw | ConvertFrom-Json } else { [pscustomobject]@{} }
    $set = { param($k, $v) if ($s.PSObject.Properties[$k]) { $s.$k = $v } else { $s | Add-Member -NotePropertyName $k -NotePropertyValue $v } }
    & $set "shared_mode" "shared"
    & $set "shared_db_path" $SharedDb
    & $set "shared_sync_minutes" $SyncMinutes
    & $set "staff_name" $p.staff
    & $set "launch_at_login" $false
    & $set "close_to_tray" $false
    if ($Outlook -eq "mock") {
        if ($MailboxName) { & $set "mailbox_name" $MailboxName }
        & $set "mailbox_store_id" "MOCKSTORE"
        & $set "folder" $inbox
        & $set "subfolder_mode" "none"
        & $set "archive_folder" $done
        & $set "archive_mailbox_name" "Mock Shared Mailbox"
        & $set "sync_enabled" $true
        & $set "interval_minutes" 5
        & $set "check_on_startup" $false
    }
    # UTF-8 without byte-order mark
    [IO.File]::WriteAllText($settingsPath, ($s | ConvertTo-Json -Depth 6), (New-Object Text.UTF8Encoding($false)))
    Write-Host "Profile $($p.name): $dir (staff '$($p.staff)', DevTools port $($p.port))"
}

if ($NoLaunch) { Write-Host "Profiles prepared; not launched (-NoLaunch)."; return }

# ---------------------------------------------------------------- launch (process-scoped environment)
$keys = "ASR_APP_DATA_DIR", "ASR_TEST_MODE", "ASR_PROFILE_NAME", "ASR_TEST_STAFF", "ASR_DEVTOOLS_PORT", "ASR_OUTLOOK", "ASR_OUTLOOK_MOCK_DIR"
$saved = @{}
foreach ($k in $keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k, "Process") }
$started = @()
if ($Only -and (Test-Path $stateFile)) { $started = @(@((Get-Content $stateFile -Raw | ConvertFrom-Json)) | Where-Object { $_.name -ne $Only }) }
try {
    foreach ($p in ($profiles | Where-Object { -not $Only -or $_.name -eq $Only })) {
        $dir = Join-Path $Root $p.name
        [Environment]::SetEnvironmentVariable("ASR_APP_DATA_DIR", $dir, "Process")
        [Environment]::SetEnvironmentVariable("ASR_TEST_MODE", "1", "Process")
        [Environment]::SetEnvironmentVariable("ASR_PROFILE_NAME", $p.name, "Process")
        [Environment]::SetEnvironmentVariable("ASR_TEST_STAFF", $p.staff, "Process")
        [Environment]::SetEnvironmentVariable("ASR_DEVTOOLS_PORT", [string]$p.port, "Process")
        [Environment]::SetEnvironmentVariable("ASR_OUTLOOK", $Outlook, "Process")
        [Environment]::SetEnvironmentVariable("ASR_OUTLOOK_MOCK_DIR", $mock, "Process")
        $out = Join-Path $dir "logs\stdout.log"
        $err = Join-Path $dir "logs\stderr.log"
        $proc = Start-Process -FilePath $Exe -WorkingDirectory $dir -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
        $started += [pscustomobject]@{ name = $p.name; pid = $proc.Id; data_dir = $dir; devtools_port = $p.port; staff = $p.staff; started_at = (Get-Date).ToString("o") }
        Write-Host "Started $($p.name): pid $($proc.Id)" -ForegroundColor Green
        Start-Sleep -Milliseconds 800
    }
} finally {
    foreach ($k in $keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k], "Process") }
}
ConvertTo-Json @($started) | Set-Content -Encoding UTF8 $stateFile
Write-Host ""
Write-Host "Both instances share: $SharedDb"
Write-Host "Mock Outlook mailbox: $mock (journal.jsonl records every move/flag/send request)"
Write-Host "Automated dual-instance tests: node scripts\multiuser\run-tests.mjs"
Write-Host "Stop both: scripts\launch_multiuser_test.ps1 -Stop"
