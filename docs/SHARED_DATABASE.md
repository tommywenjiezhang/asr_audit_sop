# Shared Database — Setup, Operation and Recovery

This guide is for the person setting up the shared team database and for anyone who needs to recover from a problem. Staff only need the short version in the user guide.

## 1. What is stored where

| Place | What it holds | Shared? |
|---|---|---|
| **Local SQLite** (`%APPDATA%\gov.nj.doh.asrsupportdesk\support_desk.db`, on each computer) | A working copy of the team cases, changes waiting to be sent (the *queue*), conflicts, your Outlook folder mappings and scan scope, who you are on this computer, sync checkpoints, Diagnostics events | No. Never put it on a share or in OneDrive. |
| **Shared Access database** (`.accdb` on the network share) | Accepted team cases, staff (name, email, Windows account), owners and ownership history, notes, issue types, schools/fields entered by staff, emails linked to cases (with a saved copy of the text), completion history, activity history, conflicts, processed operations, change feed, database identity and schema version | Yes |
| **Outlook** | The actual emails and folders, flags, moves, handoff emails | Each user's profile |

The app always **saves on this computer first**. A change is then *queued* and sent to the shared database when it is reachable. If the share is down, the Access driver is missing or you have no permission, you keep working; the case shows **Saved locally — pending sync**.

## 2. Requirements

- **Microsoft Access Database Engine, 64-bit** (the ODBC driver "Microsoft Access Driver (\*.mdb, \*.accdb)"). The app is 64-bit and cannot use the 32-bit driver.
  - Check: *ODBC Data Sources (64-bit)* → *Drivers* tab lists *Microsoft Access Driver (\*.mdb, \*.accdb)*. 64-bit Microsoft 365/Office normally includes it.
  - If only 32-bit Office is installed, Settings shows **"Only the 32-bit Microsoft Access driver is installed"** (`ACCESS_DRIVER_ARCH_MISMATCH`). IT must install the 64-bit Access Database Engine (side-by-side installation with 32-bit Office needs IT guidance). The app keeps working locally meanwhile.
- **Network share permissions**: every staff member needs **Modify** (read, write, create, delete) on the **folder** that holds the `.accdb`, not only on the file. Access creates a lock file (`asr_email_support.laccdb`) next to the database while it is open; without folder write permission the database opens **read-only** (Settings shows *Read-only connection*; changes stay pending).
- The shared database must **not** be in a OneDrive-synchronized folder (Settings refuses such paths). OneDrive copies a live Access file while it is in use and can corrupt or fork it.

### Security note

The Windows account mapping in Settings › Staff Assignment only pre-selects *I am* on that person's computer. It does **not** secure the Access file: anyone with Modify permission on the share folder can open the file with Access. Protect the data with the **share/NTFS permissions** (only the support team), and keep the share backed up (section 7).

## 3. Default path and per-computer settings

Settings › **Shared Database**:

- **Mode**: *Shared* (default) or *Standalone*.
- **Database path** — prepopulated with the team default:
  `\\epi-qb-5\VPD_Shared\PgmDocs\Assessment and Coverage\Annual Immunization Status Report\ASR Master List Revamp--2023\asr_email_support.accdb`
  Each computer can change and save its own path.
- **Sync every** N minutes (default 2). A change also triggers a sync a few seconds later.
- **Test Connection** — checks, in the background: folder reachable → database readable (identity) → write permission (a small device heartbeat). It shows *Reachable / readable / write permission*, the database name and environment (test/production), its identity, schema compatibility and the plain-language error with the provider code (e.g. `SQLSTATE HY000 / native -1024`).
- **Sync Now** — sends waiting changes and receives the team's changes.
- **Open Diagnostics** — the copyable event log.

The app **never creates or overwrites** a database at the configured path. A missing file shows *"The database file was not found at this path"* (`DB_FILE_NOT_FOUND`).

