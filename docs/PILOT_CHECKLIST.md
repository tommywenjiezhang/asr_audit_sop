# P0 Two-Computer Pilot — Checklist and Result Record

**Gate status: OPEN.** The P0 code (shared database, team workflow, Diagnostics) is complete and tested on one Windows machine with the real Access driver. The two-computer pilot below has **not** been run. Do not start P1 (Outlook Assign/Reassign with shared reservations), P2 or P3 until every row is recorded as passed.

## What you need

| Prerequisite | Notes |
|---|---|
| Two Windows workstations, each with the 1.1.0 build and classic Outlook | Normal (non-administrator) sessions |
| 64-bit Microsoft Access Database Engine on both | Settings › Shared Database › Test Connection shows *driver Available64* |
| A **test** network share folder with Modify permission for both users | Not the production share folder, not OneDrive |
| A **test** shared database | Settings › Shared Database › *Create a new shared database* → environment **test**. Note: the team default path is now the **production** shared database (initialized 2026-11-18); the pilot must use a separate test file |
| A designated **test mailbox** (or test folder) with test messages, never production staff mailboxes | Both computers monitor the same test folder |
| Two staff records (e.g. *Aaron (test)*, *Wenjie (test)*), each computer's **I am** set | Settings › Staff Assignment |
| A local backup on each computer before starting | Settings › Database / Backup |

## Setup (both computers)

1. Install 1.1.0. On first start the local database upgrades to v4 (a backup is written to `backups\pre-migration-v3-*.db`).
2. Settings › Shared Database: mode **Shared**, path = the test `.accdb`, **Save**, **Test Connection** → *Reachable / readable / write permission: Yes / Yes / Yes*, environment *test*.
3. If *N change(s) held for review* appears, choose **Keep on this computer only** for the pilot (nothing from earlier local work is sent to the test database), or **Import existing local cases** only if you want to test the migration with a copy.
4. Settings › Staff Assignment: add yourself (name, email, optional Windows account), set **I am**.
5. Point Monitored Folders at the test folder; **Check Now**.

## Scenarios — record each result

Use the copied Diagnostics log (Diagnostics › Copy Log) as evidence for anything that fails.

| # | Scenario | Steps | Expected | Result (pass/fail, date, initials, notes) |
|---|---|---|---|---|
| 1 | Duplicate import | Send one test email. Check Now on **both** computers before either syncs (or with the share disconnected), then Sync Now on both | One case in Team Tickets on both computers, same ASR reference; no duplicate; any notes from both kept | |
| 2 | Simultaneous claim | Both open the same Unassigned case, both click **Claim** within seconds, both Sync Now | One confirmed owner on both; the other computer shows a Conflict panel naming the owner; nothing moved in Outlook | |
| 3 | Takeover history | Aaron claims, Start Work, adds a note; Wenjie **Take over…** | Wenjie owns it, status stays In Progress; Ownership table shows Aaron's period ended and Wenjie's started; Aaron's note remains | |
| 4 | Claim moves nothing | After 2/3 check the test mailbox | Emails are still in the same folder; no handoff email was sent | |
| 5 | Offline restart | Disconnect the share (or rename the test folder), add a note and Mark Done, **close and restart** the app | Changes still shown as *Saved locally — pending sync* after restart | |
| 6 | Reconnect and replay | Restore the share access; Sync Now | Pending changes accepted once; Settings shows 0 pending; colleague sees them | |
| 7 | Lost acknowledgement | During Sync Now, disconnect the network right after the progress shows sending (best effort), then reconnect and Sync Now | No duplicate notes/activity; Diagnostics may show a replayed result | |
| 8 | Reply during completion | Aaron (offline) Mark Done; meanwhile send a reply to the test email and let Wenjie Check Now and sync; then Aaron reconnects | Case ends in **Review — New reply from …** on both; Aaron sees a conflict *"New reply … arrived after this Done decision"*; the reply is visible on Aaron's computer | |
| 9 | Reply after completion | Mark Done and sync; then send a reply; the other computer checks and syncs | Case moves to **Review**, owner kept; history shows the reopen reason | |
| 10 | Older email found later | After Done, move an *older* message of the same conversation into the folder and Check Now | Case stays Done; history says an earlier email was found | |
| 11 | Folder-scope change | Change Monitored Folders to another test folder and save | Current Tickets no longer shows the previous folder's cases; they remain in Team Tickets / History | |
| 12 | Copied Diagnostics | Cause a failure (wrong path, then rename back); Diagnostics › Go to latest error › Copy Log, paste into Notepad | Plain text with codes and plain-language messages; no email addresses, phone numbers, NJIIS IDs, subjects or user names | |
| 13 | Database identity change | Point one computer at a *second* test database (path change), Test Connection | *different database — review*; nothing sent; **Use this database instead** isolates old pending changes | |
| 14 | Access unavailable | Enter a path to a non-existent share; work for 10 minutes | Everything works locally; sidebar shows *offline — saved locally*; retries back off (Diagnostics) | |
| 15 | Read-only access | Remove Modify (keep Read) for one user on the test folder | Test Connection: readable Yes, write permission No; team cases still refresh; own changes stay pending | |
| 16 | Archive in Outlook | Mark Done, sync, then **Archive in Outlook** | Only the covered incoming emails are flagged and moved to the completion folder; the case stays Done if Outlook fails | |

## Already verified without the pilot (for reference)

| Area | Evidence | Real Access driver? |
|---|---|---|
| Schema creation, identity, write check, refuse existing file | `shared_store::tests::access_create_probe_and_reject_existing` | Yes (local test `.accdb`) |
| Exclusive lock → locked; read-only file → read-only; maintenance blocks writes; upgrade keeps data | `access_locks_read_only_and_maintenance` | Yes |
| Missing driver (IM002) | `missing_driver_is_reported_as_driver_missing` | Yes (real ODBC driver manager) |
| Unreachable share, missing file, NTFS-denied folder, maintenance → distinct codes in the copied log | `diag_reproduce_failures_with_the_real_driver` | Yes (local folders + non-existent UNC host) |
| Two workstations: duplicate import, competing edits, lost acknowledgement, identity change, restore without replay, read-only pull, standalone import, missing Message-ID, claims, takeover, simultaneous claims, Mark Done offline, reply after/during completion, older email/auto-reply, review/resume, Add to Existing Case | 33 scenario tests run with `ASR_SHARED_BACKEND=access` (each test creates a new `.accdb`) and with SQLite | Yes — **two simulated workstations in one process on one machine**, local disk (not a network share) |
| Outlook moves/flags/sends | Python worker tests with **fake COM objects** only | No (mocked) |

What is **not** covered by any of the above, and is the point of the pilot: two physical computers, a real SMB network share (latency, locking across machines, `.laccdb` behaviour), real Outlook profiles and test messages, and staff using the UI.
