# Measures startup time, idle CPU and memory of a running ASR/Audit Support Desk instance.
# Start the app first, wait for it to settle, then run this script.
$p = Get-Process -Name "asr-support-desk" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $p) { Write-Host "App is not running."; exit 1 }
$web = Get-CimInstance Win32_Process | Where-Object { $_.Name -eq "msedgewebview2.exe" -and $_.CommandLine -like "*asrsupportdesk*" }
$webMem = ($web | ForEach-Object { (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).WorkingSet64 } | Measure-Object -Sum).Sum
$cpu1 = $p.TotalProcessorTime; Start-Sleep -Seconds 30; $p.Refresh(); $cpu2 = $p.TotalProcessorTime
"Backend working set : {0:N1} MB" -f ($p.WorkingSet64 / 1MB)
"WebView2 working set: {0:N1} MB ({1} processes)" -f ($webMem / 1MB), @($web).Count
"Backend idle CPU    : {0:N2} % over 30 s" -f ((($cpu2 - $cpu1).TotalSeconds / 30) * 100 / [Environment]::ProcessorCount)
"Backend startup time is shown in Settings > Database / Backup and in the log (startup_ms)."
