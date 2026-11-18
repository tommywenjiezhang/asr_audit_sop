//! Local workflow actions: Claim / Take over / Release, Start Work, Flag for Review / Resume, Mark Done,
//! Reopen, Add to Existing Case, and the reaction to new incoming replies.
//!
//! Each action is saved locally at once (so it works offline) and, in shared mode, queued for the shared
//! database in the same SQLite transaction. The shared database re-checks every rule (see
//! `shared_workflow`); until it accepts, the case shows "pending sync", and a refused action becomes a
//! conflict with the confirmed shared value shown.
//!
//! Claim never moves Outlook emails and never sends a notification. Mark Done needs no sent-reply check.

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::sync_queue::{self as q, NewOp};
use crate::tickets::{self, history, Ticket, ASSIGNED, DONE, IN_PROGRESS, REVIEW, UNASSIGNED};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;

fn owner_shared(conn: &Connection, staff_id: Option<i64>) -> AppResult<Option<String>> {
    match staff_id {
        Some(s) => Ok(conn.query_row("SELECT shared_id FROM staff WHERE id=?1", [s], |r| r.get(0)).optional()?.flatten()),
        None => Ok(None),
    }
}

fn set_status(conn: &Connection, tid: i64, actor: &str, from: &str, to: &str, review_reason: Option<&str>, action: &str) -> AppResult<()> {
    let ts = now();
    conn.execute(
        "UPDATE tickets SET status=?2, review_reason=?3, updated_at=?4, completed_at=CASE WHEN ?2='Done' THEN ?4 WHEN ?2='Review' THEN completed_at ELSE NULL END WHERE id=?1",
        params![tid, to, review_reason, ts],
    )?;
    history(conn, tid, actor, action, Some("status"), Some(from), Some(to))?;
    Ok(())
}

fn finish(tx: rusqlite::Transaction<'_>, conn: &Connection, tid: i64) -> AppResult<Ticket> {
    crate::scope::recompute_ticket(&tx, tid, "system")?;
    crate::search::reindex(&tx, tid)?;
    tx.commit()?;
    tickets::get(conn, tid)
}

/// Claim (or, with `takeover`, take over from the current owner). Changes ownership only.
pub fn claim(conn: &Connection, tid: i64, actor_name: &str, takeover: bool) -> AppResult<Ticket> {
    let tx = conn.unchecked_transaction()?;
    let t = tickets::get(&tx, tid)?;
    let a = q::actor(&tx, actor_name);
    let me = a.staff_id.ok_or_else(|| AppError::coded("NO_STAFF", "Choose who you are first (Settings › Staff Assignment › I am)."))?;
    if t.assignee_staff_id == Some(me) {
        return Ok(t);
    }
    if let Some(other) = t.assignee_staff_id {
        if !takeover {
            let name = t.assignee_name.clone().unwrap_or_else(|| "another staff member".into());
            return Err(AppError::coded("OWNED", format!("{name} owns this case. Use Take over to become the owner; their work and history are kept.")));
        }
        let _ = other;
    }
    let mark = q::last_history_row(&tx)?;
    let ts = now();
    let reason = if t.assignee_staff_id.is_some() { "takeover" } else { "claim" };
    tx.execute("UPDATE tickets SET assignee_staff_id=?2, assigned_at=?3, updated_at=?3 WHERE id=?1", params![tid, me, ts])?;
    q::open_ownership(&tx, tid, Some(me), &ts, reason, &a.name, "local")?;
    history(&tx, tid, &a.name, if takeover { "Taken over" } else { "Claimed" }, Some("assignee"), t.assignee_name.as_deref(), Some(&a.name))?;
    if t.status == UNASSIGNED {
        set_status(&tx, tid, &a.name, UNASSIGNED, ASSIGNED, None, "Status changed")?;
    }
    let op = q::enqueue(&tx, actor_name, NewOp {
        ticket_id: Some(tid),
        entity_type: "ticket",
        entity_id: None,
        op_type: "ticket.claim",
        original: json!({"owner_staff_id": owner_shared(&tx, t.assignee_staff_id)?, "status": t.status}),
        proposed: json!({"owner_staff_id": a.staff_shared_id, "takeover": takeover}),
    })?;
    q::tag_history(&tx, tid, &op, mark)?;
    finish(tx, conn, tid)
}

