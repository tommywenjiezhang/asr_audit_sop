# Building the ASR/Audit Support Desk on Windows

This guide covers how to compile, test, package and verify this Tauri 2 app (Rust + Svelte + SQLite + a Python Outlook worker) on a managed NJ OIT Windows 11 PC. It is written so that the same process can be reused for **any Rust/Tauri desktop app** on a machine with CrowdStrike Falcon, Group Policy and OneDrive-redirected folders. Section 10 is the checklist for reusing it.

Last verified end to end on 2026-11-17, with Rust 1.100.0 (MSVC), VS 2022 Build Tools (MSVC 14.44), Windows SDK 10.0.26100, Node 24, Python 3.14 and Tauri CLI 2.12.

---

## 1. Quick start

Use a **normal (non-administrator) PowerShell**, opened in the project folder:

```powershell
cd "$env:USERPROFILE\OneDrive - New Jersey Office of Information Technology\Documents\Other Data Project\ASR_email_monitor"
scripts\doctor.ps1                 # check the build environment (add -Fix for the Rust parts)
scripts\build.ps1 -WithWorkerExe   # tests -> worker exe -> Tauri release build -> NSIS installer
scripts\verify-install.ps1         # install per-user, launch, check DB / templates / Outlook
```

The installer is copied to:

```
%USERPROFILE%\Documents\ASR Audit Support Desk_<version>_x64-setup.exe
```

On this PC that is `C:\Users\wezhang\Documents`, the local Documents folder, **not** the OneDrive Documents folder. To copy it somewhere else, use `scripts\build.ps1 -WithWorkerExe -OutputDir D:\Somewhere` or set `$env:ASR_INSTALLER_OUT`.

---

## 2. What the build produces

```
 src/ (Svelte)  --vite build-->  dist-web/  --+
                                          |
 src-tauri/ (Rust) --cargo (MSVC)--> asr-support-desk.exe --+--> Tauri bundler --> NSIS --> *-setup.exe
                                                            |                                  |
 outlook_worker/outlook_worker.py --PyInstaller--> outlook_worker.exe (bundled as resource)    |
                                                                                               v
                                                              copied to %USERPROFILE%\Documents
```

| Artifact | Location |
|---|---|
| Rust build output (several GB, cached between builds) | `%LOCALAPPDATA%\asr-support-desk-target-msvc\` |
| App exe | `%LOCALAPPDATA%\asr-support-desk-target-msvc\release\asr-support-desk.exe` |
| Installer (original) | `%LOCALAPPDATA%\asr-support-desk-target-msvc\release\bundle\nsis\ASR Audit Support Desk_<ver>_x64-setup.exe` |
| Installer (copy to hand out) | `%USERPROFILE%\Documents\ASR Audit Support Desk_<ver>_x64-setup.exe` |
| Worker exe | `outlook_worker\outlook_worker.exe` (gitignored) |
| Generated Tauri config with worker exe | `src-tauri\tauri.conf.windows.json` (gitignored) |
| Frontend bundle | `dist-web\` (gitignored; embedded into the exe) |
| Portable ZIP + `.sha256` | `dist\ASR-Audit-Support-Desk-<ver>-windows-x64.zip` (gitignored), from `scripts\build-portable.ps1` |
| Tauri's NSIS toolchain cache | `%LOCALAPPDATA%\tauri\NSIS\` |

Installed app (per-user, no admin rights): the default location is `%LOCALAPPDATA%\ASR Audit Support Desk\`. A reinstall reuses the folder of the previous install, which is recorded in `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\ASR Audit Support Desk`. App data is always in `%APPDATA%\gov.nj.doh.asrsupportdesk\`.

---

## 3. One-time setup

### 3.1 Install the required software

| Component | How to get it on a managed PC | Notes |
|---|---|---|
| **Visual Studio 2022 Build Tools** | **Software Center**. `winget` is disabled by Group Policy ("Enable Windows Package Manager"), so do not try to work around that | Select the **Desktop development with C++** workload, **MSVC v143 x64/x86 build tools** and a **Windows 10/11 SDK**. This is the only step that needs elevation, and Software Center handles it |
| **Rust (rustup)** | https://rustup.rs, installed per user under `%USERPROFILE%\.cargo` and `%USERPROFILE%\.rustup`, no admin rights | Make sure `%USERPROFILE%\.cargo\bin` is on PATH. Reopen PowerShell after installing |
| **Node.js 20+** | Software Center or the user installer | `npm ci` installs the Tauri CLI locally (`npx tauri`) |
| **Python 3.9+** | Software Center or the user installer | `build.ps1 -WithWorkerExe` installs `pyinstaller` and `pywin32` with `pip --user` |
| **WebView2 runtime** | Preinstalled on Windows 11 | Needed to run the app, not to build it |

### 3.2 Select the MSVC Rust toolchain for this project only

The global default on this PC is the GNU toolchain (`stable-x86_64-pc-windows-gnu`). Leave it as it is and add a **directory override**:

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc --profile minimal
cd <project root>;  rustup override set stable-x86_64-pc-windows-msvc
cd src-tauri;       rustup override set stable-x86_64-pc-windows-msvc
rustup show active-toolchain   # must say ...-msvc (directory override ...)
rustup default                 # still ...-gnu: unchanged
```

