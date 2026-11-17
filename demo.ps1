# =====================================================================================
# demo.ps1 - Build the ASR/Audit Support Desk yourself, one step at a time.
#
# HOW TO USE
#   Open a NORMAL PowerShell window (NOT "Run as administrator"), then either:
#     a) run the whole file:   powershell -ExecutionPolicy Bypass -File .\demo.ps1
#     b) or copy/paste one step at a time and read the result before moving on.
#   Every step stops the script if it fails, so you always know which step broke.
#
# BEFORE YOU START
#   On this PC, CrowdStrike Falcon blocks newly built programs (Step 7 onward).
#   Run this only after the security team has added a Falcon exclusion for
#   %LOCALAPPDATA%\asr-support-desk-target, or run it on an unrestricted build machine.
#   Step 2 checks for recent Falcon blocks so you can tell if that is the problem.
# =====================================================================================


# ---- STEP 0: Go to the project folder -------------------------------------------------
# $PSScriptRoot is the folder this script lives in (the project root).
# Set-Location makes it the current folder, so relative paths like "src-tauri" work.
Set-Location $PSScriptRoot

# Stop at the first PowerShell error instead of carrying on with a broken build.
$ErrorActionPreference = "Stop"


# ---- STEP 1: Make sure you are NOT running as administrator ---------------------------
# An elevated window cannot talk to Outlook (COM error 0x80080005), and an elevated
# install would put files in the wrong user profile. This line asks Windows whether
# the current user token has the Administrator role switched on.
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

# If it does, print a warning. (Building works when elevated, but testing with Outlook does not.)
if ($isAdmin) { Write-Warning "This window is elevated. Building is fine, but Outlook tests will fail. Prefer a normal window." }


# ---- STEP 2: Check for recent CrowdStrike blocks --------------------------------------
# Reads the last 24 hours of the Falcon sensor's Windows event log and counts events
# 3 ("process was blocked") and 4 ("file was quarantined"). If the count goes up after
# Step 7, Falcon is still blocking the build and you need the security team.
$falconBlocks = Get-WinEvent -FilterHashtable @{ LogName = 'CrowdStrike-Falcon Sensor-CSFalconService/Operational'; Id = 3, 4; StartTime = (Get-Date).AddDays(-1) } -ErrorAction SilentlyContinue

# Show the count (0 if the log is missing or empty).
Write-Host "Falcon block/quarantine events in the last 24 h: $(@($falconBlocks).Count)"


# ---- STEP 3: Set up the build environment ---------------------------------------------
# Rust is installed per-user in %USERPROFILE%\.cargo\bin, which is not on PATH by default.
# Putting it first on PATH lets PowerShell find cargo.exe and rustc.exe.
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

# Tell Cargo to write its (large, ~2-4 GB) build output OUTSIDE OneDrive.
# This is also the exact folder the Falcon exclusion must cover.
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\asr-support-desk-target"

# Print versions to confirm every tool is found. Each line fails loudly if a tool is missing.
rustc --version      # Rust compiler (expects stable, GNU or MSVC toolchain)
cargo --version      # Rust package manager / build tool
node --version       # Node.js, runs the frontend build and the Tauri CLI (needs 20+)
python --version     # Python, runs the Outlook worker and its tests (needs 3.9+)


# ---- STEP 4: Install JavaScript dependencies ------------------------------------------
# "npm ci" installs exactly the versions pinned in package-lock.json into node_modules.
# Skipped if node_modules already exists, because a clean reinstall takes a minute or two.
if (-not (Test-Path node_modules)) { npm ci; if ($LASTEXITCODE -ne 0) { throw "npm ci failed" } }

# Install the Python packages: pywin32 (Outlook COM access) and pyinstaller (makes outlook_worker.exe).
# --user installs for you only, so no admin rights are needed.
python -m pip install --user pywin32 pyinstaller
if ($LASTEXITCODE -ne 0) { throw "pip install failed" }


# ---- STEP 5: Test the Python Outlook worker -------------------------------------------
# Runs the 18 unit tests in outlook_worker\tests. They use fake Outlook objects,
# so they need no Outlook and never touch your mailbox.
python -m unittest discover -s outlook_worker/tests -v
if ($LASTEXITCODE -ne 0) { throw "Python worker tests failed" }