The connection check reads the database identity through a **read-only** connection first. Only a confirmed, compatible ASR shared database is ever opened for writing (the device heartbeat and synchronization). Any other Access file at the path is reported as *not a compatible ASR Support Desk shared database* (`DB_INCOMPATIBLE`) and is not changed.

> **Production database (2026-11-18):** the file at the team default path was an **empty** Access database (system tables only) prepared for this purpose. It was initialized in place as the production shared database *ASR Support Desk* (database id `cf55090a-…`, schema v1) after a verified backup (`asr_email_support.backup-20261118-063021.accdb` next to it, SHA-256 identical to the original). The first computer's 477 local cases were then imported (see §6). Any other non-empty or non-ASR file at a configured path is still refused and never changed.

## 4. First-time setup (once per team)

1. Ask IT for a share folder with Modify permission for the support team only (the default path above, or another).
2. Make sure the 64-bit Access driver is installed on the computer doing the setup.
3. In Settings › Shared Database, open **Create a new shared database (setup / test only)**:
   - choose a **new** file name (the app refuses an existing file),
   - name it (e.g. *ASR Support Desk*) and choose **production** (or **test** for a pilot),
   - **Create database**. This writes the tables, a new **database identity** (a random id), the schema version and the creator.
4. Put that path in **Database path**, **Save**, **Test Connection**.
5. Every other computer: set the same path (if different from the default), **Save**, **Test Connection**.

**Initializing an existing empty database.** If an administrator has already created a *blank* `.accdb` on the share (as for the production path), Test Connection reports *empty Access database* and Settings shows **Initialize this database**. The app (or the `shared_setup` tool below) then: refuses if the file holds any table, linked table or query, is already an ASR database, or is open on another computer (`.laccdb` present / exclusive open fails); writes a byte-verified backup copy next to the file (`<name>.backup-<time>.accdb`, SHA-256 compared); opens the file **exclusively** and creates the tables and identity. It never creates, replaces or deletes the file.

Administrator tool (same core code, no UI; close the desktop app first):

```powershell
. scripts\env.ps1; cd src-tauri
cargo build --no-default-features --example shared_setup
$T = "$env:CARGO_TARGET_DIR\debug\examples\shared_setup.exe"
& $T inspect <path.accdb>                                   # read-only: tables, identity
& $T init    <path.accdb> "ASR Support Desk" production      # empty file only; verified backup first
& $T sync    "$env:APPDATA\gov.nj.doh.asrsupportdesk\support_desk.db" <path.accdb> "<mailbox name>"   # local backup, bind, import, sync until nothing pending
& $T verify  "$env:APPDATA\gov.nj.doh.asrsupportdesk\support_desk.db" <path.accdb>                     # count comparison, no writes
```

Run it from PowerShell (Git Bash rewrites UNC path arguments).

For the two-computer **pilot**, create a *test* database (for example `asr_email_support_TEST.accdb` in a test folder) and use designated test mailbox messages — never the production mailbox or database.

## 5. Identity binding, held changes and Standalone mode

- The first successful connection **binds** this computer's queue to that database identity. Every queued change records the identity it is meant for.
- Changes made **before** the first successful connection are **held for review**. Settings shows *N change(s) held for review* with **Send to <database>** or **Keep on this computer only**.
- If the path later points to a **different** database (another identity), nothing is sent or received. Settings shows *different database — review* and offers **Use this database instead…**; choosing it **isolates** the changes made for the old database (they are kept but never sent anywhere automatically).
- **Standalone** keeps everything on this computer. Switching back to Shared, or the database becoming reachable, does **not** upload Standalone work automatically: use **Import existing local cases**.

## 6. Importing existing local cases (migration)

Settings › Shared Database › **Import existing local cases**:

