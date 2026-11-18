# ASR/Audit Support Desk

A lightweight Windows desktop app for the ASR/Audit support mailbox. It:

- checks an Outlook folder (manually or on a schedule) using your classic Outlook profile, including shared mailboxes
- turns emails into **support cases** (one case can hold several schools, issue types and emails) with workflow statuses **Unassigned / Assigned / In Progress / Review / Done** and one current **owner**
- shares cases, owners, notes and history between computers through a **shared Microsoft Access database** on a network share, while always saving locally first (works offline; see [docs/SHARED_DATABASE.md](docs/SHARED_DATABASE.md))
- **Claim / Take over** change ownership without touching Outlook; **Mark Done** is a recorded staff decision (no sent-reply verification); a new reply to a Done case moves it to **Review**
- **Diagnostics** with a copyable, redacted event log
- assigns tickets to staff: moves the ticket's emails into the person's Outlook folder and emails them a handoff summary
- shows **Current Tickets** (emails now in the selected folders), **Awaiting Verification** and **Ticket History**, verified against Outlook on every check
- extracts issue types (several per email), error messages, school details (several schools per email, with NJIIS and other IDs, address, municipality, county and ZIP) and sender details (title, phone, extension)
- builds **one plain-text Copilot prompt** per ticket from the saved data, the approved SOP wording and the original email

**Workflow:** Scan → **Claim** → Review case → **Copy Prompt** → paste into Microsoft Copilot → **Open Outlook Reply** → paste Copilot's reply and send it yourself → **Mark Done** (decision saved at once) → Outlook archiving of the covered emails (immediately on a standalone computer; **Archive in Outlook** after the shared database confirmed the decision).

What the app does in Outlook:

- **Scanning and verification** only read. They never mark emails as read, move them or change their flags.
- **Open Outlook Reply** only shows Outlook's own Reply window. Nothing is inserted, saved or sent.
- **Assign / Reassign** moves the ticket's incoming emails into the staff member's folder (no flag or read-state change), then sends **one handoff email to that staff member only**. This is the only email the app sends by itself; replies to schools are always sent by staff.
- **Mark Done** records the decision; the incoming emails it covered are then flagged *Complete* and moved to the completion folder (standalone: right away; shared database: when you click **Archive in Outlook** after the decision is confirmed). Emails that arrived later are never archived under an older decision.
- **Claim**, **Take over**, **Review**, notes and **Add to existing case** never change Outlook.
- With the shared database, **Assign / Reassign** (Outlook moves + handoff) is withheld until the next phase adds shared move reservations (P1); on a standalone computer it works as before.
- Nothing is ever deleted, and there is no AI/API integration. You paste the prompt into Copilot yourself.

For staff, the **illustrated step-by-step guide** is [docs/guide/ASR-Support-Desk-User-Guide.html](docs/guide/ASR-Support-Desk-User-Guide.html): one HTML file with a screenshot for every step, using fictional demo data. Rebuild it with `scripts\build-guide.ps1`. See [docs/USER_GUIDE.md](docs/USER_GUIDE.md) for the shorter text reference. See [docs/TEMPLATE_COVERAGE.md](docs/TEMPLATE_COVERAGE.md) for how each template maps to the SOP, and [docs/BUILD_WINDOWS.md](docs/BUILD_WINDOWS.md) for building.

---

## Status and verification

### Shared team database release (1.1.0, phases P0.1–P0.3) — last verified 2026-11-18