`scripts\doctor.ps1 -Fix` runs these commands for you. Overrides are stored in `%USERPROFILE%\.rustup\settings.toml` and apply to the folder and its subfolders. If the project folder is moved or renamed, set them again.

**Why MSVC and not GNU:** Tauri, WebView2 and `windows-rs` are developed and tested against MSVC first. The MSVC linker ships with Build Tools from Software Center, so there is no portable MinGW from unknown sources. The previous GNU builds were also the ones Falcon blocked (see section 7).

You do **not** need to open a "Developer PowerShell for VS" or run `vcvars64.bat`. `rustc` finds `link.exe` and the SDK libraries itself through `vswhere`. `scripts\doctor.ps1` shows the exact linker it will use.

### 3.3 Keep build output out of OneDrive

`scripts\env.ps1` always sets:

```powershell
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\asr-support-desk-target-msvc"
```

Reasons:

- OneDrive would try to sync several GB of short-lived files, and it locks files while cargo is writing them.
- Falcon has allowed builds from this folder (verified 2026-11-17).
- A separate folder keeps the MSVC output apart from the old GNU output in `...\asr-support-desk-target`, which can be deleted.

`env.ps1` overrides any inherited `CARGO_TARGET_DIR`, because one was found inherited from an older session. To build somewhere else on purpose, set `$env:ASR_CARGO_TARGET_DIR`.

If you run `cargo` by hand **outside** the scripts, set it first:

```powershell
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\asr-support-desk-target-msvc"
```

---

## 4. Everyday commands

| Command | What it does |
|---|---|
| `scripts\doctor.ps1 [-Fix]` | Read-only environment report: elevation, toolchain and overrides, Build Tools, `link.exe`, SDK, Node, Python, pywin32, output folders, NSIS cache, Falcon events in the last 24 h. `-Fix` installs the MSVC toolchain and sets the overrides, user-level only |
| `scripts\test.ps1` | Python worker tests (fake COM), `svelte-check`, Rust core tests (`cargo test --no-default-features`) |
| `scripts\dev.ps1` | `npx tauri dev`: hot-reloading development build |
| `scripts\build.ps1 [-WithWorkerExe] [-SkipTests] [-OutputDir <dir>]` | Full release pipeline (section 5). Copies the installer to `%USERPROFILE%\Documents` |
| `scripts\verify-install.ps1 [-OutlookDraftTest] [-Installer <path>] [-NoInstall]` | Silent per-user install, launch through `explorer.exe`, DB and template check, Outlook connection check, Falcon check (section 6) |
| `scripts\measure.ps1` | Idle CPU and memory of a running instance |

Release builds use `opt-level = "s"`, LTO and `codegen-units = 1`. A clean release build takes about 2.5 minutes, and a rebuild with nothing changed takes seconds.

---

## 4a. Shared database (Access) tests

The shared-database code uses the `odbc-api` crate and links `odbc32.dll` / `odbccp32.dll` (always present on Windows). The 64-bit **Microsoft Access Database Engine** is needed only to run the real-driver tests and to use the shared database at run time; the build itself does not need it.

