# ASR/Audit Support Desk

A lightweight Windows desktop app for the ASR/Audit support mailbox. It:

- checks an Outlook folder on a schedule, using your classic Outlook profile
- turns emails into tickets and extracts School NJIIS IDs, school names and error messages
- classifies each email into one of 18 SOP issue types
- drafts the SOP-based reply

Every issue type ships with ready-to-use default templates based on `SOP for ASR-Audit Problems.docx`. They work without Outlook and with incomplete ticket data. Missing values stay visible as placeholders.

Nothing is ever sent automatically. The app creates Outlook **drafts** only. It never moves, deletes, archives or marks emails as read.

See [docs/USER_GUIDE.md](docs/USER_GUIDE.md) for staff instructions. See [docs/TEMPLATE_COVERAGE.md](docs/TEMPLATE_COVERAGE.md) for how each template maps to the SOP.

---

## Status and verification

Last verified 2026-11-16.

| Area | Status |
|---|---|
| SOP review, issue catalog, 26 template bodies (39 template entries, 18 issue types) | Done. Catalog cross-checked: all files, placeholders, conditions, field sets and SOP images resolve |
| Python Outlook worker (6 operations) | Done. 18 tests pass (fake COM objects + subprocess contract) |
| Live classic Outlook (16.0) | **Tested.** All 6 operations pass against the real profile, including the shared *DOH-Schoolimmforms-team* mailbox (folder tree, Inbox + 30 subfolders). One test draft was created in Drafts: not sent, no recipients, inline SOP image attached |
| Svelte frontend | Done. `svelte-check`: 0 errors, 0 warnings. `vite build` succeeds |
| SQLite schema and queries | Schema applied in SQLite. All 75 static SQL statements in the Rust code prepare without errors against it (checked through Python `sqlite3`, not through Rust) |
| Rust backend (DB, analyzer, tickets, drafts, scheduler, export, Tauri shell) | Written, **not yet compiled**. Blocked by endpoint security. See below |
| Windows installer | Script ready (`scripts/build.ps1`), **not yet produced** (needs the Rust build) |
| Performance measurements | Script ready (`scripts/measure.ps1`), **not yet run** |

### Why the Rust code has not been compiled on the development PC

The blocker is **CrowdStrike Falcon**, not Windows Code Integrity. User-mode Code Integrity, Smart App Control and AppLocker are not enforcing on this PC, and local administrator rights do not change the result. Rust links fine (GNU toolchain), but Falcon blocks and quarantines the first newly built program as soon as it runs. Cargo always runs one first, a build script, so `cargo build`, `cargo check` and `cargo test` all fail with `Access is denied (os error 5)`. The events are `CrowdStrike-Falcon Sensor-CSFalconService/Operational` IDs 3 ("process was blocked") and 4 ("file was quarantined"). The detection name and file hash are only in the Falcon console. The test executables, `outlook_worker.exe` (PyInstaller) and the finished app would be blocked the same way.

The endpoint security team must act on this. Endpoint protection must not be disabled or worked around locally. Ask them for one of:

1. A **Falcon exclusion** (machine-learning/IOA) for the Rust build folder `%LOCALAPPDATA%\asr-support-desk-target` on this PC. Then run `scripts\build.ps1 -WithWorkerExe` here.
2. A build machine or CI runner that is not under this prevention policy. Run `scripts\build.ps1 -WithWorkerExe` there.

Either way, staff PCs with the same policy will also need the finished app **code-signed** or allow-listed (by hash or certificate) in Falcon.

The core logic has no Tauri dependency (Cargo feature `app`), so on any unrestricted machine:

```powershell
cd src-tauri
cargo test --no-default-features   # core unit tests, no WebView/Tauri needed
```

## Requirements

| Component | Version |
|---|---|
| Windows | 10/11 with WebView2 (preinstalled on Windows 11) |
| Outlook | Classic Outlook for Windows, with the shared mailbox in the profile (the "new Outlook" has no COM interface) |
| Python | 3.9+ with `pywin32` on the target PC. Alternatively, build `outlook_worker.exe` with `scripts\build.ps1 -WithWorkerExe` |
| Build only | Node.js 20+, Rust (rustup, stable), plus MSVC Build Tools **or** the GNU toolchain with MinGW-w64 |

## Commands

