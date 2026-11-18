//! Assign / reassign tickets to staff and send the handoff notification.
//!
//! An assignment moves the ticket's incoming source emails into the staff member's Outlook folder
//! (never drafts, sent items or staff emails; nothing is marked read or flagged). Progress is stored per
//! email, so a retry only repeats moves that did not succeed. The assignment is finalized only when every
//! required move succeeded; only then is the notification allowed.
//!
//! Notification states:  none -> pending -> sending -> submitted | failed | uncertain   (or skipped)
//!   * a claim (pending/failed -> sending) is atomic, so repeated clicks cannot send twice
//!   * "submitted" means Outlook accepted Send(); delivery is NOT confirmed
//!   * "uncertain" (Send() may or may not have happened) is never resent automatically: staff check the
//!     Sent Items (find_notification) or explicitly confirm a resend
//!   * reassigning to the same person (no ownership change) -> "skipped"

use crate::analyzer::split_current;
use crate::catalog::catalog;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::render::html_escape;
use crate::scope::{self, ScopeCfg};
use crate::staff::{self, Staff};
use crate::tickets::{self, history};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const NOT_PROVIDED: &str = "Not provided";
pub const NOTIFY_MARKER: &str = "ASR-Desk-Notification";

#[derive(Debug, Clone, Serialize)]
pub struct MoveRow {
    pub message_id: i64,
    pub subject: Option<String>,
    pub moved_at: Option<String>,
    pub new_folder_path: Option<String>,
    pub last_error: Option<String>,
    pub attempts: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssignmentOp {
    pub id: i64,
    pub ticket_id: i64,
    pub from_staff_id: Option<i64>,
    pub from_name: Option<String>,
    pub to_staff_id: i64,
    pub to_name: Option<String>,
    pub operator: String,
    pub note: Option<String>,
    pub created_at: String,
    pub state: String,
    pub dest_store_id: String,
    pub dest_folder_entry_id: String,
    pub dest_path: String,
    pub prev_folders: Option<String>,
    pub finished_at: Option<String>,
    pub notify_state: String,
    pub notify_id: Option<String>,
    pub notify_recipient: Option<String>,
    pub notify_account: Option<String>,
    pub notify_attempts: i64,
    pub notify_at: Option<String>,
    pub notify_error: Option<String>,
    pub notify_entry_id: Option<String>,
    pub notify_checked_at: Option<String>,
    pub moves: Vec<MoveRow>,
}

const OP_COLS: &str = "o.id,o.ticket_id,o.from_staff_id,(SELECT display_name FROM staff s WHERE s.id=o.from_staff_id),o.to_staff_id,(SELECT display_name FROM staff s WHERE s.id=o.to_staff_id),
    o.operator,o.note,o.created_at,o.state,o.dest_path,o.prev_folders,o.finished_at,o.notify_state,o.notify_id,o.notify_recipient,o.notify_account,o.notify_attempts,
    o.notify_at,o.notify_error,o.notify_entry_id,o.notify_checked_at,o.dest_store_id,o.dest_folder_entry_id";

fn map_op(r: &rusqlite::Row) -> rusqlite::Result<AssignmentOp> {
    Ok(AssignmentOp {
        id: r.get(0)?,
        ticket_id: r.get(1)?,
        from_staff_id: r.get(2)?,
        from_name: r.get(3)?,
        to_staff_id: r.get(4)?,
        to_name: r.get(5)?,
        operator: r.get(6)?,
        note: r.get(7)?,
        created_at: r.get(8)?,
        state: r.get(9)?,
        dest_path: r.get(10)?,
        prev_folders: r.get(11)?,
        finished_at: r.get(12)?,
        notify_state: r.get(13)?,
        notify_id: r.get(14)?,
        notify_recipient: r.get(15)?,
        notify_account: r.get(16)?,
        notify_attempts: r.get(17)?,
        notify_at: r.get(18)?,
        notify_error: r.get(19)?,
        notify_entry_id: r.get(20)?,
        notify_checked_at: r.get(21)?,
        dest_store_id: r.get(22)?,
        dest_folder_entry_id: r.get(23)?,
        moves: vec![],
    })
}

fn moves(conn: &Connection, op_id: i64) -> AppResult<Vec<MoveRow>> {
    let mut st = conn.prepare(
        "SELECT a.message_id, m.subject, a.moved_at, a.new_folder_path, a.last_error, a.attempts
         FROM assignment_moves a JOIN email_messages m ON m.id=a.message_id WHERE a.op_id=?1 ORDER BY m.received_at, m.id",
    )?;
    let v = st
        .query_map([op_id], |r| Ok(MoveRow { message_id: r.get(0)?, subject: r.get(1)?, moved_at: r.get(2)?, new_folder_path: r.get(3)?, last_error: r.get(4)?, attempts: r.get(5)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn get_op(conn: &Connection, op_id: i64) -> AppResult<AssignmentOp> {
    let mut op = conn
        .query_row(&format!("SELECT {OP_COLS} FROM assignment_ops o WHERE o.id=?1"), [op_id], map_op)
        .optional()?
        .ok_or_else(|| AppError::coded("NOT_FOUND", "Assignment not found."))?;
    op.moves = moves(conn, op_id)?;
    Ok(op)
}

/// All assignment operations of a ticket, newest first.
pub fn ops(conn: &Connection, ticket_id: i64) -> AppResult<Vec<AssignmentOp>> {
    let ids: Vec<i64> = {
        let mut st = conn.prepare("SELECT id FROM assignment_ops WHERE ticket_id=?1 ORDER BY id DESC")?;
        let v = st.query_map([ticket_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    ids.into_iter().map(|id| get_op(conn, id)).collect()
}

/// Incoming source emails of a ticket (never drafts, sent items, staff emails or notifications).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SourceMsg {
    pub message_id: i64,
    pub entry_id: String,
    pub store_id: String,
    pub imid: String,
    pub folder_entry_id: String,
    pub folder_path: String,
    pub subject: String,
}

pub fn source_messages(conn: &Connection, ticket_id: i64) -> AppResult<Vec<SourceMsg>> {
    let mut st = conn.prepare(
        "SELECT m.id, COALESCE(m.entry_id,''), COALESCE(m.store_id,''), COALESCE(m.internet_message_id,''), COALESCE(m.folder_entry_id,''), COALESCE(m.folder_path,''), COALESCE(m.subject,'')
         FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
         WHERE l.ticket_id=?1 AND l.link_type IN ('original','reply') AND m.is_internal=0
           AND (COALESCE(m.entry_id,'')<>'' OR COALESCE(m.internet_message_id,'')<>'')
         ORDER BY m.received_at, m.id",
    )?;
    let v = st
        .query_map([ticket_id], |r| Ok(SourceMsg { message_id: r.get(0)?, entry_id: r.get(1)?, store_id: r.get(2)?, imid: r.get(3)?, folder_entry_id: r.get(4)?, folder_path: r.get(5)?, subject: r.get(6)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

// ------------------------------------------------------------------ preview / start

#[derive(Debug, Clone, Serialize)]
pub struct AssignPreview {
    pub ticket_id: i64,
    pub display_id: String,
    pub from_name: Option<String>,
    pub to: Staff,
    pub emails: usize,
    pub already_in_folder: usize,
    pub will_notify: bool,
    /// Why the assignment cannot run (e.g. ticket is Done, staff inactive); None = OK
    pub blocked: Option<String>,
}

pub fn preview(conn: &Connection, ticket_id: i64, staff_id: i64) -> AppResult<AssignPreview> {
    let t = tickets::get(conn, ticket_id)?;
    let to = staff::get(conn, staff_id)?;
    let src = source_messages(conn, ticket_id)?;
    let already = src.iter().filter(|m| m.folder_entry_id == to.folder_entry_id && m.store_id == to.store_id).count();
    let blocked = if t.status == tickets::DONE {
        Some("This ticket is Done. Reopen it before assigning it.".to_string())
    } else {
        staff::assignable(conn, staff_id).err().map(|e| e.to_string())
    };
    Ok(AssignPreview {
        ticket_id,
        display_id: t.display_id,
        from_name: t.assignee_name.clone(),
        will_notify: t.assignee_staff_id != Some(staff_id),
        to,
        emails: src.len(),
        already_in_folder: already,
        blocked,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Begin {
    /// operation to run (new or resumed)
    Op(i64),
    /// already assigned to this person with every email in their folder: nothing to do, no notification
    NoChange,
}

pub fn begin(conn: &Connection, ticket_id: i64, staff_id: i64, operator: &str, note: &str) -> AppResult<Begin> {
    let t = tickets::get(conn, ticket_id)?;
    if t.status == tickets::DONE {
        return Err(AppError::coded("TICKET_DONE", format!("{} is Done. Reopen it before assigning it.", t.display_id)));
    }
    let to = staff::assignable(conn, staff_id)?;
    if note.len() > 10_000 {
        return Err(AppError::coded("INVALID", "The handoff note is too long."));
    }
    let src = source_messages(conn, ticket_id)?;
    let tx = conn.unchecked_transaction()?;
    // an unfinished operation to the same person is resumed; one to someone else is superseded
    let open: Option<(i64, i64)> = tx
        .query_row("SELECT id, to_staff_id FROM assignment_ops WHERE ticket_id=?1 AND state IN ('moving','failed') ORDER BY id DESC LIMIT 1", [ticket_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    if let Some((op, to_id)) = open {
        if to_id == staff_id {
            if !note.trim().is_empty() {
                tx.execute("UPDATE assignment_ops SET note=?2 WHERE id=?1", params![op, note.trim()])?;
            }
            tx.commit()?;
            return Ok(Begin::Op(op));
        }
        tx.execute("UPDATE assignment_ops SET state='superseded', finished_at=?2 WHERE id=?1", params![op, now()])?;
        history(&tx, ticket_id, operator, "Unfinished assignment replaced by a new assignment", None, None, None)?;
    }
    let in_dest = |m: &SourceMsg| m.folder_entry_id == to.folder_entry_id && m.store_id == to.store_id;
    if t.assignee_staff_id == Some(staff_id) && src.iter().all(in_dest) {
        tx.commit()?;
        return Ok(Begin::NoChange);
    }
    let mut prev: Vec<String> = src.iter().map(|m| m.folder_path.clone()).filter(|p| !p.is_empty()).collect();
    prev.sort();
    prev.dedup();
    let ts = now();
    tx.execute(
        "INSERT INTO assignment_ops(ticket_id,from_staff_id,to_staff_id,operator,note,created_at,state,dest_store_id,dest_folder_entry_id,dest_path,prev_folders,notify_state)
         VALUES(?1,?2,?3,?4,NULLIF(?5,''),?6,'moving',?7,?8,?9,?10,'none')",
        params![ticket_id, t.assignee_staff_id, staff_id, operator, note.trim(), ts, to.store_id, to.folder_entry_id, dest_label(&to), prev.join("; ")],
    )?;
    let op = tx.last_insert_rowid();
    let reference = tickets::ensure_ticket_ref(&tx, ticket_id)?;
    tx.execute("UPDATE assignment_ops SET notify_id=?2 WHERE id=?1", params![op, format!("{}-{}-{}", reference, op, ts.replace([':', '-'], ""))])?;
    for m in &src {
        let done = if in_dest(m) { Some(ts.clone()) } else { None };
        tx.execute(
            "INSERT INTO assignment_moves(op_id,message_id,moved_at,new_folder_path) VALUES(?1,?2,?3,?4)",
            params![op, m.message_id, done, if in_dest(m) { Some(m.folder_path.clone()) } else { None }],
        )?;
    }
    let what = if t.assignee_staff_id.is_some() && t.assignee_staff_id != Some(staff_id) { "Reassignment started" } else { "Assignment started" };
    history(&tx, ticket_id, operator, what, Some("assignee"), t.assignee_name.as_deref(), Some(&to.display_name))?;
    tx.commit()?;
    Ok(Begin::Op(op))
}

fn dest_label(s: &Staff) -> String {
    match s.mailbox_name.as_deref().filter(|m| !m.is_empty()) {
        Some(mb) => format!("{mb} › {}", s.folder_path),
        None => s.folder_path.clone(),
    }
}

/// Emails of the operation that still have to be moved.
pub fn pending_moves(conn: &Connection, op_id: i64) -> AppResult<Vec<SourceMsg>> {
    let mut st = conn.prepare(
        "SELECT m.id, COALESCE(m.entry_id,''), COALESCE(m.store_id,''), COALESCE(m.internet_message_id,''), COALESCE(m.folder_entry_id,''), COALESCE(m.folder_path,''), COALESCE(m.subject,'')
         FROM assignment_moves a JOIN email_messages m ON m.id=a.message_id WHERE a.op_id=?1 AND a.moved_at IS NULL ORDER BY m.id",
    )?;
    let v = st
        .query_map([op_id], |r| Ok(SourceMsg { message_id: r.get(0)?, entry_id: r.get(1)?, store_id: r.get(2)?, imid: r.get(3)?, folder_entry_id: r.get(4)?, folder_path: r.get(5)?, subject: r.get(6)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn record_attempt(conn: &Connection, op_id: i64, items: &[SourceMsg]) -> AppResult<()> {
    for i in items {
        conn.execute("UPDATE assignment_moves SET attempts=attempts+1 WHERE op_id=?1 AND message_id=?2", params![op_id, i.message_id])?;
    }
    Ok(())
}

/// Per-email result of the worker's `move_messages`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MoveResult {
    pub message_id: i64,
    pub moved: bool,
    pub new_entry_id: Option<String>,
    pub new_store_id: Option<String>,
    pub new_folder_entry_id: Option<String>,
    pub new_folder_path: Option<String>,
    pub ancestors: Vec<String>,
    pub error_code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MoveFailure {
    pub message_id: i64,
    pub subject: String,
    pub error: String,
}

/// Store move results; finalize the assignment when nothing is left to move.
pub fn apply_moves(conn: &Connection, op_id: i64, current: &ScopeCfg, requested: &[SourceMsg], results: &[MoveResult]) -> AppResult<Vec<MoveFailure>> {
    let op = get_op(conn, op_id)?;
    let tx = conn.unchecked_transaction()?;
    let ts = now();
    let mut failures = Vec::new();
    for m in requested {
        match results.iter().find(|r| r.message_id == m.message_id) {
            Some(r) if r.moved => {
                let path = r.new_folder_path.clone().unwrap_or_else(|| op.dest_path.clone());
                tx.execute("UPDATE assignment_moves SET moved_at=COALESCE(moved_at,?3), new_folder_path=?4, last_error=NULL WHERE op_id=?1 AND message_id=?2", params![op_id, m.message_id, ts, path])?;
                // EntryIDs change when an item moves: save what Outlook returned
                scope::record_move(&tx, current, m.message_id, r.new_entry_id.as_deref().unwrap_or(&m.entry_id), r.new_store_id.as_deref().unwrap_or(&m.store_id),
                    r.new_folder_entry_id.as_deref().unwrap_or(""), &path, &r.ancestors, "reassigned")?;
            }
            other => {
                let err = other.and_then(|r| r.message.clone()).unwrap_or_else(|| "Outlook returned no result for this email.".into());
                tx.execute("UPDATE assignment_moves SET last_error=?3, last_error_at=?4 WHERE op_id=?1 AND message_id=?2", params![op_id, m.message_id, err, ts])?;
                failures.push(MoveFailure { message_id: m.message_id, subject: m.subject.clone(), error: err });
            }
        }
    }
    let left: i64 = tx.query_row("SELECT COUNT(*) FROM assignment_moves WHERE op_id=?1 AND moved_at IS NULL", [op_id], |r| r.get(0))?;
    if left == 0 {
        finalize(&tx, &op)?;
    } else {
        tx.execute("UPDATE assignment_ops SET state='failed' WHERE id=?1", [op_id])?;
        history(&tx, op.ticket_id, &op.operator, "Assignment incomplete - retry needed", None, None, Some(&format!("{left} email(s) not moved yet")))?;
    }
    tx.commit()?;
    Ok(failures)
}

/// Outlook could not be used at all: keep the error on every pending email; the assignment stays open.
pub fn record_worker_failure(conn: &Connection, op_id: i64, items: &[SourceMsg], error: &str) -> AppResult<()> {
    let op = get_op(conn, op_id)?;
    let tx = conn.unchecked_transaction()?;
    for i in items {
        tx.execute("UPDATE assignment_moves SET last_error=?3, last_error_at=?4 WHERE op_id=?1 AND message_id=?2", params![op_id, i.message_id, error, now()])?;
    }
    tx.execute("UPDATE assignment_ops SET state='failed' WHERE id=?1", [op_id])?;
    history(&tx, op.ticket_id, &op.operator, "Assignment failed - retry needed", None, None, Some(error))?;
    tx.commit()?;
    Ok(())
}

/// Every required email is in the new folder: set the owner and status, allow the notification.
pub fn finalize_if_complete(conn: &Connection, op_id: i64) -> AppResult<bool> {
    let op = get_op(conn, op_id)?;
    if op.state == "done" {
        return Ok(true);
    }
    let left: i64 = conn.query_row("SELECT COUNT(*) FROM assignment_moves WHERE op_id=?1 AND moved_at IS NULL", [op_id], |r| r.get(0))?;
    if left > 0 {
        return Ok(false);
    }
    let tx = conn.unchecked_transaction()?;
    finalize(&tx, &op)?;
    tx.commit()?;
    Ok(true)
}

fn finalize(conn: &Connection, op: &AssignmentOp) -> AppResult<()> {
    let ts = now();
    let t = tickets::get(conn, op.ticket_id)?;
    let changed_owner = op.from_staff_id != Some(op.to_staff_id);
    let new_status = if changed_owner || t.status == tickets::UNASSIGNED { tickets::ASSIGNED } else { t.status.as_str() };
    conn.execute(
        "UPDATE tickets SET assignee_staff_id=?2, assigned_at=?3, status=?4, updated_at=?3 WHERE id=?1",
        params![op.ticket_id, op.to_staff_id, ts, new_status],
    )?;
    let to = staff::get(conn, op.to_staff_id)?;
    conn.execute(
        "UPDATE assignment_ops SET state='done', finished_at=?2, notify_state=?3, notify_recipient=?4 WHERE id=?1",
        params![op.id, ts, if changed_owner { "pending" } else { "skipped" }, to.email],
    )?;
    let action = match (&op.from_name, changed_owner) {
        (Some(_), true) => "Reassigned",
        (None, _) => "Assigned",
        (Some(_), false) => "Assignment confirmed (same person - no notification)",
    };
    let detail = format!(
        "{} → {} | folders: {} → {}{}",
        op.from_name.as_deref().unwrap_or("Unassigned"),
        to.display_name,
        op.prev_folders.as_deref().filter(|s| !s.is_empty()).unwrap_or("-"),
        op.dest_path,
        op.note.as_deref().map(|n| format!(" | note: {n}")).unwrap_or_default()
    );
    history(conn, op.ticket_id, &op.operator, action, Some("assignee"), op.from_name.as_deref(), Some(&detail))?;
    if new_status != t.status {
        history(conn, op.ticket_id, &op.operator, "Status changed", Some("status"), Some(&t.status), Some(new_status))?;
    }
    conn.execute("UPDATE tickets SET ownership_conflict=0 WHERE id=?1", [op.ticket_id])?;
    scope::recompute_ticket(conn, op.ticket_id, &op.operator)?;
    crate::search::reindex(conn, op.ticket_id)?;
    Ok(())
}

/// The newest unfinished assignment (moves still to do) or notification to retry for a ticket.
pub fn latest_op(conn: &Connection, ticket_id: i64) -> AppResult<Option<AssignmentOp>> {
    let id: Option<i64> = conn.query_row("SELECT id FROM assignment_ops WHERE ticket_id=?1 AND state<>'superseded' ORDER BY id DESC LIMIT 1", [ticket_id], |r| r.get(0)).optional()?;
    id.map(|i| get_op(conn, i)).transpose()
}

// ------------------------------------------------------------------ notification

#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub op_id: i64,
    pub notification_id: String,
    pub to: String,
    pub subject: String,
    pub html_body: String,
}

/// Atomically reserve the notification for sending. Only "pending" (first send) or "failed" (retry of a
/// send that certainly did not happen) can be claimed - repeated clicks and uncertain sends cannot resend.
pub fn claim_notification(conn: &Connection, op_id: i64) -> AppResult<()> {
    let n = conn.execute(
        "UPDATE assignment_ops SET notify_state='sending', notify_attempts=notify_attempts+1, notify_error=NULL WHERE id=?1 AND state='done' AND notify_state IN ('pending','failed')",
        [op_id],
    )?;
    if n == 0 {
        let st: String = conn.query_row("SELECT notify_state || '/' || state FROM assignment_ops WHERE id=?1", [op_id], |r| r.get(0)).unwrap_or_default();
        let msg = match st.split('/').next().unwrap_or("") {
            "sending" => "The notification is being sent right now.",
            "submitted" => "The notification was already submitted to Outlook.",
            "uncertain" => "The last send attempt has an uncertain outcome. Check the Sent Items first, or confirm a resend.",
            "skipped" => "No notification is needed (same person).",
            _ => "The assignment is not finished yet; the notification is sent after all emails were moved.",
        };
        return Err(AppError::coded("NOTIFY_NOT_ALLOWED", msg));
    }
    Ok(())
}

pub enum NotifyOutcome {
    Submitted { entry_id: Option<String>, account: String },
    /// certainly not sent (e.g. account not found, Outlook not reachable before Send) - Retry allowed
    Failed(String),
    /// Send() may have happened (error during/after Send, worker timeout) - review required
    Uncertain(String),
}

pub fn record_notification(conn: &Connection, op_id: i64, outcome: NotifyOutcome) -> AppResult<()> {
    let op = get_op(conn, op_id)?;
    let ts = now();
    let (state, action, detail) = match &outcome {
        NotifyOutcome::Submitted { entry_id, account } => {
            conn.execute("UPDATE assignment_ops SET notify_state='submitted', notify_at=?2, notify_account=?3, notify_entry_id=?4, notify_error=NULL WHERE id=?1",
                params![op_id, ts, account, entry_id])?;
            ("submitted", "Handoff notification submitted to Outlook (delivery not confirmed)", format!("to {} via {}", op.notify_recipient.as_deref().unwrap_or("?"), if account.is_empty() { "default account" } else { account }))
        }
        NotifyOutcome::Failed(e) => {
            conn.execute("UPDATE assignment_ops SET notify_state='failed', notify_at=?2, notify_error=?3 WHERE id=?1", params![op_id, ts, e])?;
            ("failed", "Assigned — notification failed", e.clone())
        }
        NotifyOutcome::Uncertain(e) => {
            conn.execute("UPDATE assignment_ops SET notify_state='uncertain', notify_at=?2, notify_error=?3 WHERE id=?1", params![op_id, ts, e])?;
            ("uncertain", "Notification outcome uncertain - check Sent Items before resending", e.clone())
        }
    };
    let _ = state;
    history(conn, op.ticket_id, "system", action, Some("notification"), None, Some(&detail))?;
    Ok(())
}

/// Result of looking for the notification in Sent Items / Outbox (uncertain outcome review).
pub fn record_check(conn: &Connection, op_id: i64, found_in: Option<&str>, entry_id: Option<&str>) -> AppResult<String> {
    let op = get_op(conn, op_id)?;
    conn.execute("UPDATE assignment_ops SET notify_checked_at=?2 WHERE id=?1", params![op_id, now()])?;
    match found_in {
        Some(place) => {
            conn.execute("UPDATE assignment_ops SET notify_state='submitted', notify_entry_id=COALESCE(?2, notify_entry_id), notify_error=NULL WHERE id=?1 AND notify_state='uncertain'", params![op_id, entry_id])?;
            history(conn, op.ticket_id, "system", "Notification found in Outlook", Some("notification"), None, Some(place))?;
            Ok(format!("Found in {place}: the notification was submitted (delivery is not confirmed)."))
        }
        None => Ok("Not found in Sent Items or Outbox. If you are sure it was not sent, choose Send again.".into()),
    }
}

/// Staff explicitly confirm that an uncertain notification should be sent again.
pub fn allow_resend(conn: &Connection, op_id: i64, operator: &str) -> AppResult<()> {
    let op = get_op(conn, op_id)?;
    let n = conn.execute("UPDATE assignment_ops SET notify_state='failed' WHERE id=?1 AND notify_state='uncertain'", [op_id])?;
    if n == 0 {
        return Err(AppError::coded("NOTIFY_NOT_ALLOWED", "Only a notification with an uncertain outcome can be confirmed for resending."));
    }
    history(conn, op.ticket_id, operator, "Staff confirmed: send the notification again", Some("notification"), None, None)?;
    Ok(())
}

fn np(v: Option<&str>) -> String {
    match v.map(str::trim) {
        Some(s) if !s.is_empty() && s != "[]" => html_escape(&crate::render::display_value(s)),
        _ => format!("<i>{NOT_PROVIDED}</i>"),
    }
}

fn local(iso: &str) -> String {
    crate::prompt::local_time(Some(iso)).replace("[Not provided]", NOT_PROVIDED)
}

/// Subject and HTML body of the handoff email (sent only to the assigned staff member).
pub fn build_notification(conn: &Connection, op_id: i64) -> AppResult<Notification> {
    let op = get_op(conn, op_id)?;
    let t = tickets::get(conn, op.ticket_id)?;
    let to = staff::get(conn, op.to_staff_id)?;
    let fields: HashMap<String, String> = tickets::fields(conn, op.ticket_id)?.into_iter().map(|f| (f.key, f.effective)).collect();
    let f = |k: &str| fields.get(k).map(String::as_str);
    let issues: Vec<String> = {
        let v = tickets::issues(conn, op.ticket_id)?;
        let ids: Vec<String> = if v.is_empty() { vec![t.issue_type.clone()] } else { v.into_iter().map(|i| i.issue_type).collect() };
        ids.iter().map(|i| catalog().issue(i).map(|d| d.name.clone()).unwrap_or_else(|| i.clone())).collect()
    };
    let school_title = t.school_name.clone().filter(|s| !s.trim().is_empty()).or_else(|| t.original_subject.clone()).unwrap_or_else(|| NOT_PROVIDED.into());
    let subject = format!("ASR/Audit Ticket #{} Assigned to You — {}", t.display_id, school_title);
    let notification_id = op.notify_id.clone().unwrap_or_else(|| format!("op-{}", op.id));
    let row = |k: &str, v: String| format!("<tr><td style=\"padding:3px 12px 3px 0;color:#555;vertical-align:top;white-space:nowrap\">{}</td><td style=\"padding:3px 0\">{}</td></tr>", html_escape(k), v);
    let mut h = String::new();
    h.push_str("<div style=\"font-family:Segoe UI,Arial,sans-serif;font-size:14px;color:#1d2733\">");
    let verb = if op.from_staff_id.is_some() && op.from_staff_id != Some(op.to_staff_id) { "reassigned" } else { "assigned" };
    h.push_str(&format!("<p>Hello {},</p><p><b>{}</b> {verb} this ASR/Audit support ticket to you on {}.</p>",
        html_escape(&to.display_name), html_escape(&op.operator), local(op.finished_at.as_deref().unwrap_or(&op.created_at))));
    h.push_str("<table style=\"border-collapse:collapse\">");
    h.push_str(&row("Ticket", format!("{} (reference {}; the T-number is local to the sender's computer)", html_escape(&t.display_id), np(t.ticket_ref.as_deref()))));
    h.push_str(&row("Issue type(s)", html_escape(&issues.join("; "))));
    h.push_str(&row("Previous assignee", np(op.from_name.as_deref())));
    h.push_str(&row("New assignee", format!("{} &lt;{}&gt;", html_escape(&to.display_name), html_escape(&to.email))));
    h.push_str(&row("Assigned by", format!("{} on {}", html_escape(&op.operator), local(op.finished_at.as_deref().unwrap_or(&op.created_at)))));
    for slot in tickets::school_slots(conn, op.ticket_id)? {
        let k = |a: &str| crate::analyzer::school_key(slot, a);
        let label = if slot == 1 { "School".to_string() } else { format!("School {slot}") };
        let addr: Vec<String> = ["address", "municipality", "county", "zip"].iter().filter_map(|a| f(&k(a)).filter(|v| !v.trim().is_empty()).map(|v| v.trim().to_string())).collect();
        h.push_str(&row(&format!("{label} name"), np(f(&k("name")))));
        h.push_str(&row(&format!("{label} NJIIS ID"), np(f(&k("njiis_id")))));
        h.push_str(&row(&format!("{label} other IDs"), np(f(&k("other_ids")))));
        h.push_str(&row(&format!("{label} address"), if addr.is_empty() { np(None) } else { html_escape(&addr.join(", ")) }));
    }
    h.push_str(&row("NJIIS facility identifier", np(f("facility_id"))));
    let phone = match (f("sender_phone").filter(|p| !p.trim().is_empty()), f("sender_phone_ext").filter(|p| !p.trim().is_empty())) {
        (Some(p), Some(x)) => html_escape(&format!("{p} ext. {x}")),
        (Some(p), None) => html_escape(p),
        _ => np(None),
    };
    h.push_str(&row("Sender", np(t.sender_name.as_deref())));
    h.push_str(&row("Sender title", np(f("sender_title"))));
    h.push_str(&row("Sender email", np(t.sender_email.as_deref())));
    h.push_str(&row("Sender phone", phone));
    h.push_str(&row("Original subject", np(t.original_subject.as_deref())));
    h.push_str(&row("Email location", format!("{} — the original email and its attachments are in this folder", html_escape(&op.dest_path))));
    h.push_str("</table>");
    let block = |title: &str, body: String| format!("<h3 style=\"font-size:15px;margin:18px 0 6px\">{}</h3>{}", html_escape(title), body);
    let pre = |s: Option<&str>| match s.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => format!("<div style=\"white-space:pre-wrap;border-left:3px solid #c9d1db;padding:4px 10px;background:#f8fafc\">{}</div>", html_escape(s)),
        None => format!("<p><i>{NOT_PROVIDED}</i></p>"),
    };
    h.push_str(&block("Handoff instructions", pre(op.note.as_deref())));
    h.push_str(&block("Staff actions already taken", pre(t.staff_actions.as_deref())));
    let note_text = crate::notes::list(conn, op.ticket_id)?.into_iter().map(|n| format!("{} ({})", n.body.trim(), n.author_name)).collect::<Vec<_>>().join("

");
    h.push_str(&block("Staff notes", pre(Some(note_text.as_str()))));
    let mut msgs = tickets::messages(conn, op.ticket_id)?;
    msgs.sort_by(|a, b| a.received_at.cmp(&b.received_at).then(a.id.cmp(&b.id)));
    let orig_idx = msgs.iter().position(|m| m.link_type == "original" || m.link_type == "manual").or_else(|| msgs.iter().position(|m| !m.is_internal));
    match orig_idx.map(|i| &msgs[i]) {
        Some(m) => {
            let head = format!("<p style=\"color:#555\">From {} &lt;{}&gt; · {} · {}</p>", np(m.sender_name.as_deref()), html_escape(m.sender_email.as_deref().unwrap_or("")),
                local(m.received_at.as_deref().unwrap_or("")), np(m.subject.as_deref()));
            h.push_str(&block("Original email", head + &pre(m.body_text.as_deref())));
        }
        None => h.push_str(&block("Original email", format!("<p><i>{NOT_PROVIDED}</i></p>"))),
    }
    let others: Vec<String> = msgs
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != orig_idx)
        .map(|(_, m)| {
            let body = m.body_text.as_deref().unwrap_or("");
            let (cur, _) = split_current(body);
            format!("<p style=\"color:#555;margin-bottom:2px\">{} · {} · {}</p>{}", np(m.sender_name.as_deref().or(m.sender_email.as_deref())),
                local(m.received_at.as_deref().unwrap_or("")), np(m.subject.as_deref()), pre(Some(if cur.trim().is_empty() { body } else { cur.as_str() })))
        })
        .collect();
    h.push_str(&block("Conversation context", if others.is_empty() { "<p><i>None</i></p>".into() } else { others.join("") }));
    h.push_str(&format!(
        "<hr><p style=\"color:#777;font-size:12px\">Sent automatically by ASR/Audit Support Desk to the assigned staff member only. \
         The school/sender was not notified. Ticket notes, statuses and history are stored on the assigning computer and are not shared.<br>[{NOTIFY_MARKER}: {}]</p></div>",
        html_escape(&notification_id)
    ));
    Ok(Notification { op_id, notification_id, to: to.email, subject, html_body: h })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::tests_support::settings_wen_scope;
    use crate::staff::tests::input;
    use crate::tickets::tests::{conn, msg};
    use crate::tickets::{get, ingest, ASSIGNED, DONE, UNASSIGNED};

    fn setup() -> (Connection, i64, i64, ScopeCfg) {
        let c = conn();
        let s = settings_wen_scope();
        let w = staff::save(&c, &input("Wenjie", "wenjie@example.org", "F-WEN")).unwrap();
        let a = staff::save(&c, &input("Aaron", "aaron@example.org", "F-AAR")).unwrap();
        let mut m1 = msg(1, "C1", "Subtype", "Our school is missing the subtype. School Name: Maple Leaf Academy\nSchool NJIIS ID: 0012345", "jane@school.org");
        m1.folder_entry_id = "F-INBOX".into();
        m1.folder_path = "Inbox".into();
        let mut m2 = msg(2, "C1", "RE: Subtype", "Also the webinar is black.", "jane@school.org");
        m2.folder_entry_id = "F-INBOX".into();
        m2.folder_path = "Inbox".into();
        ingest(&c, &[m1, m2], &s).unwrap();
        (c, w.id, a.id, scope::cfg(&s))
    }

    fn moved(id: i64, entry: &str, folder: &str, path: &str) -> MoveResult {
        MoveResult { message_id: id, moved: true, new_entry_id: Some(entry.into()), new_store_id: Some("S1".into()), new_folder_entry_id: Some(folder.into()),
            new_folder_path: Some(path.into()), ancestors: vec!["F-INBOX".into(), "F-ROOT".into()], ..Default::default() }
    }

    #[test]
    fn assignment_moves_then_finalizes_and_saves_new_ids() {
        let (c, w, _a, sc) = setup();
        assert_eq!(get(&c, 1).unwrap().status, UNASSIGNED);
        let p = preview(&c, 1, w).unwrap();
        assert_eq!((p.emails, p.already_in_folder, p.will_notify, p.blocked.is_none()), (2, 0, true, true));
        let Begin::Op(op) = begin(&c, 1, w, "Sam", "Please call the nurse").unwrap() else { panic!() };
        let pend = pending_moves(&c, op).unwrap();
        assert_eq!(pend.len(), 2);
        let f = apply_moves(&c, op, &sc, &pend, &[moved(pend[0].message_id, "N1", "F-WEN", "Inbox/Wenjie"), moved(pend[1].message_id, "N2", "F-WEN", "Inbox/Wenjie")]).unwrap();
        assert!(f.is_empty());
        let t = get(&c, 1).unwrap();
        assert_eq!((t.status.as_str(), t.assignee_staff_id), (ASSIGNED, Some(w)));
        let e: String = c.query_row("SELECT entry_id FROM email_messages WHERE id=?1", [pend[0].message_id], |r| r.get(0)).unwrap();
        assert_eq!(e, "N1", "identifier returned by Outlook is saved");
        let o = get_op(&c, op).unwrap();
        assert_eq!((o.state.as_str(), o.notify_state.as_str()), ("done", "pending"));
        assert_eq!(o.notify_recipient.as_deref(), Some("wenjie@example.org"));
        assert_eq!(t.scope_state, "current", "moved within the selected scope (Wenjie folder) - stays visible");
        assert!(tickets::history_rows(&c, 1).unwrap().iter().any(|h| h.action == "Assigned" && h.new_value.as_deref().unwrap_or("").contains("note: Please call the nurse")));
    }

    #[test]
    fn partial_failure_retry_moves_only_the_rest_and_no_notification_before_done() {
        let (c, _w, a, sc) = setup();
        let Begin::Op(op) = begin(&c, 1, a, "Sam", "").unwrap() else { panic!() };
        let pend = pending_moves(&c, op).unwrap();
        let fail = MoveResult { message_id: pend[1].message_id, error_code: Some("MOVE_FAILED".into()), message: Some("no permission".into()), ..Default::default() };
        let f = apply_moves(&c, op, &sc, &pend, &[moved(pend[0].message_id, "N1", "F-AAR", "Inbox/Aaron"), fail]).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(get_op(&c, op).unwrap().state, "failed");
        assert_eq!(get(&c, 1).unwrap().status, UNASSIGNED, "not finalized");
        assert!(claim_notification(&c, op).is_err(), "no notification until every move succeeded");
        // retry: begin() resumes the same operation; only the failed email is pending
        assert_eq!(begin(&c, 1, a, "Sam", "").unwrap(), Begin::Op(op));
        let pend2 = pending_moves(&c, op).unwrap();
        assert_eq!(pend2.len(), 1);
        apply_moves(&c, op, &sc, &pend2, &[moved(pend2[0].message_id, "N2", "F-AAR", "Inbox/Aaron")]).unwrap();
        let t = get(&c, 1).unwrap();
        assert_eq!((t.status.as_str(), t.assignee_staff_id), (ASSIGNED, Some(a)));
        assert_eq!(t.scope_state, "history", "moved outside the selected scope (Aaron's folder) after confirmation");
        assert_eq!(t.scope_reason.as_deref(), Some("reassigned"));
    }

    #[test]
    fn notification_claims_prevent_duplicates_and_failures_keep_the_assignment() {
        let (c, w, _a, sc) = setup();
        let Begin::Op(op) = begin(&c, 1, w, "Sam", "").unwrap() else { panic!() };
        let pend = pending_moves(&c, op).unwrap();
        apply_moves(&c, op, &sc, &pend, &pend.iter().map(|m| moved(m.message_id, "N", "F-WEN", "Inbox/Wenjie")).collect::<Vec<_>>()).unwrap();
        claim_notification(&c, op).unwrap();
        assert!(claim_notification(&c, op).is_err(), "a second click while sending is refused");
        record_notification(&c, op, NotifyOutcome::Failed("Account not found".into())).unwrap();
        assert_eq!(get(&c, 1).unwrap().status, ASSIGNED, "assignment preserved");
        assert!(tickets::history_rows(&c, 1).unwrap().iter().any(|h| h.action == "Assigned — notification failed"));
        claim_notification(&c, op).unwrap(); // Retry Notification - no moves repeated
        assert!(pending_moves(&c, op).unwrap().is_empty());
        record_notification(&c, op, NotifyOutcome::Uncertain("Outlook did not answer after Send".into())).unwrap();
        assert!(claim_notification(&c, op).is_err(), "uncertain outcome is never resent automatically");
        assert!(record_check(&c, op, None, None).unwrap().contains("Not found"));
        assert_eq!(get_op(&c, op).unwrap().notify_state, "uncertain");
        allow_resend(&c, op, "Sam").unwrap();
        claim_notification(&c, op).unwrap();
        record_notification(&c, op, NotifyOutcome::Submitted { entry_id: Some("SENT-1".into()), account: "sam@example.org".into() }).unwrap();
        let o = get_op(&c, op).unwrap();
        assert_eq!((o.notify_state.as_str(), o.notify_attempts), ("submitted", 3));
        assert!(claim_notification(&c, op).is_err(), "never sent twice once submitted");
        // uncertain -> found in Sent Items
        let Begin::Op(op2) = begin(&c, 1, _a, "Sam", "").unwrap() else { panic!() };
        let pend = pending_moves(&c, op2).unwrap();
        apply_moves(&c, op2, &sc, &pend, &pend.iter().map(|m| moved(m.message_id, "A", "F-AAR", "Inbox/Aaron")).collect::<Vec<_>>()).unwrap();
        claim_notification(&c, op2).unwrap();
        record_notification(&c, op2, NotifyOutcome::Uncertain("timeout".into())).unwrap();
        record_check(&c, op2, Some("Sent Items"), Some("S-2")).unwrap();
        assert_eq!(get_op(&c, op2).unwrap().notify_state, "submitted");
    }

    #[test]
    fn same_person_no_change_and_done_tickets_need_reopen() {
        let (c, w, _a, sc) = setup();
        let Begin::Op(op) = begin(&c, 1, w, "Sam", "").unwrap() else { panic!() };
        let pend = pending_moves(&c, op).unwrap();
        apply_moves(&c, op, &sc, &pend, &pend.iter().map(|m| moved(m.message_id, &format!("N{}", m.message_id), "F-WEN", "Inbox/Wenjie")).collect::<Vec<_>>()).unwrap();
        assert_eq!(begin(&c, 1, w, "Sam", "again").unwrap(), Begin::NoChange, "same person, emails already there: no operation, no notification");
        // email moved back out by hand -> moving it again for the same person is allowed but not notified
        c.execute("UPDATE email_messages SET folder_entry_id='F-INBOX', folder_path='Inbox' WHERE id=?1", [pend[0].message_id]).unwrap();
        let Begin::Op(op2) = begin(&c, 1, w, "Sam", "").unwrap() else { panic!() };
        let p2 = pending_moves(&c, op2).unwrap();
        assert_eq!(p2.len(), 1, "an email already in the destination counts as moved");
        apply_moves(&c, op2, &sc, &p2, &[moved(p2[0].message_id, "N9", "F-WEN", "Inbox/Wenjie")]).unwrap();
        assert_eq!(get_op(&c, op2).unwrap().notify_state, "skipped");
        tickets::set_done(&c, 1, "Sam", "Marked Done").unwrap();
        assert_eq!(begin(&c, 1, _a, "Sam", "").unwrap_err().code(), "TICKET_DONE");
        assert!(preview(&c, 1, _a).unwrap().blocked.is_some());
        staff::set_active(&c, _a, false).unwrap();
        tickets::reopen(&c, 1, "Sam", "Reopened by staff").unwrap();
        assert!(begin(&c, 1, _a, "Sam", "").is_err(), "inactive staff cannot receive assignments");
    }

    #[test]
    fn notification_content_and_recipient() {
        let (c, w, a, sc) = setup();
        tickets::update(&c, 1, &tickets::TicketPatch { staff_actions: Some("Checked NJIIS".into()), internal_notes: Some("Call after 2pm".into()), ..Default::default() }, "Sam").unwrap();
        let Begin::Op(op) = begin(&c, 1, w, "Sam", "").unwrap() else { panic!() };
        let pend = pending_moves(&c, op).unwrap();
        apply_moves(&c, op, &sc, &pend, &pend.iter().map(|m| moved(m.message_id, "N", "F-WEN", "Inbox/Wenjie")).collect::<Vec<_>>()).unwrap();
        let Begin::Op(op2) = begin(&c, 1, a, "Sam", "Please take over - parent called twice").unwrap() else { panic!() };
        let pend = pending_moves(&c, op2).unwrap();
        apply_moves(&c, op2, &sc, &pend, &pend.iter().map(|m| moved(m.message_id, "A", "F-AAR", "Inbox/Aaron")).collect::<Vec<_>>()).unwrap();
        let n = build_notification(&c, op2).unwrap();
        assert_eq!(n.to, "aaron@example.org", "only the assigned staff member");
        assert_eq!(n.subject, "ASR/Audit Ticket #T-000001 Assigned to You — Maple Leaf Academy");
        for needle in ["reassigned this", "Previous assignee", "Wenjie", "Aaron", "0012345", "Please take over - parent called twice", "Checked NJIIS",
            "Call after 2pm", "Our school is missing the subtype", "Also the webinar is black", "Team › Inbox/Aaron", "attachments are in this folder",
            "<i>Not provided</i>", "[ASR-Desk-Notification: ASR-"] {
            assert!(n.html_body.contains(needle), "missing {needle:?}");
        }
        assert!(n.html_body.contains("The school/sender was not notified"));
    }

    #[test]
    fn bulk_assignment_tracks_each_ticket_independently() {
        let (c, w, _a, sc) = setup();
        let s = settings_wen_scope();
        ingest(&c, &[msg(5, "C5", "User guide", "where is the user guide", "x@school.org"), msg(6, "C6", "PDF", "save as pdf", "y@school.org")], &s).unwrap();
        let ids = [1i64, 2, 3];
        let mut done = vec![];
        for (n, id) in ids.iter().enumerate() {
            let Begin::Op(op) = begin(&c, *id, w, "Sam", "bulk").unwrap() else { panic!() };
            let pend = pending_moves(&c, op).unwrap();
            let res: Vec<MoveResult> = pend
                .iter()
                .map(|m| if n == 1 { MoveResult { message_id: m.message_id, message: Some("locked".into()), ..Default::default() } } else { moved(m.message_id, &format!("B{}", m.message_id), "F-WEN", "Inbox/Wenjie") })
                .collect();
            apply_moves(&c, op, &sc, &pend, &res).unwrap();
            done.push(get_op(&c, op).unwrap().state);
        }
        assert_eq!(done, vec!["done", "failed", "done"], "a failure in one ticket does not affect the others");
        assert_eq!(get(&c, 1).unwrap().status, ASSIGNED);
        assert_eq!(get(&c, 2).unwrap().status, UNASSIGNED);
        assert_eq!(get(&c, 3).unwrap().status, ASSIGNED);
        let l = tickets::list(&c, &tickets::ListQuery { view: "current".into(), assignee: w.to_string(), ..Default::default() }).unwrap();
        assert_eq!(l.total, 2, "My Tickets");
        let l = tickets::list(&c, &tickets::ListQuery { view: "current".into(), assignee: "none".into(), ..Default::default() }).unwrap();
        assert_eq!(l.total, 1, "Unassigned shortcut");
        assert!(l.rows[0].in_scope_count >= 1);
    }

    #[test]
    fn notifications_are_never_imported_and_mark_done_still_works_after_assignment() {
        let (c, w, _a, sc) = setup();
        let s = settings_wen_scope();
        let mut note = msg(9, "C9", "ASR/Audit Ticket #T-000001 Assigned to You — Maple", "[ASR-Desk-Notification: x]", "sam@example.org");
        note.notification_id = "ASR-1-1".into();
        let r = ingest(&c, &[note], &s).unwrap();
        assert_eq!((r.skipped_notifications, r.created_tickets, r.new_messages), (1, 0, 0));
        let Begin::Op(op) = begin(&c, 1, w, "Sam", "").unwrap() else { panic!() };
        let pend = pending_moves(&c, op).unwrap();
        apply_moves(&c, op, &sc, &pend, &pend.iter().map(|m| moved(m.message_id, &format!("W{}", m.message_id), "F-WEN", "Inbox/Wenjie")).collect::<Vec<_>>()).unwrap();
        // Mark Done uses the new locations and keeps the owner
        let items = crate::completion::pending(&c, 1).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.entry_id.starts_with('W')));
        let dest = crate::settings::FolderRef { store_id: "S1".into(), entry_id: "F-DONE".into(), path: "Inbox/Completed".into() };
        let res: Vec<crate::completion::ItemResult> = items.iter().map(|i| crate::completion::ItemResult { message_id: i.message_id, flagged: true, moved: true,
            new_entry_id: Some(format!("D{}", i.message_id)), new_folder_path: Some("Inbox/Completed".into()), ..Default::default() }).collect();
        let o = crate::completion::apply_results(&c, 1, &dest, &items, &res, "Sam").unwrap();
        assert!(o.done);
        let t = get(&c, 1).unwrap();
        assert_eq!((t.status.as_str(), t.assignee_staff_id), (DONE, Some(w)));
    }
}