```powershell
. scripts\env.ps1; cd src-tauri
$env:ASR_ACCESS_TEST_DIR = "$env:LOCALAPPDATA\asr-access-test"      # a LOCAL test folder (never the production share)
New-Item -ItemType Directory -Force $env:ASR_ACCESS_TEST_DIR | Out-Null
cargo test --no-default-features -- --ignored access_ diag_reproduce --nocapture   # driver-specific checks
$env:ASR_SHARED_BACKEND = "access"                                    # run the multi-workstation scenarios on real .accdb files
cargo test --no-default-features -- --test-threads=1 shared_ workflow:: diag::
Remove-Item Env:ASR_SHARED_BACKEND
```

Each test creates its own new `.accdb` in that folder; delete the folder afterwards. `diag_reproduce` temporarily adds and removes an NTFS *deny* entry for the current user on a sub-folder it created.

## 5. What `build.ps1` does, step by step

You can run each step by hand, for example when adapting the process to another app.

1. **Environment** (`env.ps1`): puts `.cargo\bin` on PATH, sets `CARGO_TARGET_DIR` and `ASR_INSTALLER_OUT`, and changes to the project root.
2. **`npm ci`** if `node_modules` is missing.
3. **NSIS cache repair** (see Troubleshooting T3).
4. **Tests** (`test.ps1`). Any failure stops the build:
   - `python -m unittest discover -s outlook_worker/tests -v` (25 tests)
   - `npx svelte-check --tsconfig ./tsconfig.json` (0 errors, 0 warnings expected)
   - `cargo test --no-default-features` in `src-tauri` (64 tests, plus one ignored opt-in test `migrate_real_copy` for checking a migration on a copy of a real database: `set ASR_DB_COPY=<copy> && cargo test --no-default-features -- --ignored migrate_real_copy --nocapture`). The `app` feature, which is the Tauri shell, is off, so the core logic can be tested without WebView2.
5. **Worker exe** (`-WithWorkerExe` only):
   `python -m PyInstaller --onefile --noconsole --name outlook_worker --distpath outlook_worker outlook_worker/outlook_worker.py`.
   Then it writes `src-tauri/tauri.conf.windows.json`, a copy of `tauri.conf.json` with `outlook_worker.exe` added to `bundle.resources`.
6. **`npx tauri build [--config src-tauri/tauri.conf.windows.json]`**:
   - runs `beforeBuildCommand` (`npm run build`, the Vite production build into `dist-web/`)
   - runs `cargo build --release --features app` for the MSVC target
   - downloads NSIS 3.x into `%LOCALAPPDATA%\tauri` the first time (hash-validated)
   - writes the bundle-type marker into the exe, so its hash differs from an unbundled build, and builds the NSIS installer (`installMode: currentUser`)
7. If the bundler fails because of the NSIS cache rename, it **repairs the cache and retries once**. Any other failure stops with an error. The script never reports success after a failed bundle.
8. Picks the installer **created by this run** and copies it to `%USERPROFILE%\Documents` (or `-OutputDir`).
9. Checks the Falcon log for block or quarantine events since the build started, and prints the installer path, size, build time and SHA-256.

---

## 6. Verifying a build

```powershell
scripts\verify-install.ps1                    # automatic checks
scripts\verify-install.ps1 -OutlookDraftTest  # also creates ONE labelled test draft in Outlook
```

Requirements: a normal PowerShell, and classic Outlook running for the Outlook checks.

| Check | How |
|---|---|
| Installs without admin rights | `setup.exe /S`, then reads the HKCU uninstall entry |
| Launches as a normal user | `Start-Process explorer.exe "<exe>"` gives a Medium-integrity process even if the parent shell is elevated. Waits for the main window and checks that it responds |
| Backend started | `backend ready startup_ms=â€¦` line in `%APPDATA%\gov.nj.doh.asrsupportdesk\logs\support-desk.<date>.log`. ERROR lines are shown |
| SQLite initialised | `verify_install.py db` opens `support_desk.db` **read-only** (`mode=ro`): `integrity_check`, table row counts |
| SOP catalog loaded | `issue_definitions` = **18**, `email_templates` > 0 (39 today) |
| Outlook integration | `verify_install.py worker`: `test_connection` through the **installed** `outlook_worker.exe` |
| Draft creation (optional) | `-OutlookDraftTest` saves one draft titled `[TEST - safe to delete] ASR Support Desk build verification draft`, with no recipient. The result must say `"sent": false`. Delete the draft afterwards |
| No security blocks | Falcon events since the check started |

