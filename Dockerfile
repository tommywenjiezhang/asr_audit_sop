# syntax=docker/dockerfile:1
# Windows container build environment for ASR/Audit Support Desk.
# Requires Docker Desktop switched to Windows containers on a compatible Windows host.
# IT approval required; containers do not override endpoint security restrictions.
FROM mcr.microsoft.com/windows/servercore:ltsc2022

SHELL ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]

# Installer versions can be overridden with --build-arg during docker build.
ARG NODE_VERSION=22.22.0
ARG PYTHON_VERSION=3.12.10
# Bridge the build ARGs into environment variables so the PowerShell RUN below
# can read them as $env:NODE_VERSION / $env:PYTHON_VERSION (ARGs are NOT auto-exported).
ENV NODE_VERSION=$NODE_VERSION
ENV PYTHON_VERSION=$PYTHON_VERSION

COPY docker/install-toolchain.ps1 C:/setup/install-toolchain.ps1
RUN & 'C:\setup\install-toolchain.ps1' -NodeVersion $env:NODE_VERSION -PythonVersion $env:PYTHON_VERSION

COPY docker/build-in-container.ps1 C:/setup/build-in-container.ps1

# Container will receive the local project as read-only C:\source and publish to C:\output.
WORKDIR C:/work
ENTRYPOINT ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\setup\\build-in-container.ps1"]
