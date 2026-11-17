# Starts the app in development mode (hot reload for the UI).
. "$PSScriptRoot\env.ps1"
if (-not (Test-Path node_modules)) { npm ci }
npx tauri dev
