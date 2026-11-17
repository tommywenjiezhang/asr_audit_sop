//! Mark Done: flag the ticket's incoming Outlook emails complete, move them to the configured
//! completion folder, then mark the ticket Done.
//!
//! Outlook and SQLite cannot share a transaction, so progress is stored per message in
//! `message_completion`. A retry only does what is still missing (no second move of a message that
//! was already moved), and the ticket becomes Done only when every incoming email is flagged and moved.
//! Sent replies, drafts and staff emails are never touched. Nothing is ever deleted.

use crate::db::now;
use crate::error::AppResult;
use crate::settings::FolderRef;
use crate::tickets::{self, history};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// One incoming email that still needs Outlook work.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PendingItem {
    pub message_id: i64,
    pub entry_id: String,
    pub store_id: String,
    pub internet_message_id: String,
    pub folder_entry_id: String,
    pub folder_path: String,
    pub subject: String,
    pub need_flag: bool,
    pub need_move: bool,
}

/// Incoming emails of the ticket (original + replies from outside, with an Outlook EntryID)
/// that are not yet both flagged complete and moved.
pub fn pending(conn: &Connection, ticket_id: i64) -> AppResult<Vec<PendingItem>> {
    let mut st = conn.prepare(
        "SELECT m.id, COALESCE(m.entry_id,''), COALESCE(m.store_id,''), COALESCE(m.internet_message_id,''),
                COALESCE(m.folder_entry_id,''), COALESCE(m.folder_path,''), COALESCE(m.subject,''),
                c.flagged_at IS NULL, c.moved_at IS NULL
         FROM ticket_email_links l
         JOIN email_messages m ON m.id = l.message_id
         LEFT JOIN message_completion c ON c.message_id = m.id
         WHERE l.ticket_id = ?1 AND l.link_type IN ('original','reply') AND m.is_internal = 0 AND COALESCE(m.entry_id,'') <> ''
         ORDER BY m.received_at, m.id",
    )?;
    let v = st
        .query_map([ticket_id], |r| {
            Ok(PendingItem {
                message_id: r.get(0)?,
                entry_id: r.get(1)?,
                store_id: r.get(2)?,
                internet_message_id: r.get(3)?,
                folder_entry_id: r.get(4)?,
                folder_path: r.get(5)?,
                subject: r.get(6)?,
                need_flag: r.get(7)?,
                need_move: r.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v.into_iter().filter(|p| p.need_flag || p.need_move).collect())
}

/// Record that an attempt is starting (before calling Outlook).
pub fn begin_attempt(conn: &Connection, ticket_id: i64, items: &[PendingItem]) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    for i in items {
        tx.execute(
            "INSERT INTO message_completion(message_id, ticket_id, attempts) VALUES(?1, ?2, 1)
             ON CONFLICT(message_id) DO UPDATE SET attempts = attempts + 1, ticket_id = excluded.ticket_id",
            params![i.message_id, ticket_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Per-message result from the worker's `complete_messages` operation.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ItemResult {
    pub message_id: i64,
    pub flagged: bool,
    pub moved: bool,
    pub new_entry_id: Option<String>,
    pub new_store_id: Option<String>,
    pub new_folder_entry_id: Option<String>,
    pub new_folder_path: Option<String>,
    pub error_code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedItem {
    pub message_id: i64,
    pub subject: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    /// true when the ticket is now Done
    pub done: bool,
    pub flagged: usize,
    pub moved: usize,
    pub failed: Vec<FailedItem>,
    pub dest_path: String,
}

/// Store the worker results; mark the ticket Done only when nothing is left to do.
pub fn apply_results(conn: &Connection, ticket_id: i64, dest: &FolderRef, requested: &[PendingItem], results: &[ItemResult], actor: &str) -> AppResult<Outcome> {
    let tx = conn.unchecked_transaction()?;
    let ts = now();
    let mut flagged = 0;
    let mut moved = 0;
    let mut failed: Vec<FailedItem> = Vec::new();
    for item in requested {
        // the progress row normally exists from begin_attempt; never lose a result if it does not
        tx.execute(
            "INSERT OR IGNORE INTO message_completion(message_id, ticket_id, attempts) VALUES(?1, ?2, 1)",
            params![item.message_id, ticket_id],
        )?;
        let r = results.iter().find(|r| r.message_id == item.message_id);
        let Some(r) = r else {
            failed.push(FailedItem { message_id: item.message_id, subject: item.subject.clone(), error: "Outlook returned no result for this email.".into() });
            tx.execute("UPDATE message_completion SET last_error=?2, last_error_at=?3 WHERE message_id=?1", params![item.message_id, "No result from Outlook", ts])?;
            continue;
        };
        if r.flagged {
            tx.execute("UPDATE message_completion SET flagged_at=COALESCE(flagged_at, ?2) WHERE message_id=?1", params![item.message_id, ts])?;
            if item.need_flag {
                flagged += 1;
            }
        }
        if r.moved {
            tx.execute(
                "UPDATE message_completion SET moved_at=COALESCE(moved_at, ?2), dest_store_id=?3, dest_folder_entry_id=?4, dest_path=?5 WHERE message_id=?1",
                params![item.message_id, ts, dest.store_id, dest.entry_id, r.new_folder_path.clone().unwrap_or_else(|| dest.path.clone())],
            )?;
            if item.need_move {
                moved += 1;
            }
        }
        // the item's current Outlook location (EntryIDs change when an item moves between folders)
        if let Some(new_entry) = r.new_entry_id.as_deref().filter(|e| !e.is_empty()) {
            if new_entry != item.entry_id {
                tx.execute(
                    "UPDATE email_messages SET entry_id=?2, store_id=COALESCE(NULLIF(?3,''), store_id), folder_entry_id=COALESCE(NULLIF(?4,''), folder_entry_id), folder_path=COALESCE(NULLIF(?5,''), folder_path) WHERE id=?1",
                    params![item.message_id, new_entry, r.new_store_id.clone().unwrap_or_default(), r.new_folder_entry_id.clone().unwrap_or_default(), r.new_folder_path.clone().unwrap_or_default()],
                )?;
            }
        }
        if r.moved {
            // the completion folder is never part of the scope: confirmed out, reason "completed"
            tx.execute(
                "UPDATE email_messages SET scope_state='out', out_reason='completed', location_status='found', location_checked_at=?2,
                   folder_entry_id=COALESCE(NULLIF(?3,''), folder_entry_id), folder_path=COALESCE(NULLIF(?4,''), folder_path) WHERE id=?1",
                params![item.message_id, ts, r.new_folder_entry_id.clone().unwrap_or_else(|| dest.entry_id.clone()), r.new_folder_path.clone().unwrap_or_else(|| dest.path.clone())],
            )?;
        }
        let still_missing = (item.need_flag && !r.flagged) || (item.need_move && !r.moved);
        if still_missing {
            let what = match (item.need_flag && !r.flagged, item.need_move && !r.moved) {
                (true, true) => "not flagged and not moved",
                (true, false) => "not flagged complete",
                _ => "flagged complete but not moved",
            };
            let err = format!("{} ({what})", r.message.clone().unwrap_or_else(|| "Outlook could not complete this email".into()));
            tx.execute("UPDATE message_completion SET last_error=?2, last_error_at=?3 WHERE message_id=?1", params![item.message_id, err, ts])?;
            failed.push(FailedItem { message_id: item.message_id, subject: item.subject.clone(), error: err });
        } else {
            tx.execute("UPDATE message_completion SET last_error=NULL, last_error_at=NULL WHERE message_id=?1", [item.message_id])?;
        }
    }
    let remaining = pending(&tx, ticket_id)?;
    let done = remaining.is_empty();
    if done {
        tickets::set_done(&tx, ticket_id, actor, &format!("Marked Done: {} email(s) flagged complete and moved to {}", requested.len(), dest.path))?;
    } else {
        history(&tx, ticket_id, actor, "Mark Done incomplete - retry needed", None, None,
            Some(&format!("{} of {} email(s) still need Outlook steps", remaining.len(), requested.len())))?;
        tx.execute("UPDATE tickets SET updated_at=?2 WHERE id=?1", params![ticket_id, ts])?;
    }
    crate::scope::recompute_ticket(&tx, ticket_id, actor)?;
    crate::search::reindex(&tx, ticket_id)?;
    tx.commit()?;
    Ok(Outcome { done, flagged, moved, failed, dest_path: dest.path.clone() })
}

/// Outlook could not be used at all (e.g. not running): keep the error on each pending email.
pub fn record_failure(conn: &Connection, ticket_id: i64, items: &[PendingItem], error: &str, actor: &str) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    let ts = now();
    for i in items {
        tx.execute("UPDATE message_completion SET last_error=?2, last_error_at=?3 WHERE message_id=?1", params![i.message_id, error, ts])?;
    }
    history(&tx, ticket_id, actor, "Mark Done failed - retry needed", None, None, Some(error))?;
    tx.commit()?;
    Ok(())
}

/// Ticket without Outlook emails to archive (e.g. created manually, or all already archived).
pub fn finish_without_outlook(conn: &Connection, ticket_id: i64, actor: &str) -> AppResult<Outcome> {
    let tx = conn.unchecked_transaction()?;
    tickets::set_done(&tx, ticket_id, actor, "Marked Done (no Outlook emails left to archive)")?;
    crate::scope::recompute_ticket(&tx, ticket_id, actor)?;
    crate::search::reindex(&tx, ticket_id)?;
    tx.commit()?;
    Ok(Outcome { done: true, flagged: 0, moved: 0, failed: vec![], dest_path: String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::tickets::tests::{conn, msg};
    use crate::tickets::{get, ingest, NewTicket, DONE, UNASSIGNED};

    fn dest() -> FolderRef {
        FolderRef { store_id: "S1".into(), entry_id: "DONE".into(), path: "Inbox/Completed".into() }
    }
    fn ok(id: i64, entry: &str) -> ItemResult {
        ItemResult { message_id: id, flagged: true, moved: true, new_entry_id: Some(entry.into()), new_store_id: Some("S1".into()),
            new_folder_entry_id: Some("DONE".into()), new_folder_path: Some("Inbox/Completed".into()), ..Default::default() }
    }

    #[test]
    fn success_marks_done_and_records_new_location() {
        let c = conn();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org")], &Settings::default()).unwrap();
        let p = pending(&c, 1).unwrap();
        assert_eq!(p.len(), 1);
        assert!(p[0].need_flag && p[0].need_move);
        begin_attempt(&c, 1, &p).unwrap();
        let o = apply_results(&c, 1, &dest(), &p, &[ok(p[0].message_id, "E1-NEW")], "staff").unwrap();
        assert!(o.done);
        assert_eq!((o.flagged, o.moved), (1, 1));
        let t = get(&c, 1).unwrap();
        assert_eq!(t.status, DONE);
        assert!(t.completed_at.is_some());
        let (entry, path): (String, String) = c.query_row("SELECT entry_id, folder_path FROM email_messages WHERE id=?1", [p[0].message_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((entry.as_str(), path.as_str()), ("E1-NEW", "Inbox/Completed"), "EntryID change after the move is stored");
        assert!(pending(&c, 1).unwrap().is_empty());
        // a later scan sees the archived copy under its new EntryID: no new ticket, no reopen
        let mut archived = msg(1, "C1", "Help", "user guide please", "a@b.org");
        archived.entry_id = "E1-NEW".into();
        archived.folder_path = "Inbox/Completed".into();
        let r = ingest(&c, &[archived], &Settings::default()).unwrap();
        assert_eq!(r.new_messages, 0);
        assert_eq!(get(&c, 1).unwrap().status, DONE);
    }

    #[test]
    fn flagged_but_not_moved_keeps_in_progress_and_retry_only_moves() {
        let c = conn();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org")], &Settings::default()).unwrap();
        let p = pending(&c, 1).unwrap();
        begin_attempt(&c, 1, &p).unwrap();
        let partial = ItemResult { message_id: p[0].message_id, flagged: true, moved: false, error_code: Some("MOVE_FAILED".into()), message: Some("Move failed: no permission".into()), ..Default::default() };
        let o = apply_results(&c, 1, &dest(), &p, &[partial], "staff").unwrap();
        assert!(!o.done);
        assert_eq!(o.failed.len(), 1);
        assert!(o.failed[0].error.contains("flagged complete but not moved"));
        assert_eq!(get(&c, 1).unwrap().status, UNASSIGNED, "no extra status: stays open (Unassigned)");
        let rows = crate::tickets::completion_rows(&c, 1).unwrap();
        assert!(rows[0].flagged_at.is_some() && rows[0].moved_at.is_none() && rows[0].last_error.is_some());
        // retry: only the move is still needed
        let p2 = pending(&c, 1).unwrap();
        assert_eq!(p2.len(), 1);
        assert!(!p2[0].need_flag && p2[0].need_move);
        begin_attempt(&c, 1, &p2).unwrap();
        let o = apply_results(&c, 1, &dest(), &p2, &[ok(p2[0].message_id, "E1-NEW")], "staff").unwrap();
        assert!(o.done);
        assert_eq!((o.flagged, o.moved), (0, 1));
        let rows = crate::tickets::completion_rows(&c, 1).unwrap();
        assert_eq!(rows[0].attempts, 2);
        assert!(rows[0].last_error.is_none());
    }

    #[test]
    fn partial_batch_retries_only_unfinished_and_skips_staff_mail() {
        let c = conn();
        let mut s = Settings::default();
        s.internal_senders = vec!["@doh.nj.gov".into()];
        ingest(&c, &[
            msg(1, "C1", "Help", "user guide please", "a@b.org"),
            msg(2, "C1", "RE: Help", "more info", "a@b.org"),
            msg(3, "C1", "RE: Help", "staff reply", "me@doh.nj.gov"),
            msg(4, "C1", "RE: Help", "and another", "a@b.org"),
        ], &s).unwrap();
        let p = pending(&c, 1).unwrap();
        assert_eq!(p.len(), 3, "the staff email is never flagged or moved");
        begin_attempt(&c, 1, &p).unwrap();
        let res = vec![ok(p[0].message_id, "N1"), ok(p[1].message_id, "N2"),
            ItemResult { message_id: p[2].message_id, error_code: Some("ITEM_NOT_FOUND".into()), message: Some("Email not found".into()), ..Default::default() }];
        let o = apply_results(&c, 1, &dest(), &p, &res, "staff").unwrap();
        assert!(!o.done);
        let p2 = pending(&c, 1).unwrap();
        assert_eq!(p2.iter().map(|x| x.message_id).collect::<Vec<_>>(), vec![p[2].message_id]);
        let o = apply_results(&c, 1, &dest(), &p2, &[ok(p2[0].message_id, "N4")], "staff").unwrap();
        assert!(o.done);
    }

    #[test]
    fn reopened_ticket_completes_only_the_new_reply() {
        let c = conn();
        let s = Settings::default();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org")], &s).unwrap();
        let p = pending(&c, 1).unwrap();
        apply_results(&c, 1, &dest(), &p, &[ok(p[0].message_id, "N1")], "staff").unwrap();
        ingest(&c, &[msg(2, "C1", "RE: Help", "one more question", "a@b.org")], &s).unwrap();
        assert_eq!(get(&c, 1).unwrap().status, UNASSIGNED);
        let p2 = pending(&c, 1).unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].entry_id, "E2");
    }

    #[test]
    fn worker_failure_and_ticket_without_outlook_mail() {
        let c = conn();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org")], &Settings::default()).unwrap();
        let p = pending(&c, 1).unwrap();
        begin_attempt(&c, 1, &p).unwrap();
        record_failure(&c, 1, &p, "Outlook is not running", "staff").unwrap();
        assert_eq!(get(&c, 1).unwrap().status, UNASSIGNED);
        assert_eq!(crate::tickets::completion_rows(&c, 1).unwrap()[0].last_error.as_deref(), Some("Outlook is not running"));
        // a manual ticket has no Outlook emails: Done locally
        let id = crate::tickets::create_manual(&c, &NewTicket { subject: "Phone call".into(), body: "caller asked for the guide".into(), ..Default::default() }, &Settings::default(), "staff").unwrap();
        assert!(pending(&c, id).unwrap().is_empty());
        let o = finish_without_outlook(&c, id, "staff").unwrap();
        assert!(o.done);
        assert_eq!(get(&c, id).unwrap().status, DONE);
    }
}
