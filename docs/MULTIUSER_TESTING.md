# Multiuser Testing with Two Instances on One Computer

Development and pre-UAT testing of the shared database does not need a second computer: two independent
instances of the real desktop app run side by side, each with its own *application profile*, both connected
to the same shared Access database.

## 1. Application profiles

| Variable | Meaning |
|---|---|
| `ASR_APP_DATA_DIR` | Absolute folder for this instance: `support_desk.db` (local SQLite, its own device id, "I am" choice and sync queue), `settings.json`, `logs\`, `backups\`, `webview\` (WebView2 cache). The app **never falls back** to the normal profile: a folder that cannot be created stops the start-up with a message (also written to `%TEMP%\asr-support-desk-startup-error.txt`). Tauri's Known-Folder lookup is not used when it is set. |
| `ASR_TEST_MODE=1` | Development test mode: several instances may run at once (no single-instance hand-off), no autostart registration, the window title shows the profile, a "TEST MODE" banner appears in the sidebar, and Outlook is replaced by the mock below. |
| `ASR_OUTLOOK=mock\|live` | Default `mock` in test mode. **mock** = no Outlook COM call at all: nothing is moved, flagged or sent. `live` must be given explicitly and should only be used with designated test folders. |
| `ASR_OUTLOOK_MOCK_DIR` | Shared mock mailbox for both instances (default `<data dir>\outlook-mock`). |
| `ASR_PROFILE_NAME` | Label (window title, device name in the shared database, Diagnostics). |
| `ASR_TEST_STAFF` | Test mode: staff name to create (if missing) and select as "I am" (`<name>@example.invalid`, never a real address). |
| `ASR_DEVTOOLS_PORT` | Test mode: WebView2 remote-debugging port on 127.0.0.1 used by the automated UI driver. |

Without these variables the app behaves exactly as before (normal `%APPDATA%` profile, one instance, live Outlook).

## 2. Launch

From a normal (non-administrator) PowerShell in the repository:

```powershell
npx tauri build --debug --no-bundle            # or use an installed app with -Exe
scripts\launch_multiuser_test.ps1 -Reset       # fresh C:\ASR_Dev\WorkstationA / WorkstationB
scripts\launch_multiuser_test.ps1              # relaunch keeping the profiles
scripts\launch_multiuser_test.ps1 -Stop        # close both (-Only WorkstationA for one)
```

The script prepares `C:\ASR_Dev\WorkstationA` (staff *Test Staff A*, port 9301) and `WorkstationB`
(*Test Staff B*, port 9302), writes each `settings.json` (shared mode, the same database path, 1-minute
sync, the mock Inbox and Completed folder), sets the environment for each child process only, starts both
apps and records their process ids in `C:\ASR_Dev\instances.json`. `-Reset` only removes the local test
profile folders and the mock mailbox — never the shared database.

## 3. Mock Outlook mailbox

`C:\ASR_Dev\OutlookMock` stands for the team mailbox both "computers" see:

* `inbox.json` — emails in the mock folders; the normal **Check Now** scan imports them, moves change their folder.
* `faults.json` — inject failures, e.g. `{"move_messages": ["fail"], "send_notification": ["uncertain"]}` (each entry is used once).
* `journal.jsonl` — one line per move / flag / handoff / opened window: the evidence that a move or handoff happened exactly once.

## 4. Automated dual-instance tests

```powershell
node scripts\multiuser\run-tests.mjs                 # all scenarios (restarts the apps twice)
node scripts\multiuser\run-tests.mjs --only S5,S15   # selected scenarios
```

The runner connects to both windows through WebView2's DevTools protocol (no extra software), calls the
same commands the buttons call, and drives parts of the real UI (New Ticket dialog, Kanban drag, bulk bar,
every page, detail panels). Every case it creates has `DEVTEST<run>` in the subject. Results are written to
`C:\ASR_Dev\results\multiuser-<run>.md/.json`.

| # | Scenario |
|---|---|
| S0 | Distinct profiles/device ids, same database identity, both writable; colleague folders mapped |
| S1 | A creates a ticket in the UI → B receives it |
| S2 / S3 | A edits → B sees it; B adds a note → A sees it |
| S4 / S5 / S6 | Claim; simultaneous claims → one owner + visible conflict; Take over keeps history |
| S7 / S8 | Mark Done (archived through the mock, once) → B sees Done; a new reply → Review on both |
| S9 / S10 | School and issue types propagate; B combines two cases (preview first) → A sees one case, old reference still resolves |
| S11 / S12 | A offline keeps saving locally, UI responsive; reconnect delivers each change once |
| S13 / S14 | Separate fields merge; the same field → conflict with both values, explicit resolution |
| S15 / S15b | Same Outlook assignment from both → one reservation, one handoff; move failure retry, uncertain handoff check + confirmed resend |
| S16 / S17 | Kanban drag on A → B's list; bulk claim (UI) + start on B → A |
| T1 / T2 / T3 | Automatic background sync timing; concurrent writes/reads; process killed during sync |
| S18 | Both apps restarted → identical cases, owners, empty queues |
| UI | Every page renders on both windows; detail, related cases, combine dialog, timeline, Test Database Connection |

## 5. Limits of a one-computer test

Both instances run under the same Windows account and Outlook profile on one machine. The test proves the
shared-database coordination (reservations, ownership, conflicts, offline queue, Access locking from two
processes) but not separate Windows permissions on the share, separate Outlook profiles, or network latency
between buildings. Those are covered by staff UAT (`docs/UAT_CHECKLIST.md`).