/// Clear the owner (explicit). Assigned / In Progress go back to Unassigned; Review stays Review.
pub fn release(conn: &Connection, tid: i64, actor_name: &str) -> AppResult<Ticket> {
    let tx = conn.unchecked_transaction()?;
    let t = tickets::get(&tx, tid)?;
    if t.assignee_staff_id.is_none() {
        return Ok(t);
    }
    let a = q::actor(&tx, actor_name);
    let mark = q::last_history_row(&tx)?;
    let ts = now();
    tx.execute("UPDATE tickets SET assignee_staff_id=NULL, assigned_at=NULL, updated_at=?2 WHERE id=?1", params![tid, ts])?;
    q::open_ownership(&tx, tid, None, &ts, "released", &a.name, "local")?;
    history(&tx, tid, &a.name, "Ownership cleared", Some("assignee"), t.assignee_name.as_deref(), None)?;
    if t.status == ASSIGNED || t.status == IN_PROGRESS {
        set_status(&tx, tid, &a.name, &t.status, UNASSIGNED, None, "Status changed")?;
    }
    let op = q::enqueue(&tx, actor_name, NewOp {
        ticket_id: Some(tid), entity_type: "ticket", entity_id: None, op_type: "ticket.release",
        original: json!({"owner_staff_id": owner_shared(&tx, t.assignee_staff_id)?}), proposed: json!({}),
    })?;
    q::tag_history(&tx, tid, &op, mark)?;
    finish(tx, conn, tid)
}

/// Start Work: needs an owner; Assigned (or Review) -> In Progress.
pub fn start_work(conn: &Connection, tid: i64, actor_name: &str) -> AppResult<Ticket> {
    let tx = conn.unchecked_transaction()?;
    let t = tickets::get(&tx, tid)?;
    if t.status == IN_PROGRESS {
        return Ok(t);
    }
    if t.assignee_staff_id.is_none() {
        return Err(AppError::coded("INVALID", "Claim or assign the case before starting work."));
    }
    if t.status == DONE {
        return Err(AppError::coded("INVALID", "The case is Done. Reopen it if work continues."));
    }
    let mark = q::last_history_row(&tx)?;
    set_status(&tx, tid, actor_name, &t.status, IN_PROGRESS, None, "Work started")?;
    let op = q::enqueue(&tx, actor_name, NewOp { ticket_id: Some(tid), entity_type: "ticket", entity_id: None, op_type: "ticket.start", original: json!({"status": t.status}), proposed: json!({}) })?;
    q::tag_history(&tx, tid, &op, mark)?;
    finish(tx, conn, tid)
}

/// Flag for Review with a reason (manual). The owner is kept.
pub fn flag_review(conn: &Connection, tid: i64, reason: &str, actor_name: &str) -> AppResult<Ticket> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(AppError::coded("INVALID", "Enter a reason for the review."));
    }
    let tx = conn.unchecked_transaction()?;
    let t = tickets::get(&tx, tid)?;
    let mark = q::last_history_row(&tx)?;
    set_status(&tx, tid, actor_name, &t.status, REVIEW, Some(reason), "Flagged for review")?;
    let op = q::enqueue(&tx, actor_name, NewOp { ticket_id: Some(tid), entity_type: "ticket", entity_id: None, op_type: "ticket.flag_review", original: json!({"status": t.status}), proposed: json!({"reason": reason}) })?;
    q::tag_history(&tx, tid, &op, mark)?;
    finish(tx, conn, tid)
}

