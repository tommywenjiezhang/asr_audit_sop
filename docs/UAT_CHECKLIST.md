# Staff UAT Checklist — ASR/Audit Support Desk 1.2.0

Development testing (automated suites plus two app instances on one computer, see `MULTIUSER_TESTING.md`)
is complete. UAT confirms what one computer cannot: real staff accounts, share permissions, separate Outlook
profiles and real (test) mailbox messages. Record results in the last column.

## Before starting

| Item | Notes | Done |
|---|---|---|
| 1.2.0 installed on each UAT computer (normal, non-admin account) | Installer in `Documents\ASR_Development_Build` | |
| 64-bit Access driver; Modify permission on the share **folder** | Settings › Shared Database › Test Connection: Yes / Yes / Yes | |
| Shared database schema v2 | Already upgraded on the development database. Another database: Settings › Shared Database › *Upgrade the shared database…* (everyone closed, verified backup first) | |
| Designated **test** messages/folders in Outlook | Never assign or archive real cases during UAT | |
| Staff records with email + Outlook folder; *I am* set on each computer | Settings › Staff Assignment | |
| Completed folder chosen | Settings › Completion Folder | |
| Local backup on each computer | Settings › Database / Backup | |

## Scenarios

| # | Scenario | Expected | Result |
|---|---|---|---|
| 1 | Same test email checked on two computers | One case on both, same ASR reference | |
| 2 | Claim on one computer | Other computer shows the owner; nothing moved in Outlook, no email sent | |
| 3 | Both claim the same case | One owner; the other sees a conflict naming the owner | |
| 4 | Take over | New owner on both; ownership history keeps the earlier owner | |
| 5 | **Assign** to a colleague | Emails move to the colleague's folder; ONE handoff email to the colleague (ticket, schools, issues, summary, previous/new owner, email location) | |
| 6 | Two people Assign the same case at once | Only one runs; the other gets "already running … on <computer>"; one handoff only | |
| 7 | Reassign | Emails move again; one new handoff; history shows both periods | |
| 8 | Mark Done (online) | Done at once; covered emails flagged complete and moved to Completed | |
| 9 | Mark Done while the share is unreachable | Done saved locally; after reconnecting the emails are archived automatically | |
| 10 | Reply to a Done case | Case moves to Review on both computers | |
| 11 | Related cases panel | Suggestions show reasons; Link keeps cases separate | |
| 12 | Combine cases (preview) | One case keeps all emails, notes, schools; old reference still opens it | |
| 13 | Move an email to another case | Email appears in the other case on both computers | |
| 14 | Kanban drag | Status changes through the normal checks; colleague sees it | |
| 15 | Bulk Claim / Start / Mark Done | Per-case results; Retry failed only reruns failures | |
| 16 | Same field edited on both computers | Conflicts page shows both values; Keep shared / Use my value resolves it | |
| 17 | Read-only share permission | Readable, not writable; own changes stay pending | |
| 18 | Diagnostics | Test Database / Outlook Connection, Sync Now, Check Shared Integrity, Export Log (no personal data in the file) | |
| 19 | Team Overview | Workload, older/stale cases, pending Outlook work, conflicts, staff activity | |
| 20 | Restart both apps with pending changes | Changes sent once after restart | |

Use Diagnostics › Copy Log / Export Log as evidence for any failure.
