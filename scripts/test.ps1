# Runs every automated test suite.
#   Python Outlook worker tests (fake COM objects, no Outlook needed)
#   Frontend type check
#   Rust unit tests (core logic only; the Tauri shell is excluded with --no-default-features)
. "$PSScriptRoot\env.ps1"
$failed = @()

Write-Host "== Python Outlook worker tests ==" -ForegroundColor Cyan
python -m unittest discover -s outlook_worker/tests -v
if ($LASTEXITCODE -ne 0) { $failed += "python" }

Write-Host "== Frontend type check ==" -ForegroundColor Cyan
npx svelte-check --tsconfig ./tsconfig.json
if ($LASTEXITCODE -ne 0) { $failed += "svelte-check" }

Write-Host "== Production frontend bundle contract ==" -ForegroundColor Cyan
npm run build
if ($LASTEXITCODE -ne 0) { $failed += "vite-build" }
python scripts\verify_frontend_bundle.py
if ($LASTEXITCODE -ne 0) { $failed += "frontend-bundle" }

Write-Host "== Rust tests ==" -ForegroundColor Cyan
Push-Location src-tauri
cargo test --no-default-features
if ($LASTEXITCODE -ne 0) { $failed += "rust" }
Pop-Location

if ($failed.Count) { Write-Host "FAILED: $($failed -join ', ')" -ForegroundColor Red; exit 1 }
Write-Host "All test suites passed." -ForegroundColor Green
