# ASR/Audit Support Desk — Windows Docker build (experimental)

**Important:** This is a *build environment*, not a way to bypass NJ OIT endpoint security. Obtain IT approval before running it. CrowdStrike can still inspect/block processes associated with containers; Windows Docker availability depends on organizational policy.

## Prerequisites

- Windows 11 Pro/Enterprise and Docker Desktop installed in all-users mode with Windows containers enabled (not Linux/WSL containers).
- Hyper-V + Containers Windows features enabled; appropriate machine virtualization resources and a generous amount of available disk space.
- Docker Desktop **Switch to Windows containers**. Run `docker info --format '{{.OSType}}'` to verify `windows`.
- Corporate network access/approved proxy and CA trust for downloads from Microsoft, Python, Node.js, Rust, and npm/crates.io. Prefer an IT-managed builder with approved toolchain sources.
- The ASR_email_monitor project with package.json, src-tauri, outlook_worker, and scripts/build.ps1.

## Add these files to your project root

Copy `Dockerfile`, `.dockerignore`, and the `docker/` folder into ASR_email_monitor. The Dockerfile builds a Windows Server Core 2022 image with Node, Python, Microsoft C++ Build Tools, and Rust. It then runs your existing `scripts/build.ps1 -WithWorkerExe` inside the container.

## PowerShell commands (from project root)

```powershell
docker info --format '{{.OSType}}'   # Must output windows
# Only after authorization to run container builds:
docker build --isolation=hyperv -t asr-support-desk-builder -f Dockerfile .
New-Item -ItemType Directory -Force .\docker-output | Out-Null
$src = (Get-Location).Path
$out = (Resolve-Path .\docker-output).Path
docker run --rm --isolation=hyperv `
  --mount "type=bind,source=$src,target=C:\source,readonly" `
  --mount "type=bind,source=$out,target=C:\output" `
  asr-support-desk-builder
```

Result (on success): look in `docker-output\nsis` for the NSIS setup executable.

## Limitations

- **Not tested here:** This conversation runtime cannot run Docker Desktop/Windows containers or compile your private project.
- The image is substantial. Visual Studio Build Tools installation may fail depending on the Server Core image, network requirements, SDK component availability, or unsupported installer behavior.
- A Windows container generally cannot run or test the interactive Outlook COM desktop workflow; test the installer on a separately authorized Windows workstation.
- WebView2 and NSIS packaging prerequisites are Tauri-version dependent. Adjust as needed if the project installer stage reports missing components.
- End-user executable signing and corporate approval are still required.
- The project build script may use hard-coded paths or assume a developer desktop. If it does, modify it to respect `CARGO_TARGET_DIR` and a non-interactive environment.
- Docker image builds run downloaded installers; verify URLs and software versions with IT before use. Windows container images must be compatible with the host OS and license.
