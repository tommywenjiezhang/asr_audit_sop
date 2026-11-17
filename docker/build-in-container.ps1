$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

if (!(Test-Path C:\source\package.json)) { throw 'Mount the project root at C:\source (package.json not found).' }
if (!(Test-Path C:\source\scripts\build.ps1)) { throw 'Expected existing scripts\build.ps1 is missing.' }
New-Item -ItemType Directory -Force C:\work,C:\output | Out-Null

# Copy source into the container: npm/cargo and build scripts can modify it safely.
Copy-Item 'C:\source\*' C:\work -Recurse -Force
Set-Location C:\work

# Read Machine PATH updated during image build, and initialize MSVC environment.
$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + $env:Path
$env:CARGO_HOME = 'C:\rust\cargo'
$env:RUSTUP_HOME = 'C:\rust\rustup'
$env:CARGO_TARGET_DIR = 'C:\cargo-target'
$env:CI = 'true'

$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$vcvars = 'C:\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
if (!(Test-Path $vcvars)) { throw "MSVC vcvars64.bat not found: $vcvars" }
# Translate the environment from vcvars64.bat into this PowerShell process.
$batEnv = & cmd.exe /d /s /c "`"call `"$vcvars`" >nul && set`""
if ($LASTEXITCODE -ne 0) { throw 'Failed initializing Visual C++ compiler environment.' }
foreach ($line in $batEnv) {
  if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1],$matches[2],'Process') }
}

Write-Host '=== Toolchain versions ==='
& node --version
& npm.cmd --version
& python --version
& cargo --version
& rustc --version

Write-Host '=== Project existing build script ==='
# This is the same entry point requested in the existing ASR_email_monitor project.
# It should build the PyInstaller Outlook worker sidecar and the Tauri/NSIS installer.
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build.ps1 -WithWorkerExe
if ($LASTEXITCODE -ne 0) { throw "scripts/build.ps1 failed with exit code $LASTEXITCODE" }

Write-Host '=== Publishing build artifacts ==='
$targets = @('C:\cargo-target\release\bundle', 'C:\work\src-tauri\target\release\bundle', 'C:\work\target\release\bundle')
$found = $false
foreach ($dir in $targets) {
  if (Test-Path $dir) {
    Copy-Item "$dir\*" C:\output -Recurse -Force
    $found = $true
  }
}
if (!$found) { throw 'Build completed but no Tauri bundle directory was found; inspect build.ps1 output locations.' }
Get-ChildItem C:\output -Recurse -File | Select-Object FullName, Length