```powershell
scripts\test.ps1     # Python worker tests + frontend type check + Rust core tests
scripts\dev.ps1      # run in development mode
scripts\build.ps1    # tests, then NSIS per-user installer (no admin needed to install)
scripts\measure.ps1  # idle CPU / memory of a running instance
```

Rust build output goes to `%LOCALAPPDATA%\asr-support-desk-target` (outside OneDrive).

## Architecture

```
Svelte UI (WebView2)  --invoke-->  Rust (Tauri)  --JSON stdin/stdout-->  python outlook_worker.py (short-lived)
                                     |  SQLite (WAL)                       |  pythoncom.CoInitialize / win32com
                                     |  settings.json                      |  one operation per process, then exit
```

- **Rust** handles scheduling, ticket operations, classification, persistence, templates and placeholder rendering.
- **Python** handles Outlook COM only. Each operation starts a new process (`-I` isolated mode, no console window, with a timeout). COM is initialized on that process's main thread and released at exit, so no live COM object ever crosses a process boundary. No Python process runs while idle.
- **Scheduler:** a single sleeping async task. It runs one sync at a time (overlapping runs are rejected) and reads incrementally from a per-folder watermark minus an overlap window. A full-lookback rescan every 24 h catches emails moved into the folder later. Transient Outlook errors get 3 bounded retries.
- **Deduplication:** by Internet Message-ID, then by EntryID. Emails moved between folders update their location without creating new tickets. Replies attach to the existing ticket through the ConversationID. Probable duplicates (same sender or school, same issue type, within 14 days) are flagged for review and never merged.

| Path | Contents |
|---|---|
| `templates/catalog.json` | Issue types, form fields, keywords, template metadata and rules, placeholders |
| `templates/<issue>/*.html` | Default email bodies (SOP wording) |
| `templates/sop-images/` | SOP screenshots, sent as inline (CID) images in Outlook drafts |
| `src-tauri/src/` | `analyzer` (extraction/classification), `render` (placeholders/rules), `templates_store`, `tickets`, `drafts`, `sync_core`, `outlook` (worker bridge), `export`, `db`. Tauri-only: `shell`, `commands`, `sync` |
| `outlook_worker/` | `outlook_worker.py` and its tests |
| `src/` | Svelte UI |

### Data locations

Everything is stored under `%APPDATA%\gov.nj.doh.asrsupportdesk\`:

| File / folder | Contents |
|---|---|
| `support_desk.db` | SQLite: tickets, fields, emails, history, drafts, templates and versions, sync state |
| `settings.json` | Configuration |
| `logs\` | Daily logs, kept 14 days. They contain no email bodies |
| `backups\` | Automatic safety copy taken before a restore |

### Templates and upgrades

System templates are seeded on first run. When a new app version ships a newer template version:

- **Unchanged templates** are updated, and the old text is kept in version history.
- **Customized templates** are never overwritten. They are flagged "update available", and *Restore System Default* adopts the new text.

### Data integrity rules

- Extracted values are stored separately from staff-entered and verified values. A later sync only fills extracted values and never touches staff entries.
- `verified_only` placeholders, such as verified submitted grades, never take extracted guesses.
- Templates that confirm facts (subtype updated, account created, data received) declare `asserts`. The UI warns when those facts are not verified on the ticket. Creating an Outlook draft then requires explicit confirmation, and the draft is marked `[INCOMPLETE DRAFT]`.
- *Fill from Ticket* replaces only intact `{{placeholder}}` tokens, so manual edits survive. Regenerating over edited text saves the previous text as a version first.

## Known limitations

- Shared mailboxes that are not in the Outlook profile's folder list can be opened by name, using the Inbox of the "shared mailbox" field.
- Do not start the app with **Run as administrator**. Windows blocks COM between an elevated program and the normal (non-elevated) Outlook, and the worker reports `OUTLOOK_UNAVAILABLE` (HRESULT `0x80080005`) with an explanation.
- Corporate setups without antivirus registration may show the Outlook "a program is trying to access…" security prompt when reading sender addresses.
- Classification is keyword-based and configurable in `catalog.json`. Staff should confirm the issue type. Tickets with low confidence or that look like duplicates start in **Needs Review**.
- The rich-text editor uses `contenteditable`, which is enough for routine edits. HTML source mode is available.