1. **Back up** the local database first (Settings › Database / Backup).
2. **Preview** (dry run): how many local cases are not yet shared, how many can be matched by Outlook Message-ID, how many cannot, and — when connected — how many **already exist** in the shared database.
3. **Import local cases** queues them in batches (restartable: re-running skips cases already queued). The shared database:
   - **joins** a case whose Outlook email (same Internet Message-ID in the same logical mailbox) is already there — notes, staff-entered values and owners from both computers are kept; different values for the same field become a **conflict** to review, not an overwrite;
   - keeps cases whose emails have **no Message-ID** separate, and records a *possible duplicate* relation for review when the sender, time and subject match — they are never merged automatically;
   - never merges cases just because the sender or school is the same.
4. Statuses migrate as they are (Unassigned / Assigned / In Progress / Done). Earlier ownership periods (from assignments) and Done decisions (as *legacy* completions with the emails they covered) are preserved. The old single *Staff notes* box became a note ("Earlier staff notes").

### Email identity

Each email gets a **matching key** = SHA-256 of `asr-email-v1`, the logical mailbox name (lower-case) and the Internet Message-ID (without `<>`), UTF-8, newline separated. It is a matching key, not encryption. Outlook EntryIDs/StoreIDs are profile-specific and stay on each computer (refreshed after moves). Emails without a Message-ID get a unique key plus *evidence* (sender, received second, subject) that only produces review suggestions.

## 7. Backups, recovery and rollback

**Two separate backups:**