| Area | Status |
|---|---|
| Shared Access database (P0.1) | Done. Schema v1, identity/compatibility/maintenance, offline queue bound to the database identity, idempotent operations, field-level merge with conflicts, duplicate-import convergence, pull with overlap + full reconciliation, standalone mode and restartable import. **Verified with the real 64-bit Access ODBC driver** on local test `.accdb` files (create, lock, read-only, maintenance, unicode, long text, lost acknowledgement) |
| Team workflow (P0.2) | Done. Five statuses, Claim/Take over/Clear owner, Start Work, Review with reason, Mark Done with coverage, stale-Done protection, Review on new replies, Add to Existing Case, Team/My/Unassigned views |
| Diagnostics (P0.3) | Done. Redacted bounded log, Copy Log/Copy Selected, latest error; failures reproduced with the real driver (unreachable share, missing file, NTFS-denied folder, maintenance, missing driver) |
| Automated tests | 120 core tests (+6 opt-in real-driver tests); 33 multi-workstation scenarios also pass against real `.accdb` files (`ASR_SHARED_BACKEND=access`); 37 Python worker tests (fake COM); `svelte-check` clean |
| Default shared path | The file at the team default path exists but is **not** an ASR shared database; the app refuses it read-only and never changes it. A deliberate decision on the production file name/location is needed (see docs/SHARED_DATABASE.md §3) |
| **Two-computer pilot** | **Not run — gate open.** Needs two workstations, a test network share, a test database and test Outlook messages. See [docs/PILOT_CHECKLIST.md](docs/PILOT_CHECKLIST.md) |
| Real network share / two physical computers / live Outlook with the new workflow | **Not verified** |

### Earlier release (1.0.0) — last verified 2026-11-17