The app and the worker only ever **save** drafts. `outlook_worker.py` never calls `.Send()`. Do not add any verification step that sends mail.

**Manual check** (cannot be automated; it needs the UI):

1. In the app window, run **Check Now** and open a ticket. Check its issue types and the school and sender fields.
2. Click **Copy Prompt**, paste into Notepad, and compare it with the preview. They must match exactly.
3. Click **Open Outlook Reply**. A threaded Reply window with your signature must open, with nothing sent.
4. With a completion folder set and a **test** email, click **Mark Done**. The email must be flagged complete and moved, and the ticket must become Done.

Never run the app, the worker or the Outlook checks **as Administrator**. COM between an elevated process and the normal Outlook fails with `0x80080005`, and the worker reports `OUTLOOK_UNAVAILABLE`.

---

## 7. CrowdStrike Falcon

**History.** Until 2026-11-17 morning, Falcon blocked and quarantined freshly built executables on this PC: cargo build scripts, test binaries and PyInstaller output. Every `cargo build/test` therefore failed with `Access is denied. (os error 5)`. Since 2026-11-17, with the MSVC toolchain and the `%LOCALAPPDATA%\asr-support-desk-target-msvc` target folder, no block events have occurred for cargo, PyInstaller, NSIS, the installer or the installed app.

**How to check** (read-only, no admin needed):

```powershell
Get-WinEvent -LogName "CrowdStrike-Falcon Sensor-CSFalconService/Operational" -MaxEvents 30 |
  Where-Object Id -in 3,4 | Format-Table TimeCreated, Id, Message -AutoSize
```

- ID **3**: "A process was blocked because malicious behavior was detected."
- ID **4**: "A file was quarantined because malicious behavior was detected."

The local log does not contain the file name, hash or detection name. Those are only in the Falcon console, which the security team can see.

**If builds are blocked again:**

1. Stop. Do **not** disable, pause or uninstall the sensor, rename files to evade it, build from other folders to dodge it, or use containers or VMs as a workaround without approval.
2. Collect:
   - the exact failing command and the `Access is denied` line, which names the exe path (usually `...\build\<crate>-<hash>\build-script-build.exe` or `...\deps\*.exe`)
   - the time stamps of the matching event ID 3 and 4 entries (the command above)
   - the host name (`$env:COMPUTERNAME`) and user name