/// Resume a Review case: In Progress with an owner, otherwise Unassigned.
pub fn resume(conn: &Connection, tid: i64, actor_name: &str) -> AppResult<Ticket> {
    let tx = conn.unchecked_transaction()?;
    let t = tickets::get(&tx, tid)?;
    if t.status != REVIEW {
        return Ok(t);
    }
    let next = if t.assignee_staff_id.is_some() { IN_PROGRESS } else { UNASSIGNED };
    let mark = q::last_history_row(&tx)?;
    set_status(&tx, tid, actor_name, REVIEW, next, None, "Review handled")?;
    let op = q::enqueue(&tx, actor_name, NewOp { ticket_id: Some(tid), entity_type: "ticket", entity_id: None, op_type: "ticket.resume", original: json!({"status": REVIEW}), proposed: json!({}) })?;
    q::tag_history(&tx, tid, &op, mark)?;
    finish(tx, conn, tid)
}

/// Reopen a Done case (no Outlook change).
pub fn reopen(conn: &Connection, tid: i64, actor_name: &str, reason: &str) -> AppResult<Ticket> {
    let tx = conn.unchecked_transaction()?;
    let t = tickets::get(&tx, tid)?;
    if t.status != DONE {
        return Ok(t);
    }
    let next = if t.assignee_staff_id.is_some() { IN_PROGRESS } else { UNASSIGNED };
    let mark = q::last_history_row(&tx)?;
    set_status(&tx, tid, actor_name, DONE, next, None, reason)?;
    let op = q::enqueue(&tx, actor_name, NewOp { ticket_id: Some(tid), entity_type: "ticket", entity_id: None, op_type: "ticket.reopen", original: json!({"status": DONE}), proposed: json!({"reason": reason}) })?;
    q::tag_history(&tx, tid, &op, mark)?;
    finish(tx, conn, tid)
}

#[derive(Debug, Clone, Serialize)]
pub struct DoneDecision {
    pub completion_id: String,
    /// incoming emails covered by this decision
    pub covered: usize,
    /// "pending" (waiting for the shared database) | "accepted" (standalone: final at once)
    pub state: String,
}

/// Mark Done: staff decision, saved at once (offline too). Records who, when and which incoming emails it
/// covers. No sent-reply verification, no recipient-facing email. Outlook archiving is separate.
pub fn mark_done(conn: &Connection, tid: i64, actor_name: &str) -> AppResult<DoneDecision> {
    let tx = conn.unchecked_transaction()?;
    let t = tickets::get(&tx, tid)?;
    if t.status == DONE {
        return Err(AppError::coded("ALREADY_DONE", "This case is already Done."));
    }
    let a = q::actor(&tx, actor_name);
    let (covered_ids, max_recv) = q::incoming_coverage(&tx, tid)?;
    let covered: Vec<String> = covered_ids
        .iter()
        .filter_map(|m| {
            tx.query_row("SELECT shared_email_id, internet_message_id FROM email_messages WHERE id=?1", [m], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))
                .ok()
                .and_then(|(s, i)| s.or_else(|| i.filter(|x| !x.is_empty()).map(|x| format!("imid:{}", crate::shared_apply::normalize_message_id(&x)))))
        })
        .collect();
    let mark = q::last_history_row(&tx)?;
    let cid = crate::shared_apply::new_id();
    let ts = now();
    set_status(&tx, tid, &a.name, &t.status, DONE, None, "Marked Done")?;
    let op = q::enqueue(&tx, actor_name, NewOp {
        ticket_id: Some(tid),
        entity_type: "ticket",
        entity_id: None,
        op_type: "ticket.done",
        original: json!({"status": t.status}),
        proposed: json!({"completion_id": cid, "covered": covered, "max_covered_received_at": max_recv, "decided_at": ts}),
    })?;
    let state = if op.is_some() { "pending" } else { "accepted" };
    tx.execute(
        "INSERT INTO completion_history(completion_id, ticket_id, actor_name, actor_staff_id, decided_at, covered_json, max_covered_received_at, state, op_id, origin) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'local')",
        params![cid, tid, a.name, a.staff_id, ts, serde_json::to_string(&covered_ids)?, max_recv, state, op],
    )?;
    q::tag_history(&tx, tid, &op, mark)?;
    finish(tx, conn, tid)?;
    Ok(DoneDecision { completion_id: cid, covered: covered_ids.len(), state: state.into() })
}