| What | How | Restores |
|---|---|---|
| Shared database | Back up the share folder (IT backup), or copy the `.accdb` when nobody is connected (no `.laccdb` file next to it) | The team's shared records |
| Local database | Settings › Database / Backup › Back Up Database… (and automatic copies before every local upgrade in `backups\`) | One computer's working copy and its unsent queue |

The local database is a cache and recovery layer, **not** a substitute for backing up the shared database.

**Restoring a local backup** never replays old changes silently: after the restore, every change that was still waiting is **held for review**, and the next sync re-reads the shared state. Review the held list in Settings before sending anything.

**Restoring the shared database from a backup** (e.g. after damage):
1. Ask everyone to close the app (or switch to Standalone) so nothing writes during the restore.
2. Replace the `.accdb` with the backup copy (keep the damaged file aside).
3. The restored file keeps its **identity**, so computers continue syncing. Changes accepted after the backup was taken are lost from the shared database; computers that still have them only locally will **not** resend accepted changes automatically. Re-enter them or re-import affected cases deliberately.
4. If you instead create a **new** database, every computer sees *different database* and must choose **Use this database instead**, then **Import existing local cases**.

**Rollback of the app version**: install the previous version. A local database already upgraded to v4 is not readable by an older version: restore the automatic `backups\pre-migration-v3-<time>.db` copy (Settings › Restore From Backup) — work done after the upgrade is not in that copy.

## 8. Schema versions, maintenance and incompatible writers

- `DatabaseMetadata` holds `database_id`, `schema_version`, `min_client_protocol`, `maintenance`, `environment` and the creator.
- An app that finds a **newer** schema (or a higher minimum protocol) refuses to write (*needs a newer version of this app*). An app that finds an **older** schema refuses to write until the controlled upgrade runs.
- Upgrades are done by one person, deliberately: copy the `.accdb` first (`<name>.backup-<time>.accdb`), the upgrade sets **maintenance mode** (all apps stop writing and keep changes pending), applies the steps, then clears maintenance. A failed upgrade leaves maintenance on (writers stay blocked) and the data untouched; restore the copy or fix and re-run.
- Schema v1 is the only shared schema in this release; later phases add tables in new versions.

## 9. Synchronization rules (for reviewers)

- Every change has a unique **operation id** reused on every retry. The shared database records processed operation ids, so a retry after a lost acknowledgement returns the stored result and changes nothing twice.
- Each operation is applied in **one short Access transaction** together with its activity rows, version update, change-feed row and processed-operation result.
- Edits carry the values the user saw. Different fields changed by different people **merge**; the same field changed differently becomes a **conflict** keeping the original, your value and the shared value. Notes are separate records, so notes never overwrite each other. Workstation clocks are never used to pick a winner.
- Pulling reads the change feed **with an overlap** (AutoNumber order is not commit order) and runs a **full version comparison every 30 minutes**. Values with unsent local changes are not overwritten by a pull.
- Access text parameters: values over 255 characters are bound as long text (MEMO) and empty values with a declared length of 1, because the Access ODBC driver rejects other declarations (`HY104 Invalid precision value`). This was found and fixed with the real driver; SQLite-only tests would not have shown it.

## 9a. Diagnostics

Diagnostics (left menu) is the developer view: a bounded, redacted event log stored in the local database (separate from case history) plus the current connection state. **Copy Log** / **Copy Selected** produce plain text with a header (app version, local schema, shared database state/name/environment/schema, driver, last successful sync, queue counts, a short device id). Redaction runs before writing and again when copying.

Reproduced on a Windows 11 test machine with the real 64-bit Access driver (`cargo test --no-default-features -- --ignored diag_reproduce --nocapture`):

| Condition (test setup) | State shown | Code |
|---|---|---|
| UNC host that does not exist | unavailable | `SHARE_UNAVAILABLE` |
| Folder reachable, file missing | unavailable | `DB_FILE_NOT_FOUND` |
| NTFS *deny* ACL on the test folder for the current user | no permission | `DB_PERMISSION_DENIED` |
| Maintenance flag set | maintenance | `DB_MAINTENANCE` |
| Driver name not installed (real ODBC driver manager, SQLSTATE IM002) | driver missing | `ACCESS_DRIVER_MISSING` |
| `Exclusive=1` holder | locked | `DB_LOCKED` (native −1024) |
| Read-only file | read-only | `DB_READ_ONLY` (native −3035) |

## 10. Verified so far

| Check | How | Result |
|---|---|---|
| Create database, identity, write check, refuse existing file | Real 64-bit Access ODBC driver, new local test `.accdb` | Pass |
| Exclusive lock by another program → *locked* | Real driver (`Exclusive=1` holder) | Pass (native −1024) |
| Read-only file → readable but not writable | Real driver | Pass (native −3035) |
| Maintenance blocks writes; upgrade on current schema keeps data | Real driver | Pass |
| Two computers import the same email offline → one case, both notes kept, same shared id | Real driver, two simulated workstations in one process | Pass |
| Lost acknowledgement → one shared change | Real driver | Pass |
| Unicode text and 1,000-character notes | Real driver | Pass |
| Full P0.1 scenario suite (competing edits, identity change, restore, read-only, standalone import, missing Message-ID) | Same tests run against real `.accdb` files (`ASR_SHARED_BACKEND=access`) and SQLite | Pass |
| Initialize an existing empty `.accdb` in place (verified backup, exclusive open; refuses non-empty, already-initialized and open files) | Real driver, local test files (`access_initialize_existing_empty_file`) | Pass |
| Import keeps earlier ownership history as one record each (original dates, no local duplicates after the pull) | `import_keeps_earlier_ownership_history_as_one_record_each`, SQLite and real `.accdb` | Pass |
| Rehearsal on copies of the production file and the real local database: init, import of 477 cases, rerun, process killed after 292 of 480 operations then rerun, a second empty workstation pulling everything | Real driver, local copies | Pass — no duplicates, counts equal |
| **Production initialization and first import** on the UNC path (2026-11-18) | `shared_setup init/sync/verify` against the team default path (`asr_email_support.accdb`) | Pass — 20 tables; 477 cases, 574 emails, 203 ownership periods, 3 staff; rerun sends nothing |
| Two physical computers, real Outlook test messages, staff using the UI | **Not yet run** — this is the P0 pilot gate | Open |

Run the real-driver tests (never against the production path):

```powershell
$env:ASR_ACCESS_TEST_DIR = "$env:LOCALAPPDATA\asr-access-test"   # a local test folder
cd src-tauri
cargo test --no-default-features -- --ignored access_ --nocapture
$env:ASR_SHARED_BACKEND = "access"; cargo test --no-default-features shared_ -- --test-threads=1
```