3. Ask the endpoint security team for one of:
   - a Falcon ML/IOA exclusion for the build folder `%LOCALAPPDATA%\asr-support-desk-target-msvc\` (and `%LOCALAPPDATA%\tauri\NSIS\` if makensis is blocked) on the developer PC
   - a build machine or CI runner outside this prevention policy
4. For **distribution to staff PCs**, the installer and app are unsigned today. If Falcon or SmartScreen blocks them there, ask for code signing (preferred) or an allow-list entry by SHA-256. `build.ps1` prints the hash.

---

## 8. Troubleshooting

| ID | Symptom | Cause | Fix |
|---|---|---|---|
| T1 | `winget install ...` â†’ `This operation is disabled by Group Policy: Enable Windows Package Manager` | Organisation policy | Use Software Center. Do not work around the policy |
| T2 | `error: linker 'link.exe' not found` / `LNK1181: cannot open input file 'kernel32.lib'` | Build Tools C++ workload or Windows SDK missing | Install them from Software Center. Check with `scripts\doctor.ps1` |
| T3 | `failed to bundle project: Access is denied. (os error 5)` right after `Info extracting NSIS`, and **no** Falcon event at that time | Tauri extracts NSIS to `%LOCALAPPDATA%\tauri\nsis-3.xx` and renames it to `NSIS`. The rename fails while the new files are still open | `build.ps1` finishes the rename and retries automatically. By hand: `Rename-Item "$env:LOCALAPPDATA\tauri\nsis-3.13" NSIS`, then rebuild |
| T4 | `Access is denied. (os error 5)` running `build-script-build.exe` or a test exe, **with** Falcon event 3 or 4 | Falcon block | Section 7. Stop and report |
| T5 | `build.ps1` printed "Installer:" but no new installer exists | Old script ignored the `tauri build` exit code | Fixed: the script now stops on failure and only copies an installer created by the current run |
| T6 | Rust rebuilds everything, or builds into `...\asr-support-desk-target` | An inherited `CARGO_TARGET_DIR` from another session | `env.ps1` now always sets it. Outside the scripts, set it yourself (section 3.3) |
| T7 | `cmd /c ...` â†’ `Cannot run a document in the middle of a pipeline: C:\rtools45\usr\bin\cmd` | Rtools' `usr\bin` is early on PATH and shadows `cmd` and `link` | Use `"$env:WINDIR\System32\cmd.exe"`. rustc is not affected because it finds the MSVC `link.exe` through vswhere |
| T8 | Worker answers `params must be an object` when fed from PowerShell | PowerShell's stdin writer adds a UTF-8 BOM, so the JSON fails to parse | Feed it from Python (`scripts\verify_install.py`) or from the app. The Rust app writes plain bytes |
| T9 | Worker returns `OUTLOOK_UNAVAILABLE` / `0x80080005` | App or shell is elevated, Outlook is not | Run both non-elevated. Launch through `explorer.exe` |
| T10 | `Start-Process setup.exe -Wait` never returns | `-Wait` also waits for the processes the installer leaves running | Use `$p = Start-Process ... -PassThru; $p.WaitForExit()` (as `verify-install.ps1` does) |
| T11 | Installer installs into a OneDrive folder | NSIS reuses the `InstallLocation` of the previous install | Uninstall (Settings > Apps, or `uninstall.exe`), then reinstall. The default is `%LOCALAPPDATA%\ASR Audit Support Desk` |
| T12 | `svelte-check` or Vite errors | Frontend code or deps | `npm ci`, then `npx svelte-check --tsconfig ./tsconfig.json` for details |
| T13 | PyInstaller output blocked or quarantined | Falcon (section 7), since unsigned one-file packers are often flagged | Report it. Alternatively build without `-WithWorkerExe`; staff PCs then need Python 3.9+ and pywin32 |

---

## 8b. Portable ZIP (no installer)

```powershell
scripts\build-portable.ps1   # tests -> outlook_worker.exe -> release exe (tauri build --no-bundle) -> ZIP + SHA-256
```

The output is `dist\ASR-Audit-Support-Desk-<version>-windows-x64.zip` and `.zip.sha256`. The version comes from `tauri.conf.json`, and the build stops if it differs from `package.json` or `Cargo.toml`. The ZIP holds one folder:

| File | Notes |
|---|---|
| `ASR Audit Support Desk.exe` | The release app. SOP templates, the catalog, SOP images and the UI are embedded |
| `outlook_worker\outlook_worker.exe` | The PyInstaller worker, with Python and pywin32 inside. It is built by the shared `scripts\worker-exe.ps1`, the same as for the installer |
| `README.txt` | For recipients, from `scripts\portable\README.txt`: extract first, which exe to open, Outlook and WebView2 requirements, no admin, data location, updating |
| `MANIFEST.txt` | Size and SHA-256 of each file |
| `ASR-Support-Desk-User-Guide.html` | The illustrated guide, if it has been built |

How it works:

- **Locating the worker:** the app finds the worker through Tauri's resource directory, which is the exe's folder. That works for any extract path, including paths with spaces, and doesn't depend on the working directory.
- **No source-tree fallback:** release builds no longer fall back to the source checkout's `outlook_worker\` folder; only debug builds do.
- **Diagnostics:** the resolved worker is written to the log (`outlook worker resolved`) and shown under Settings › Database / Backup.
- **No redistributables:** the exe imports only Windows system DLLs. Tauri links the VC runtime statically, and the UCRT is part of Windows 10/11. WebView2 must already be on the PC; it ships with Windows 11 and Edge.
- **User data** stays in `%APPDATA%\gov.nj.doh.asrsupportdesk`, the same folder the installed version uses, so it is kept across updates.
- **Package contents check:** the script refuses to package anything except the files listed above, so no databases, settings, logs or test data get in.
- **Single instance:** only one copy of the app can run. Close the installed copy before starting the portable one.

Vite now builds the UI into `dist-web\` instead of `dist\`, so frontend builds no longer wipe the release packages in `dist\`.

## 8a. Rebuilding the user guide (Quarto)

The staff guide `docs/guide/ASR-Support-Desk-User-Guide.html` is one self-contained HTML file with a screenshot for every step. All screenshots use **fictional demo data**; no real mailbox or database is read.

```powershell
scripts\build-guide.ps1                    # demo data -> screenshots -> render; also copies the HTML to %USERPROFILE%\Documents
scripts\build-guide.ps1 -SkipScreenshots   # only re-render after editing the text
```

| Piece | Location |
|---|---|
| Guide text (Markdown + Quarto callouts) | `docs/guide/user-guide.qmd`, styling in `guide.css`, options in `_quarto.yml` |
| Demo data | `src-tauri/src/demo.rs`, an ignored test `write_docs_fixtures`. It runs the real extraction and prompt code on made-up emails and writes `docs/guide/demo/fixtures.json` |
| UI with a fake backend | `docs/guide/demo/` (`mockIPC` from `@tauri-apps/api/mocks`) and `vite.docs.config.ts` (port 1430). The app build is not affected |
| Outlook/Copilot illustrations | `docs/guide/mockups/mockup.html?view=start\|copilot\|reply\|pasted\|completed` |
| Screenshot script | `docs/guide/capture.py` (Python Playwright driving the installed Microsoft Edge, red numbered markers) â†’ `docs/guide/images/` |

Requirements:

- Python with `playwright`, using the installed Edge (no browser download needed).
- Node modules.
- **Portable Quarto** in `%LOCALAPPDATA%\Programs\quarto`. It was installed from the official `quarto-<ver>-win.zip` (GitHub quarto-dev/quarto-cli releases), with the SHA-256 checked against the release's `checksums.txt`, and needs no admin rights. Quarto 1.11.9 ran with no Falcon events.

After a UI change, rerun the script. If a control was renamed, `capture.py` fails with a clear "locator" error that names the missing element.

## 9. Release checklist

1. `git status` is clean, and the version is bumped in **both** `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`.
2. `scripts\doctor.ps1` shows no FAIL.
3. `scripts\build.ps1 -WithWorkerExe` passes all tests, then shows the installer path, size and SHA-256, with no Falcon warning.
4. `scripts\verify-install.ps1 -OutlookDraftTest` passes. Delete the test draft afterwards.
5. Do the manual UI check from section 6.
6. Record the installer name, size, SHA-256 and date in the release notes. Hand out the copy from `%USERPROFILE%\Documents`.

---

## 10. Reusing this setup for another Rust/Tauri app

1. **Toolchain:** `rustup override set stable-x86_64-pc-windows-msvc` in the new project root and its `src-tauri`. Leave the global default alone.
2. **Target dir:** add an `env.ps1` that always sets `CARGO_TARGET_DIR=%LOCALAPPDATA%\<app>-target-msvc`, outside OneDrive and in a folder Falcon allows.
3. **Testable core:** put the Tauri dependencies behind a Cargo feature (here `app`, in `default`), so `cargo test --no-default-features` runs the business logic without WebView2:
   ```toml
   [features]
   default = ["app"]
   app = ["dep:tauri", "dep:tauri-build", ...]
   [[bin]]
   name = "<app>"
   required-features = ["app"]
   ```
   Make `build.rs` call `tauri_build::build()` only when the feature is on.
4. **Bundle:** in `tauri.conf.json`, set `bundle.targets: ["nsis"]` and `bundle.windows.nsis.installMode: "currentUser"`, so installing needs no admin rights.
5. **Build script:** copy `scripts\build.ps1`, which includes the `tauri build` exit-code check, the NSIS cache repair, copying the installer from the current run only, and the Falcon check.
6. **Verification:** copy `scripts\verify-install.ps1` and adapt the read-only data checks. Always launch through `explorer.exe` (non-elevated).
7. **Security:** never weaken endpoint controls. If something is blocked, collect the evidence (section 7) and ask IT.
