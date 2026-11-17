# Shared by build.ps1 and build-portable.ps1: packages the Python Outlook worker as a standalone
# outlook_worker\outlook_worker.exe (PyInstaller one-file; contains Python + pywin32, so target PCs need no Python).
function Build-WorkerExe {
  python -m pip install --user --quiet pyinstaller pywin32
  if ($LASTEXITCODE -ne 0) { throw "pip install of pyinstaller/pywin32 failed." }
  python -m PyInstaller --noconfirm --onefile --noconsole --name outlook_worker --distpath outlook_worker --workpath build\pyinstaller --specpath build outlook_worker/outlook_worker.py
  if ($LASTEXITCODE -ne 0) { throw "PyInstaller failed - outlook_worker.exe not built." }
  $exe = Get-Item "outlook_worker\outlook_worker.exe"
  Write-Host ("Worker: {0} ({1:N1} MB)" -f $exe.FullName, ($exe.Length / 1MB))
  return $exe.FullName
}