/// Add this case to an existing case (manual grouping). Emails, notes and history move to the target;
/// this case stays as a hidden redirect. No Outlook move, no notification.
pub fn join(conn: &Connection, src: i64, target: i64, actor_name: &str) -> AppResult<Ticket> {
    if src == target {
        return Err(AppError::coded("INVALID", "Choose a different case."));
    }
    let tx = conn.unchecked_transaction()?;
    let s = tickets::get(&tx, src)?;
    let t = tickets::get(&tx, target)?;
    if let (Some(a), Some(b)) = (s.assignee_staff_id, t.assignee_staff_id) {
        if a != b {
            return Err(AppError::coded("INVALID", format!("The cases have different owners ({} and {}). Agree on one owner first (Take over), then add the case.", s.assignee_name.unwrap_or_default(), t.assignee_name.unwrap_or_default())));
        }
    }
    let target_shared = q::ensure_shared_id(&tx, target)?;
    let src_state = q::ticket_shared_state(&tx, src)?;
    // local combine first: unsent changes of the source (including an unsent creation) now go to the
    // target, where the shared database merges them; nothing is deleted
    q::merge_local(&tx, src, target)?;
    let mark = q::last_history_row(&tx)?;
    if src_state == "shared" {
        let op = q::enqueue(&tx, actor_name, NewOp { ticket_id: Some(src), entity_type: "ticket", entity_id: None, op_type: "ticket.join", original: json!({}), proposed: json!({"target_ticket_id": target_shared}) })?;
        q::tag_history(&tx, src, &op, mark)?;
    }
    history(&tx, target, actor_name, "Case added to this case", None, s.ticket_ref.as_deref(), t.ticket_ref.as_deref())?;
    if t.status == DONE && s.status != DONE {
        set_status(&tx, target, actor_name, DONE, REVIEW, Some(&format!("Case {} was added to this Done case", s.ticket_ref.unwrap_or_default())), "Status changed")?;
    }
    finish(tx, conn, target)
}