| Area | Status |
|---|---|
| SOP review, issue catalog, 26 template bodies (39 template entries, 18 issue types) | Done. Catalog cross-checked: all files, placeholders, conditions, field sets and SOP images resolve |
| Python Outlook worker (16 operations) | Done. 37 tests pass with fake COM objects. They cover: reply window, Mark Done flag and move, assignment moves (no flag/read change, no second move), scope inventory (no date/unread filter, incomplete detection), locate found/not-found/error, notification send (one recipient, header, account, prepare vs. uncertain send stage), Sent Items lookup, notification marker detection |
| Staff assignment / current-history (v3) | Done. Rust tests cover staff validation and deactivation, assignment/reassignment, bulk and partial failures, retries without duplicate moves or notifications, new Outlook IDs after moves, notification content/recipient/failure/uncertain outcomes, notifications never imported, in-scope/out-of-scope visibility, manual moves and return to scope, conflicts, failed/partial/stale scans, folder changes, migration v2→v3, Mark Done after assignment. **Live Outlook moves and sends have not been run in automated tests** |
| Live classic Outlook (16.0) | Scanning, folders and connection were tested against the real profile before the Copilot workflow. **The new operations (open reply, Mark Done flag/move, validate folder) have not yet been tested against live Outlook** |
| Svelte frontend | Done. `svelte-check`: 0 errors, 0 warnings. `vite build` succeeds |
| Rust backend | **Compiles** with the MSVC toolchain (Rust 1.100.0, VS 2022 Build Tools). 87 core tests pass (2 opt-in ignored). They cover: migration v1→v2 and v2→v3 without data loss, prompt generation, completion progress and retries, search and inclusive date filters, staff edits surviving rescans, reopen on new replies. Tauri release build succeeds |
| Database migration v2 | Tested on a copy of the real database: 75 tickets, 346 fields, 39 templates, 81 emails and 3 old drafts unchanged. Statuses mapped to In Progress/Done, the old "Needs Review" kept as a badge, and a backup written first |
| Windows installer | **Produced** by `scripts\build.ps1 -WithWorkerExe` (NSIS, per-user, about 15 MiB). Installs without admin rights, launches, the SQLite DB initialises, and 18 issue types and 39 templates load |
| CrowdStrike Falcon | No block events for MSVC builds in `%LOCALAPPDATA%\asr-support-desk-target-msvc` (earlier GNU builds were blocked). See [docs/BUILD_WINDOWS.md](docs/BUILD_WINDOWS.md#7-crowdstrike-falcon) |
| Code signing | Not signed yet. Staff PCs may need signing or a Falcon allow-list entry |
| Performance measurements | Script ready (`scripts/measure.ps1`), **not yet run** |

**How to build, test, package and verify:** see **[docs/BUILD_WINDOWS.md](docs/BUILD_WINDOWS.md)**.

## Requirements

| Component | Version |
|---|---|
| Windows | 10/11 with WebView2 (preinstalled on Windows 11) |
| Outlook | Classic Outlook for Windows, with the shared mailbox in the profile (the "new Outlook" has no COM interface) |
| Python | 3.9+ with `pywin32` on the target PC. Alternatively, build `outlook_worker.exe` with `scripts\build.ps1 -WithWorkerExe` |
| Build only | Node.js 20+, Python 3.9+, Rust (rustup) with the **MSVC** toolchain as a project override, and VS 2022 Build Tools (C++ workload + Windows SDK, from Software Center). See [docs/BUILD_WINDOWS.md](docs/BUILD_WINDOWS.md) |

## Commands

Run these in a normal (non-administrator) PowerShell:

```powershell
scripts\doctor.ps1                 # check the build environment (-Fix sets up the MSVC Rust toolchain)
scripts\test.ps1                   # Python worker tests + frontend type check + Rust core tests
scripts\dev.ps1                    # run in development mode
scripts\build.ps1 -WithWorkerExe   # tests, then NSIS per-user installer -> copied to %USERPROFILE%\Documents
scripts\build-portable.ps1         # tests, then portable ZIP (no installer, no admin) -> dist\ASR-Audit-Support-Desk-<ver>-windows-x64.zip + .sha256
scripts\verify-install.ps1         # install, launch and check the new build (-OutlookDraftTest for a test draft)
scripts\measure.ps1                # idle CPU / memory of a running instance
```

Rust build output goes to `%LOCALAPPDATA%\asr-support-desk-target-msvc` (outside OneDrive).

## Architecture

```
Svelte UI (WebView2)  --invoke-->  Rust (Tauri)  --JSON stdin/stdout-->  python outlook_worker.py (short-lived)
                                     |  SQLite (WAL)                       |  pythoncom.CoInitialize / win32com
                                     |  settings.json                      |  one operation per process, then exit
```

- **Rust** handles scheduling, ticket operations, classification, persistence, templates and placeholder rendering.
- **Shared database** (`shared_store`, `shared_apply`, `shared_workflow`, `sync_queue`, `shared_sync`): the shared Access file is reached through the 64-bit ODBC driver (`odbc-api`) on a background thread with time limits. Every local edit and its queued operation are saved in one SQLite transaction; each operation is applied in one short Access transaction together with its activity, version, change-feed and processed-operation rows. Tests run the same apply logic against SQLite files and, opt-in, real `.accdb` files.
- **Python** handles Outlook COM only. Each operation starts a new process (`-I` isolated mode, no console window, with a timeout). COM is initialized on that process's main thread and released at exit, so no live COM object ever crosses a process boundary. No Python process runs while idle.
- **Scheduler:** a single sleeping async task. It runs one sync at a time (overlapping runs are rejected) and reads incrementally from a per-folder watermark minus an overlap window. A full-lookback rescan every 24 h catches emails moved into the folder later. Transient Outlook errors get 3 bounded retries.
- **Deduplication:** by Internet Message-ID, then by EntryID. Emails moved between folders update their location without creating new tickets. Replies attach to the existing ticket through the ConversationID. A new incoming reply reopens a Done ticket; rescans of known emails never do. Probable duplicates (same sender or school, same issue type, within 14 days) get a badge and are never merged.
- **Statuses and owner:** `Unassigned`, `Assigned`, `In Progress`, `Done`, stored separately from the assignee (`tickets.assignee_staff_id`) and from current/history placement (`tickets.scope_state`). Low-confidence classification, possible duplicates, "needs review" and ownership conflicts are badges. `Assigned` is set by an assignment (or an owner inferred from a staff folder), `In Progress` by Start Work, `Done` only by Mark Done (`completion.rs`). A Done ticket must be reopened before it can be assigned.
- **Staff** (`staff.rs`): name, notification email, Outlook folder (StoreID + folder EntryID + path). A staff folder covers its descendants unless a more specific staff folder exists. Deactivated staff keep their history but cannot receive assignments or be inferred as owners.
- **Assignment** (`assignment.rs` + worker `move_messages` / `send_notification`): moves the ticket's incoming source emails (never replies, drafts, staff emails or notifications) to the staff folder with per-email progress (`assignment_moves`), finalizes only when every move succeeded, then sends the handoff email. Notification states `pending → sending → submitted | failed | uncertain` (or `skipped` when the owner did not change); the claim is atomic so repeated clicks cannot send twice, "submitted" means accepted by Outlook (delivery not confirmed), and "uncertain" is never resent automatically (Check Sent Items by the notification id, or an explicit Send again). Notifications carry an `X-ASR-Desk-Notification` header and a body marker and are never imported as tickets.
- **Scope reconciliation** (`scope.rs` + worker `scope_inventory` / `locate_messages`): after each import, a complete inventory of the selected folders (all items, no date window or unread filter, completion folder excluded) is compared with every known email. Presence is always applied; absence only when the inventory is complete and still matches the selected folders (results for an older selection are discarded). Emails that left the scope are located (lookup only) to record why: completed, reassigned (in a staff folder), moved, not found (never called "deleted"), or unknown (lookup error). Owners are inferred from staff folders; emails in two staff folders flag a conflict instead of choosing an owner. Folder changes found by scanning never send notifications.
- **Copilot prompt** (`prompt.rs`): built from saved data only. It includes the editable instructions, ticket, sender, all schools, staff actions and notes, SOP excerpts (the current, possibly customized template for each issue, turned into plain text), the full original email, and the conversation context (only the new part of each later message). Missing values read `[Not provided]`, and truncated bodies are flagged explicitly. The UI copies exactly the previewed string; the last copied prompt and its time are stored.
- **Monitored folders:** a top folder plus a mode:
  - `none`: the top folder only.
  - `all`: the top folder and all its subfolders.
  - `selected`: the top folder plus chosen subfolders. Each chosen subfolder can include its own subtree (`selected_with_children`).
  
  The completion folder and its subtree are always excluded from scans (`exclude_entry_ids`). Settings loads only the monitored folder's subtree (`list_folders` with `root_entry_id`), which takes about 0.3 s instead of a full mailbox walk.
- **Open Outlook Reply:** the worker's `open_reply` calls `MailItem.Reply()` / `ReplyAll()` and `Display()`. It never edits, saves or sends. If the EntryID changed after a move (staff often file mail into other subfolders), the message is found again by Internet Message-ID. The search covers, in order: its last folder, the completion folder, the monitored folders, and finally the **whole tree under the top folder**. The new location is saved. This lookup never creates tickets.
- **Mark Done** (`completion.rs` + worker `complete_messages`): per-message progress is stored in `message_completion` (flagged_at, moved_at, destination, last_error, attempts). A retry only does the missing steps. A message is never moved twice: one already in the destination counts as moved, including after a crash between Outlook and SQLite. The ticket becomes Done only when every incoming email is flagged and moved. Sent replies, drafts and staff emails are excluded. A per-ticket guard blocks double clicks.
- **Search** (`search.rs`): an SQLite FTS5 index with one document per ticket. It covers fields, IDs (as text, so leading zeros are kept), sender details, subjects, email bodies, issues, notes and actions. It is rebuilt per ticket on every change. List queries are paginated and return summaries only.

| Path | Contents |
|---|---|
| `templates/catalog.json` | Issue types, form fields, keywords, template metadata and rules, placeholders |
| `templates/<issue>/*.html` | Default email bodies (SOP wording), used as SOP excerpts in the prompt |
| `templates/sop-images/` | SOP screenshots (used by the legacy draft feature) |
| `src-tauri/src/` | `analyzer` (extraction/classification), `render` (placeholders/rules, HTML→text), `templates_store`, `tickets`, `prompt`, `completion`, `search`, `drafts` (legacy), `sync_core`, `outlook` (worker bridge), `export`, `db`. Tauri-only: `shell`, `commands`, `sync` |
| `outlook_worker/` | `outlook_worker.py` and its tests |
| `src/` | Svelte UI |

### Data locations

Everything is stored under `%APPDATA%\gov.nj.doh.asrsupportdesk\`:

| File / folder | Contents |
|---|---|
| `support_desk.db` | SQLite: tickets, issues, schools and fields, emails, history, completion progress, search index, templates and versions, legacy drafts, sync state |
| `settings.json` | Configuration, including scan mode, completion folder and prompt instructions |
| `logs\` | Daily logs, kept 14 days. They contain no email bodies |
| `backups\` | Automatic safety copies: `pre-migration-v<N>-<time>.db` before a schema upgrade, `pre-restore-<time>.db` before a restore |

### Database migrations

The schema version is `PRAGMA user_version`, and migrations live in `db.rs`. Before upgrading an existing database, the app writes a consistent copy (`VACUUM INTO`) to `backups\`. If that backup fails, it does not upgrade.

**v2 (Copilot workflow)** makes these changes:

- **Statuses:** `Resolved`/`Closed` become `Done`, with `completed_at` set to the last update time. Every other status becomes `In Progress`. Each change is recorded in the ticket history, and `Needs Review` survives as the `needs_review` badge.
- **New tables:** `ticket_issues`, `ticket_schools`, `message_completion`, and the FTS5 index `ticket_search`.
- **New ticket columns:** `first_received_at`, `staff_actions`, `last_prompt`, `last_prompt_at`.

Tickets, fields, emails, templates, customizations, drafts and history are kept unchanged.

**v3 (staff assignment, current/history)** makes these changes (backup first, as always):

- **New tables:** `staff`, `assignment_ops` (operator, previous/new assignee, folders, handoff note, notification state/recipient/account/time/identifiers) and `assignment_moves` (per-email progress).
- **New ticket columns:** `assignee_staff_id`, `assigned_at`, `ownership_conflict`, `ticket_ref` (stable `ASR-XXXXXXXX` reference derived from the original email's Message-ID, the same on every computer), `scope_state` / `scope_reason` / `scope_changed_at`.
- **New email columns:** scope membership, last confirmed location (folder ancestors), inferred owner, lookup status.
- **Existing statuses are kept** (In Progress stays In Progress, Done stays Done). Every ticket with Outlook emails starts as *Awaiting Verification* until the first successful check; tickets without Outlook emails are current (open) or history (Done). The follow-up steps are repeatable and safe to resume.

### Templates and upgrades

System templates are seeded on first run. When a new app version ships a newer template version:

- **Unchanged templates** are updated, and the old text is kept in version history.
- **Customized templates** are never overwritten. They are flagged "update available", and *Restore System Default* adopts the new text.

### Data integrity rules

- Extracted values are stored separately from staff-entered and verified values. A later sync only fills extracted values and never touches staff entries. Issue types chosen by staff are not changed by later emails.
- Identifiers are text, so leading zeros are kept. NJIIS IDs and other IDs (ASR_ID, SHAD_ID, …) are kept in separate fields.
- Schools are grouped: an NJIIS ID is paired with the school name on the same or the previous line. A school address is taken only from a labelled line or the line directly below a school name, and only **before** the sign-off, so a signature address never becomes the school address. Sender title, phone and extension come from the signature only.
- `verified_only` placeholders, such as verified submitted grades, never take extracted guesses (this also applies in the prompt's SOP excerpts).
- The prompt describes an action as completed only if staff recorded it under *Staff actions already taken*.

## Known limitations

- **Several computers:** SQLite stays local (never put it on a network drive). With the shared Access database, cases, owners, statuses, notes and history are shared; Outlook EntryIDs, folder mappings and scan scope stay per computer. Local `T-` numbers are not unique across computers; the `ASR-` reference is. The two-computer pilot has not been run yet (see Status).
- The Windows account mapping only pre-selects "I am"; it does not secure the Access file. Protect the share folder with NTFS/share permissions.
- Assignment/notification history recorded by Assign (standalone) is local to that computer.
- The Team Overview only reconciles staff folders inside the selected folders; others are labelled "not reconciled".
- Assignment needs write access to the staff folders; notifications need an Outlook account the user may send from (Send As for a shared mailbox is not assumed).

- Shared mailboxes that are not in the Outlook profile's folder list can be opened by name, using the Inbox of the "shared mailbox" field.
- Do not start the app with **Run as administrator**. Windows blocks COM between an elevated program and the normal (non-elevated) Outlook, and the worker reports `OUTLOOK_UNAVAILABLE` (HRESULT `0x80080005`) with an explanation.
- Corporate setups without antivirus registration may show the Outlook "a program is trying to access…" security prompt when reading sender addresses.
- Classification is keyword-based and configurable in `catalog.json`. Staff should confirm the issue type. Tickets with low confidence or that look like duplicates get a badge.
- School and sender extraction is deliberately conservative. Addresses written inside a sentence, or schools named without an ID or label, may need to be entered by hand.
- In *Unread items only* scan mode, an email that was opened before the scan is not imported.
- Mark Done needs write permission on the completion folder. Flag and move failures are shown per email and can be retried.
- The legacy draft composer (`drafts.rs`, `draft_*` commands, `email_drafts`) is kept for stored data but is no longer shown in the UI.
