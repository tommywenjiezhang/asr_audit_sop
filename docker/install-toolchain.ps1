param(
  [string] $NodeVersion = '22.22.0',
  [string] $PythonVersion = '3.12.10'
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Download([string]$url, [string]$destination) {
  Write-Host "Downloading $url"
  Invoke-WebRequest -Uri $url -OutFile $destination -UseBasicParsing
}
function Assert-Exit([string]$name, [int]$code) {
  if ($code -ne 0) { throw "$name failed with exit code $code" }
}

New-Item -ItemType Directory -Force C:\setup,C:\toolchain | Out-Null

# Node.js official binary distribution.
$nodeZip = 'C:\setup\node.zip'
Download "https://nodejs.org/dist/v$NodeVersion/node-v$NodeVersion-win-x64.zip" $nodeZip
Expand-Archive -Path $nodeZip -DestinationPath C:\toolchain -Force
$nodeDir = "C:\toolchain\node-v$NodeVersion-win-x64"
$env:Path = "$nodeDir;$env:Path"
[Environment]::SetEnvironmentVariable('Path', "$nodeDir;" + [Environment]::GetEnvironmentVariable('Path','Machine'), 'Machine')
& "$nodeDir\node.exe" --version

# Full CPython is required because PyInstaller creates the Outlook worker sidecar.
$pythonInstaller = 'C:\setup\python-installer.exe'
Download "https://www.python.org/ftp/python/$PythonVersion/python-$PythonVersion-amd64.exe" $pythonInstaller
$pyProc = Start-Process -FilePath $pythonInstaller -ArgumentList '/quiet InstallAllUsers=1 TargetDir=C:\Python312 PrependPath=0 Include_test=0 Include_launcher=0' -Wait -PassThru
Assert-Exit 'Python installer' $pyProc.ExitCode
$env:Path = "C:\Python312;C:\Python312\Scripts;$env:Path"
[Environment]::SetEnvironmentVariable('Path', 'C:\Python312;C:\Python312\Scripts;' + [Environment]::GetEnvironmentVariable('Path','Machine'), 'Machine')
& C:\Python312\python.exe --version

# Native MSVC toolchain + Windows SDK required for x86_64-pc-windows-msvc.
# VS Build Tools installation in containers is resource-intensive and depends on
# the available Microsoft installer components / corporate network policy.
$vsInstaller = 'C:\setup\vs_buildtools.exe'
Download 'https://aka.ms/vs/17/release/vs_buildtools.exe' $vsInstaller
$vsArguments = @(
  '--quiet', '--wait', '--norestart', '--nocache',
  '--installPath', 'C:\BuildTools',
  '--add', 'Microsoft.VisualStudio.Workload.VCTools',
  '--includeRecommended',
  '--remove', 'Microsoft.VisualStudio.Component.Windows10SDK.10240',
  '--remove', 'Microsoft.VisualStudio.Component.Windows10SDK.10586',
  '--remove', 'Microsoft.VisualStudio.Component.Windows10SDK.14393',
  '--remove', 'Microsoft.VisualStudio.Component.Windows81SDK'
)
$vsProc = Start-Process -FilePath $vsInstaller -ArgumentList $vsArguments -Wait -PassThru
if ($vsProc.ExitCode -notin @(0,3010)) { throw "Visual Studio Build Tools installer failed with exit code $($vsProc.ExitCode)" }

# rustup official bootstrap; invoke cargo via its full path and initialize VS dev environment at build time.
$rustup = 'C:\setup\rustup-init.exe'
Download 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' $rustup
$env:CARGO_HOME = 'C:\rust\cargo'
$env:RUSTUP_HOME = 'C:\rust\rustup'
[Environment]::SetEnvironmentVariable('CARGO_HOME', $env:CARGO_HOME, 'Machine')
[Environment]::SetEnvironmentVariable('RUSTUP_HOME', $env:RUSTUP_HOME, 'Machine')
$rustProc = Start-Process -FilePath $rustup -ArgumentList '-y --no-modify-path --default-toolchain stable-x86_64-pc-windows-msvc' -Wait -PassThru
Assert-Exit 'rustup' $rustProc.ExitCode
[Environment]::SetEnvironmentVariable('Path', 'C:\rust\cargo\bin;' + [Environment]::GetEnvironmentVariable('Path','Machine'), 'Machine')
& 'C:\rust\cargo\bin\cargo.exe' --version

# Tauri NSIS bundling may download NSIS and WebView2 installer payloads during packaging.
# Do not install/run Outlook here: COM integration requires an interactive staff Windows session.
