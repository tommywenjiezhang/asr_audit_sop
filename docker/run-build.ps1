# =====================================================================================
# docker\run-build.ps1 - Build the ASR/Audit Support Desk inside a Windows container
# and copy the finished NSIS installer back to the host.
#
# Run from a normal PowerShell window on a host with Docker Desktop in WINDOWS-container
# mode (Settings > "Switch to Windows containers", or the tray menu). IT approval required;
# the container does NOT override host endpoint-security policy, it is an isolated build box.
#
#   powershell -ExecutionPolicy Bypass -File .\docker\run-build.ps1
#
# Result: the installer (ASR Audit Support Desk_1.0.0_x64-setup.exe) is copied to .\installer
# =====================================================================================
param(
  [string] $Tag       = 'asr-support-desk-build:latest',
  # Where the finished installer is copied on the host (default: <project>\installer).
  [string] $OutputDir = (Join-Path (Split-Path -Parent $PSScriptRoot) 'installer')
)
$ErrorActionPreference = 'Stop'

# Project root = the folder that contains this docker\ directory.
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Write-Host "Project root : $projectRoot"
Write-Host "Output dir   : $OutputDir"

# ---- 1. Require Docker in Windows-container mode -------------------------------------
# The Dockerfile uses a Windows Server Core base image; it cannot run under the Linux engine.
$serverOs = (docker version --format '{{.Server.Os}}' 2>$null)
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($serverOs)) {
  throw 'Cannot reach the Docker daemon. Start Docker Desktop and try again.'
}
if ($serverOs -ne 'windows') {
  throw "Docker is in '$serverOs' container mode. Right-click the Docker tray icon and choose 'Switch to Windows containers...', then re-run."
}

# ---- 2. Build the image (installs Node, Python, MSVC Build Tools, Rust) --------------
# .dockerignore keeps node_modules / build output out of the build context.
Write-Host "`n=== docker build ===" -ForegroundColor Cyan
docker build -t $Tag -f (Join-Path $projectRoot 'Dockerfile') $projectRoot
if ($LASTEXITCODE -ne 0) { throw 'docker build failed.' }

# ---- 3. Run the build inside a container ---------------------------------------------
# The project is bind-mounted read-only at C:\source; the entrypoint copies it to C:\work,
# runs scripts\build.ps1 -WithWorkerExe, and publishes the bundle to C:\output.
# --mount (not -v) avoids the Windows drive-letter colon ambiguity and handles spaces in the path.
$name = "asr_build_$(Get-Date -Format yyyyMMdd_HHmmss)"
Write-Host "`n=== docker run ($name) ===" -ForegroundColor Cyan
docker run --name $name --mount "type=bind,source=$projectRoot,target=C:\source,readonly" $Tag
$runExit = $LASTEXITCODE

# ---- 4. Copy artifacts out of the container to the host ------------------------------
# Done even on failure so build logs/partial output can be inspected; the container is
# then removed. This is the "copy final exe from the image back to local" step.
New-Item -ItemType Directory -Force $OutputDir | Out-Null
Write-Host "`n=== docker cp C:\output -> $OutputDir ===" -ForegroundColor Cyan
docker cp "${name}:C:\output\." $OutputDir
$cpExit = $LASTEXITCODE
docker rm $name | Out-Null

if ($runExit -ne 0) { throw "Container build failed (exit $runExit). Any partial output is in $OutputDir." }
if ($cpExit -ne 0)  { throw "Build succeeded but copying artifacts out of the container failed." }

# ---- 5. Report the installer ---------------------------------------------------------
$exe = Get-ChildItem $OutputDir -Recurse -Filter *.exe -ErrorAction SilentlyContinue |
       Where-Object { $_.Name -like '*setup*' } | Select-Object -First 1
if ($exe) {
  Write-Host "`nInstaller copied to host: $($exe.FullName)" -ForegroundColor Green
} else {
  Write-Warning "Build finished but no '*setup*.exe' was found under $OutputDir. Inspect the folder for the bundle."
}