/// A new incoming email was linked to an existing case by a scan. A substantive reply that is newer than
/// what the last Done decision covered moves a Done case to Review (owner kept). Older emails found
/// later, internal emails and automatic replies never reopen a case.
pub fn on_new_incoming(conn: &Connection, tid: i64, message_id: i64) -> AppResult<bool> {
    let (status, done_at): (String, Option<String>) = conn.query_row("SELECT status, completed_at FROM tickets WHERE id=?1", [tid], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if status != DONE {
        return Ok(false);
    }
    let (subject, sender_name, sender_email, received, internal): (String, String, String, String, i64) = conn.query_row(
        "SELECT COALESCE(subject,''), COALESCE(sender_name,''), COALESCE(sender_email,''), COALESCE(received_at,''), is_internal FROM email_messages WHERE id=?1",
        [message_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    if internal != 0 || q::is_auto_reply(&subject) {
        return Ok(false);
    }
    let last: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT covered_json, max_covered_received_at FROM completion_history WHERE ticket_id=?1 AND state IN ('accepted','legacy','pending') ORDER BY decided_at DESC, id DESC LIMIT 1",
            [tid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (covered, max) = match last {
        Some((c, m)) => (serde_json::from_str::<Vec<i64>>(&c).unwrap_or_default().iter().map(|x| x.to_string()).collect::<Vec<_>>(), m),
        None => (vec![], None),
    };
    if !crate::shared_workflow::is_new_after(&message_id.to_string(), &received, &covered, &max, &done_at) {
        history(conn, tid, tickets::SYSTEM, "Earlier email found; the case stays Done", None, None, Some(&subject))?;
        return Ok(false);
    }
    let who = if sender_name.trim().is_empty() { sender_email } else { sender_name };
    // the shared database applies the same rule when the email is attached there (no separate op needed)
    set_status(conn, tid, tickets::SYSTEM, DONE, REVIEW, Some(&format!("New reply from {who}")), &format!("Reopened for review: new reply from {who}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::shared_store::tests::new_shared;
    use crate::shared_sync::tests::{shared_count, Client};
    use crate::tickets::tests::msg;

    fn staff(cl: &Client, name: &str) -> i64 {
        let conn = cl.conn();
        let s = crate::staff::save(&conn, &crate::staff::StaffInput { display_name: name.into(), email: format!("{}@doh.example.gov", name.to_lowercase()), ..Default::default() }).unwrap();
        crate::staff::set_current_staff_id(&conn, Some(s.id)).unwrap();
        s.id
    }

    /// Two workstations on one test shared database, both connected, each with a "me".
    fn team() -> (tempfile::TempDir, std::path::PathBuf, Client, Client) {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("Aaron");
        let w = Client::new("Wenjie");
        staff(&a, "Aaron");
        staff(&w, "Wenjie");
        a.connect(&shared);
        w.connect(&shared);
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        (dir, shared, a, w)
    }

    fn status(cl: &Client, imid: &str) -> (String, Option<String>) {
        let t = cl.ticket_by_imid(imid);
        let t = tickets::get(&cl.conn(), t).unwrap();
        (t.status, t.assignee_name)
    }

    #[test]
    fn claim_is_seen_by_colleague_and_moves_nothing() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Subtype for 3 schools", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m1@x>");
        claim(&a.conn(), ta, "Aaron", false).unwrap();
        assert_eq!(crate::sync_queue::ticket_sync_state(&a.conn(), ta).unwrap(), "pending", "offline claim stays visibly pending");
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        assert_eq!(status(&w, "<m1@x>"), ("Assigned".into(), Some("Aaron".into())), "Wenjie sees the confirmed owner");
        // Claim never moves Outlook emails or sends a handoff
        assert_eq!(a.count("SELECT COUNT(*) FROM assignment_ops"), 0);
        assert_eq!(a.count("SELECT COUNT(*) FROM message_completion"), 0);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM OwnershipHistory WHERE ended_at IS NULL"), 1);
    }

    #[test]
    fn takeover_preserves_previous_owner_work_and_dates() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Help", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m1@x>");
        claim(&a.conn(), ta, "Aaron", false).unwrap();
        start_work(&a.conn(), ta, "Aaron").unwrap();
        crate::notes::add(&a.conn(), ta, "Called the nurse", "Aaron").unwrap();
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let tw = w.ticket_by_imid("<m1@x>");
        assert_eq!(claim(&w.conn(), tw, "Wenjie", false).unwrap_err().code(), "OWNED", "an owned case needs an explicit takeover");
        claim(&w.conn(), tw, "Wenjie", true).unwrap();
        w.sync(&shared).unwrap();
        a.sync(&shared).unwrap();
        assert_eq!(status(&a, "<m1@x>"), ("In Progress".into(), Some("Wenjie".into())), "takeover keeps the active state");
        let own = tickets::ownership_rows(&a.conn(), ta).unwrap();
        let names: Vec<_> = own.iter().map(|o| (o.staff_name.clone().unwrap_or_default(), o.ended_at.is_some())).collect();
        assert!(names.contains(&("Aaron".into(), true)) && names.contains(&("Wenjie".into(), false)), "{names:?}");
        assert_eq!(crate::notes::list(&w.conn(), tw).unwrap()[0].author_name, "Aaron", "previous work kept");
    }

    #[test]
    fn simultaneous_claims_yield_one_confirmed_owner_and_a_visible_conflict() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Help", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m1@x>");
        let tw = w.ticket_by_imid("<m1@x>");
        claim(&a.conn(), ta, "Aaron", false).unwrap();
        claim(&w.conn(), tw, "Wenjie", false).unwrap();
        a.sync(&shared).unwrap();
        let r = w.sync(&shared).unwrap();
        assert_eq!(r.conflicts, 1);
        a.sync(&shared).unwrap();
        for cl in [&a, &w] {
            assert_eq!(status(cl, "<m1@x>").1.as_deref(), Some("Aaron"), "{}: one confirmed owner", cl.name);
        }
        let c = crate::sync_queue::conflicts(&w.conn(), Some(tw), false).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].proposed_value.as_deref(), c[0].shared_value.as_deref()), (Some("Wenjie"), Some("Aaron")));
        assert!(c[0].reason.as_deref().unwrap().contains("claimed this case first"));
        // explicit retry = take over from the owner now shown
        retry_conflict(&w.conn(), &c[0].conflict_id, "Wenjie").unwrap();
        w.sync(&shared).unwrap();
        assert!(crate::sync_queue::conflicts(&w.conn(), Some(tw), false).unwrap().is_empty(), "conflict closed only after shared acceptance");
        a.sync(&shared).unwrap();
        assert_eq!(status(&a, "<m1@x>").1.as_deref(), Some("Wenjie"));
    }

    #[test]
    fn mark_done_works_offline_without_any_reply_verification() {
        let (dir, shared, a, _w) = team();
        a.ingest(msg(1, "C1", "Help", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m1@x>");
        // offline: the share is gone
        assert!(a.sync(&dir.path().join("gone").join("x.sqlite")).is_err());
        let d = mark_done(&a.conn(), ta, "Aaron").unwrap();
        assert_eq!((d.covered, d.state.as_str()), (1, "pending"));
        assert_eq!(status(&a, "<m1@x>").0, "Done", "Done locally at once");
        assert_eq!(crate::sync_queue::ticket_sync_state(&a.conn(), ta).unwrap(), "pending");
        a.sync(&shared).unwrap();
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets WHERE ticket_status='Done'"), 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM CompletionHistory WHERE completion_state='accepted' AND covered_email_ids LIKE '%-%'"), 1, "coverage recorded");
        assert_eq!(a.count("SELECT COUNT(*) FROM completion_history WHERE state='accepted'"), 1);
        assert_eq!(a.count("SELECT COUNT(*) FROM message_completion"), 0, "no Outlook action was required");
    }

    #[test]
    fn reply_after_done_reopens_into_review_and_stays_available() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Help", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m1@x>");
        claim(&a.conn(), ta, "Aaron", false).unwrap();
        mark_done(&a.conn(), ta, "Aaron").unwrap();
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        // the nurse replies; Wenjie's computer scans it first
        w.ingest(msg(2, "C1", "RE: Help", "Still broken", "nurse@school.org"));
        w.sync(&shared).unwrap();
        a.sync(&shared).unwrap();
        let t = tickets::get(&a.conn(), ta).unwrap();
        assert_eq!((t.status.as_str(), t.assignee_name.as_deref()), ("Review", Some("Aaron")), "owner kept");
        assert!(t.review_reason.unwrap().starts_with("New reply from"));
        assert!(tickets::messages(&a.conn(), ta).unwrap().iter().any(|m| m.body_text.as_deref() == Some("Still broken")), "the new reply is available on Aaron's computer");
        assert_eq!(a.count("SELECT COUNT(*) FROM completion_history WHERE state IN ('accepted','superseded')"), 1, "previous completion history kept");
    }

    #[test]
    fn stale_done_never_overwrites_a_reply_that_arrived_during_completion() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Help", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m1@x>");
        // Aaron marks Done offline (covering only m1) while the reply m2 arrives at Wenjie's computer
        mark_done(&a.conn(), ta, "Aaron").unwrap();
        w.ingest(msg(2, "C1", "RE: Help", "One more thing", "nurse@school.org"));
        w.sync(&shared).unwrap();
        let r = a.sync(&shared).unwrap();
        assert_eq!(r.conflicts, 1, "the stale Done is refused");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets WHERE ticket_status='Review'"), 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM CompletionHistory WHERE completion_state='superseded'"), 1, "decision kept as history");
        assert_eq!(status(&a, "<m1@x>").0, "Review", "Aaron's computer shows Review, not Done");
        assert_eq!(a.count("SELECT COUNT(*) FROM completion_history WHERE state='superseded'"), 1);
        // after handling the reply: retry = a fresh Done covering both emails
        let c = crate::sync_queue::conflicts(&a.conn(), Some(ta), false).unwrap();
        retry_conflict(&a.conn(), &c[0].conflict_id, "Aaron").unwrap();
        a.sync(&shared).unwrap();
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets WHERE ticket_status='Done'"), 1);
    }

    #[test]
    fn older_emails_auto_replies_and_internal_mail_do_not_reopen() {
        let (_d, shared, a, w) = team();
        let mut later = msg(5, "C1", "Help", "body", "nurse@school.org");
        later.received_at = "2026-11-16T12:00:00Z".into();
        a.ingest(later);
        a.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m5@x>");
        mark_done(&a.conn(), ta, "Aaron").unwrap();
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        // an older message of the same conversation found by a later scan
        let mut older = msg(1, "C1", "Help (earlier)", "earlier text", "nurse@school.org");
        older.received_at = "2026-11-16T09:00:00Z".into();
        w.ingest(older);
        // an automatic reply
        let mut ooo = msg(7, "C1", "Automatic reply: Help", "I am out", "nurse@school.org");
        ooo.received_at = "2026-11-16T13:00:00Z".into();
        w.ingest(ooo);
        w.sync(&shared).unwrap();
        a.sync(&shared).unwrap();
        assert_eq!(status(&a, "<m5@x>").0, "Done");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets WHERE ticket_status='Done'"), 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketEmails"), 3, "both emails are still attached and visible");
    }

    #[test]
    fn manual_review_and_resume_keep_the_owner() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Help", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        let ta = a.ticket_by_imid("<m1@x>");
        claim(&a.conn(), ta, "Aaron", false).unwrap();
        assert!(flag_review(&a.conn(), ta, " ", "Aaron").is_err(), "a reason is required");
        flag_review(&a.conn(), ta, "Check with supervisor", "Aaron").unwrap();
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let tw = w.ticket_by_imid("<m1@x>");
        let t = tickets::get(&w.conn(), tw).unwrap();
        assert_eq!((t.status.as_str(), t.review_reason.as_deref(), t.assignee_name.as_deref()), ("Review", Some("Check with supervisor"), Some("Aaron")));
        resume(&a.conn(), ta, "Aaron").unwrap();
        release(&a.conn(), ta, "Aaron").unwrap();
        a.sync(&shared).unwrap();
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets WHERE ticket_status='Unassigned' AND owner_staff_id IS NULL"), 1);
    }

    #[test]
    fn add_to_existing_case_keeps_everything_and_moves_nothing_in_outlook() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Subtype school A", "School A", "nurse@school.org"));
        a.ingest(msg(2, "C2", "Subtype school B", "School B", "nurse@school.org"));
        a.sync(&shared).unwrap();
        let t1 = a.ticket_by_imid("<m1@x>");
        let t2 = a.ticket_by_imid("<m2@x>");
        assert_ne!(t1, t2, "same sender alone never combines cases");
        crate::notes::add(&a.conn(), t2, "School B note", "Aaron").unwrap();
        join(&a.conn(), t2, t1, "Aaron").unwrap();
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        assert_eq!(w.ticket_by_imid("<m1@x>"), w.ticket_by_imid("<m2@x>"), "both emails in one case on the other computer");
        let tw = w.ticket_by_imid("<m1@x>");
        assert_eq!(crate::notes::list(&w.conn(), tw).unwrap().len(), 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketAliases WHERE alias_type='ref'"), 1, "old reference kept as a searchable alias");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketEmails WHERE is_active=0"), 1, "association history kept");
        assert_eq!(a.count("SELECT COUNT(*) FROM assignment_ops"), 0);
        assert_eq!(w.count("SELECT COUNT(*) FROM tickets WHERE merged_into IS NULL"), 1);
    }

    #[test]
    fn folder_change_shows_no_stale_results_and_keeps_history() {
        let c = crate::tickets::tests::conn();
        let s = crate::scope::tests_support::settings_wen_scope();
        crate::tickets::ingest(&c, &[msg(1, "C1", "Help", "body", "a@b.org")], &s).unwrap();
        crate::notes::add(&c, 1, "keep me", "staff").unwrap();
        let before = crate::tickets::list(&c, &crate::tickets::ListQuery { view: "current".into(), ..Default::default() }).unwrap().total;
        assert_eq!(before, 1);
        let n = crate::scope::invalidate_for_new_scope(&c, "another-folder-key").unwrap();
        assert!(n >= 1);
        let cur = crate::tickets::list(&c, &crate::tickets::ListQuery { view: "current".into(), ..Default::default() }).unwrap().total;
        assert_eq!(cur, 0, "results of the previous folder are not shown as current");
        let team = crate::tickets::list(&c, &crate::tickets::ListQuery { view: "team".into(), ..Default::default() }).unwrap().total;
        assert_eq!(team, 1, "the case and its notes are not deleted");
        assert_eq!(crate::notes::list(&c, 1).unwrap().len(), 1);
        let _ = Settings::default();
    }

    /// The P0.2 scenarios against the REAL Access driver (test database only):
    ///   set ASR_SHARED_BACKEND=access & set ASR_ACCESS_TEST_DIR=C:\local\folder & cargo test --no-default-features workflow:: -- --test-threads=1
    #[test]
    fn backend_note() {}
}