# ---- STEP 6: Check and build the Svelte frontend --------------------------------------
# svelte-check is the TypeScript/Svelte type checker. Expect "0 ERRORS 0 WARNINGS".
npx svelte-check --tsconfig ./tsconfig.json
if ($LASTEXITCODE -ne 0) { throw "Frontend type check failed" }

# Vite bundles the UI into the dist\ folder, which gets packed into the app.
npm run build
if ($LASTEXITCODE -ne 0) { throw "Frontend build failed" }


# ---- STEP 7: Run the Rust core tests (FIRST STEP FALCON MAY BLOCK) --------------------
# Compiles the backend WITHOUT the Tauri window (--no-default-features turns off the "app"
# feature) and runs ~50 unit tests: database, email analyzer, tickets, drafts, scheduler.
# The first run downloads crates and compiles for several minutes.
# If you see "Access is denied (os error 5)", Falcon blocked it: re-run Step 2 to confirm.
Push-Location src-tauri          # cargo commands must run in the folder with Cargo.toml
cargo test --no-default-features
$rustResult = $LASTEXITCODE      # save the result before Pop-Location changes anything
Pop-Location                     # back to the project root
if ($rustResult -ne 0) { throw "Rust tests failed (or were blocked). Copy the error output for fixing." }


# ---- STEP 8: Package the Outlook worker as an .exe ------------------------------------
# PyInstaller bundles outlook_worker.py plus Python itself into one file, so staff PCs
# do not need Python installed.
#   --onefile     one single .exe
#   --noconsole   no black console window flashes when the app calls it
#   --distpath    put the .exe in the outlook_worker\ folder, where the installer expects it
python -m PyInstaller --onefile --noconsole --name outlook_worker --distpath outlook_worker outlook_worker/outlook_worker.py
if ($LASTEXITCODE -ne 0) { throw "PyInstaller failed" }


# ---- STEP 9: Tell Tauri to include the worker .exe in the installer -------------------
# Read the normal Tauri config (JSON) into a PowerShell object.
$conf = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json

# Add one entry to "bundle.resources": source file -> path inside the installed app.
$conf.bundle.resources | Add-Member -NotePropertyName "../outlook_worker/outlook_worker.exe" -NotePropertyValue "outlook_worker/outlook_worker.exe" -Force

# Save it as a SEPARATE config file, so the original tauri.conf.json stays unchanged.
$conf | ConvertTo-Json -Depth 20 | Set-Content src-tauri/tauri.conf.windows.json -Encoding utf8


# ---- STEP 10: Build the app and the NSIS installer ------------------------------------
# "tauri build" does three things: rebuilds the frontend, compiles the Rust app in
# release (optimized) mode, then makes a per-user NSIS installer (no admin needed to install).
# The first time, Tauri also downloads the NSIS tool. Expect 5-15 minutes.
npx tauri build --config src-tauri/tauri.conf.windows.json
if ($LASTEXITCODE -ne 0) { throw "Tauri build failed" }


# ---- STEP 11: Show where the results are ----------------------------------------------
# The installer is written here. Its name looks like "ASR Audit Support Desk_1.0.0_x64-setup.exe".
$installerDir = "$env:CARGO_TARGET_DIR\release\bundle\nsis"
Get-ChildItem $installerDir -Filter *.exe | Select-Object FullName, @{ n = 'SizeMB'; e = { [math]::Round($_.Length / 1MB, 1) } }

# The app itself (without installing) is here:
$appExe = "$env:CARGO_TARGET_DIR\release\asr-support-desk.exe"
Write-Host "App executable: $appExe"


# ---- STEP 12 (optional): Launch the app to try it --------------------------------------
# Start classic Outlook first, then launch the app normally (not elevated).
# In the app: Settings > Test Outlook Connection > Find Mailboxes > pick the folder.
# The app only creates DRAFTS. It never sends, moves or deletes email.
Start-Process $appExe

Write-Host "Done. Installer folder: $installerDir" -ForegroundColor Green
