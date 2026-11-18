# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

ASR/Audit Support Desk: a Windows-only Tauri 2 desktop app (Svelte 5 UI + Rust backend + SQLite + a Python Outlook COM worker) that turns emails in a classic-Outlook folder into support tickets. `README.md` is the authoritative description of behaviour, data model, migrations and safety rules; `docs/BUILD_WINDOWS.md` covers the toolchain and packaging. Read the relevant README section before changing assignment, scope, completion or migration logic.

## Commands

Run from a **normal (non-administrator) PowerShell** at the repo root. Every script dot-sources `scripts/env.ps1`, which sets `CARGO_TARGET_DIR` to `%LOCALAPPDATA%\asr-support-desk-target-msvc` (outside OneDrive) and `cd`s to the root. If you run `cargo` by hand, set that env var first or you will trigger a multi-GB rebuild inside OneDrive.

```powershell
scripts\doctor.ps1                 # environment report (-Fix sets the MSVC rustup override)
scripts\test.ps1                   # all suites: Python worker, svelte-check, Rust core tests
scripts\dev.ps1                    # npx tauri dev
scripts\build.ps1 -WithWorkerExe   # tests -> PyInstaller worker exe -> NSIS installer (copied to %USERPROFILE%\Documents)
scripts\build-portable.ps1         # portable ZIP in dist\
scripts\verify-install.ps1         # silent per-user install + launch + DB/template/Outlook checks
scripts\build-guide.ps1            # rebuild docs/guide user guide (demo data -> screenshots -> Quarto)
```

Individual suites / single tests:

```powershell
# Rust core tests (Tauri shell excluded; this is what test.ps1 runs)
cd src-tauri; cargo test --no-default-features
cargo test --no-default-features <test_name_substring>
cargo test --no-default-features -- --ignored write_docs_fixtures   # opt-in: regenerates guide demo data

# Python worker (fake COM objects, no Outlook needed)
python -m unittest discover -s outlook_worker/tests -v
cd outlook_worker/tests; python -m unittest test_worker.<TestClass>.<test_method>   # test file adds outlook_worker/ to sys.path itself

# Frontend
npx svelte-check --tsconfig ./tsconfig.json
```

Toolchain: the machine's global Rust default is GNU; this project uses a **directory `rustup override` to `stable-x86_64-pc-windows-msvc`** (GNU build-script exes were blocked by CrowdStrike Falcon). Do not change the global default or try to bypass Falcon/Group Policy. If NSIS fails with "Access is denied" on a rename, `build.ps1` already repairs the `%LOCALAPPDATA%\tauri\NSIS` cache and retries.

## Architecture

```
Svelte UI (WebView2) --invoke--> Rust (Tauri) --JSON stdin/stdout--> outlook_worker.py (one process per op)
                                   SQLite (WAL) + settings.json         pythoncom / win32com
```