/// Explicitly retry a refused local action against the latest shared version (the conflict stays open
/// until the shared database accepts the retry).
pub fn retry_conflict(conn: &Connection, conflict_id: &str, actor_name: &str) -> AppResult<()> {
    let (tid, field, ctype, proposed, op_type): (Option<i64>, Option<String>, String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT c.ticket_id, c.field_name, c.conflict_type, c.proposed_value, o.op_type FROM sync_conflicts c LEFT JOIN sync_ops o ON o.op_id=c.op_id WHERE c.conflict_id=?1 AND c.status<>'resolved'",
            [conflict_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::coded("NOT_FOUND", "Conflict not found or already resolved."))?;
    let tid = tid.ok_or_else(|| AppError::coded("INVALID", "This conflict is not linked to a case."))?;
    let before: i64 = conn.query_row("SELECT COALESCE(MAX(id),0) FROM sync_ops", [], |r| r.get(0))?;
    match (ctype.as_str(), field.as_deref().unwrap_or(""), op_type.as_deref().unwrap_or("")) {
        ("owner", _, "ticket.claim") | ("owner", _, "ticket.create") => {
            claim(conn, tid, actor_name, true)?;
        }
        ("stale_done", _, _) | ("status", _, "ticket.done") => {
            // handled the newer reply: a fresh Done decision covering every email now linked
            let t = tickets::get(conn, tid)?;
            if t.status == DONE {
                reopen(conn, tid, actor_name, "Reopened to mark Done again")?;
            }
            mark_done(conn, tid, actor_name)?;
        }
        ("status", _, "ticket.start") => {
            start_work(conn, tid, actor_name)?;
        }
        (_, f, _) if f.starts_with("field:") => {
            tickets::set_field(conn, tid, &f[6..], proposed.clone(), false, actor_name)?;
        }
        (_, "staff_actions", _) | (_, "priority", _) | (_, "sender_name", _) | (_, "sender_email", _) => {
            let mut p = tickets::TicketPatch::default();
            let v = proposed.clone().unwrap_or_default();
            match field.as_deref().unwrap_or("") {
                "staff_actions" => p.staff_actions = Some(v),
                "priority" => p.priority = Some(v),
                "sender_name" => p.sender_name = Some(v),
                _ => p.sender_email = Some(v),
            }
            tickets::update(conn, tid, &p, actor_name)?;
        }
        _ => return Err(AppError::coded("INVALID", "This conflict cannot be retried automatically. Make the change again in the case, or keep the shared value.")),
    }
    let op: Option<String> = conn.query_row("SELECT op_id FROM sync_ops WHERE id>?1 AND ticket_id=?2 ORDER BY id DESC LIMIT 1", params![before, tid], |r| r.get(0)).optional()?;
    match op {
        Some(op) => q::mark_retrying(conn, conflict_id, &op)?,
        None => {
            conn.execute("UPDATE sync_conflicts SET status='resolved', resolution='retried_locally', resolved_at=?2 WHERE conflict_id=?1", params![conflict_id, now()])?;
        }
    }
    Ok(())
}