- **Rust crate is split by the `app` feature** (`src-tauri/Cargo.toml`). Everything in `lib.rs` not gated by `#[cfg(feature = "app")]` is pure core logic testable with `--no-default-features`. Only `commands.rs` (all `#[tauri::command]` handlers), `sync.rs` (scheduler + UI events) and `shell.rs` (app setup, `AppState`) depend on Tauri. Keep new business logic in core modules (e.g. `sync_core.rs`, `assignment.rs`, `scope.rs`, `completion.rs`) so it stays testable; command handlers should be thin wrappers.
- **Frontend ↔ backend contract**: `src/lib/api.ts` mirrors Rust structs as TS interfaces and wraps every `invoke` call. When a command signature or a serialized struct changes, update `commands.rs`, the Rust type, and `api.ts` together. Shared UI state lives in `src/lib/stores.svelte.ts` (Svelte 5 runes).
- **Outlook worker** (`outlook_worker/outlook_worker.py`): request `{"op", "params"}` → response `{"ok", "data", "error_code", "message", "diagnostics"}`. Rust side is `outlook.rs` (`WorkerConfig::resolve` prefers a bundled `outlook_worker.exe`, then configured Python, then `py`, then `python`, always with `-I`). A new Outlook operation needs: a handler in the worker, fake-COM tests in `outlook_worker/tests/test_worker.py`, and a Rust caller. The worker's module docstring lists the safety invariants (never delete, never change read state, moves only in `complete_messages`/`move_messages`, the only automatic send is `send_notification`, never log bodies) — preserve them.
- **Templates/catalog** (`templates/catalog.json` + `templates/<issue>/*.html`) are embedded into the binary at compile time via `include_dir!` in `catalog.rs`, then seeded into SQLite (`templates_store.rs`, versioned; customized templates are never overwritten). Editing a template file requires a rebuild; `docs/TEMPLATE_COVERAGE.md` maps templates to the SOP.
- **Pipeline**: `sync_core` (incremental watermark window, single-flight, retries) → `tickets` ingest (dedup by Message-ID then EntryID, ConversationID threading) → `analyzer` (keyword classification + school/sender extraction) → `scope` reconciliation (current / history / awaiting verification) → `search` FTS5 reindex per ticket. `prompt.rs` builds the Copilot prompt from saved data only; `render.rs` handles placeholders/conditions and HTML→text.
- **Multi-step Outlook side effects are resumable**: assignment (`assignment_moves`, notification state machine `pending → sending → submitted | failed | uncertain`) and Mark Done (`message_completion`) record per-email progress so retries only do missing steps and never move or send twice. Changes here must keep that idempotency.
- **Migrations** live in `db.rs`, keyed on `PRAGMA user_version`; a `VACUUM INTO` backup is written before any upgrade and the upgrade aborts if it fails. Add a new version rather than editing an existing one, and add a migration test.
- **Data integrity**: extracted values are stored separately from staff-entered/verified values and syncs never overwrite staff input; identifiers are stored as text (leading zeros); `verified_only` placeholders never use extracted guesses.
- **Shared database (Access, schema v2)**: `shared_store` (connections, DDL, identity, idempotent upgrade), `shared_apply` (one transaction per op, processed-op dedupe, conflicts), `shared_workflow` (claim/takeover/done/join), `shared_cases` (links, `email.move`, merge of schools/fields), `shared_outlook` (Outlook reservations: `outlook.reserve/progress/notify_claim/notify_result/finish`, applied ONLINE, never queued), `shared_admin` (upgrade with verified backup, integrity, backup), `shared_sync` (push/pull cycle), `sync_queue` (local queue, mirror, `apply_bundle`). New shared tables need a new `SCHEMA_STEPS` entry, an upgrade test, and a real-driver run (`ASR_SHARED_BACKEND=access ASR_ACCESS_TEST_DIR=...`) — Access rejects some column names (e.g. `note`).
- Local P2/P3 logic: `cases.rs` (suggestions, links, move email, merge preview/run, timeline), `bulk.rs`, `team.rs`. Outlook moves/handoffs in shared mode go through the reservation helpers in `commands.rs` (`online`, `apply_online`, `report_online`, `archive_shared`).
- **Profiles / test mode** (`profile.rs`): `ASR_APP_DATA_DIR`, `ASR_TEST_MODE`, `ASR_OUTLOOK=mock|live`, `ASR_OUTLOOK_MOCK_DIR`, `ASR_TEST_STAFF`, `ASR_DEVTOOLS_PORT`. Mock Outlook (`outlook_mock.rs`) is file-backed with a journal and fault injection. Dual-instance tests: `scripts\launch_multiuser_test.ps1` then `node scripts/multiuser/run-tests.mjs` (CDP into both WebView2 windows; see `docs/MULTIUSER_TESTING.md`).
- `drafts.rs` / `draft_*` commands / `create_reply_draft` are legacy, kept for stored data, not shown in the UI.
- `src-tauri/src/demo.rs` (test-only) generates the fictional fixtures for the user guide; `vite.docs.config.ts` + `docs/guide/demo/` run the real UI against a mocked backend for screenshots.

## Environment gotchas

- Never run the app or Outlook-touching tests elevated: COM across elevation levels fails with `OUTLOOK_UNAVAILABLE` (HRESULT `0x80080005`).
- Live Outlook moves/sends are not covered by automated tests; Python tests use fake COM objects only.
- App data: `%APPDATA%\gov.nj.doh.asrsupportdesk\` (`support_desk.db`, `settings.json`, `logs\`, `backups\`).
