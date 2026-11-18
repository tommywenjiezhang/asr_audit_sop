//! Shared workflow rules: Claim / Take over / Release, Start Work, Review, Mark Done and reopening when a
//! new reply arrives. Applied inside the shared transaction of each operation (see `shared_apply`).
//!
//! Statuses: Unassigned (no owner) | Assigned (owner accepted, work not started) | In Progress |
//! Review (a Done case got a new substantive reply, or staff flagged it) | Done (staff clicked Mark Done).
//! Synchronization conflicts and pending Outlook work are NOT statuses; they are separate badges.
//!
//! Rules enforced here:
//! - Claim changes ownership only (no Outlook move, no handoff). A case owned by someone else needs an
//!   explicit Take over, and only from the owner the user actually saw; anything else is a conflict.
//! - Mark Done needs no sent-reply verification. A Done decision records the incoming emails it covered;
//!   a stale Done (a newer substantive reply exists that it did not cover) never overwrites the reopening.
//! - A new substantive incoming reply to a Done case moves it to Review (owner kept, reason recorded).
//!   Older emails found later, duplicates, internal emails and automatic replies do not reopen.

use crate::shared_apply::{self as sa, EmailRec, OpResult, TicketCore, WorkflowTx};
use crate::shared_store::{col, col_s, SResult, Val};
use serde_json::Value;

pub const UNASSIGNED: &str = "Unassigned";
pub const ASSIGNED: &str = "Assigned";
pub const IN_PROGRESS: &str = "In Progress";
pub const REVIEW: &str = "Review";
pub const DONE: &str = "Done";

/// P0.2: workflow status and owner are managed through the shared database.
pub const WORKFLOW_SYNCED: bool = true;

/// Local ticket keys changed by a workflow operation type (protected from being overwritten by a pull).
pub fn touched_by(op_type: &str) -> &'static [&'static str] {
    match op_type {
        "ticket.claim" | "ticket.release" => &["owner", "status"],
        "ticket.start" | "ticket.flag_review" | "ticket.resume" | "ticket.done" | "ticket.reopen" => &["status"],
        "ticket.join" => &["*"],
        _ => &[],
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(String::from).filter(|x| !x.is_empty())
}

fn staff_name(tx: &mut WorkflowTx<'_, '_>, id: &Option<String>) -> String {
    match id {
        Some(i) => tx.query("SELECT display_name FROM Staff WHERE staff_id = ?", &[i.clone().into()]).ok().and_then(|r| r.first().map(|r| col_s(r, 0))).unwrap_or_else(|| "another staff member".into()),
        None => "nobody".into(),
    }
}

fn set_status(tx: &mut WorkflowTx<'_, '_>, tid: &str, cur: &TicketCore, status: &str, review_reason: Option<String>, done_at: Option<String>) -> SResult<()> {
    tx.exec(
        "UPDATE Tickets SET ticket_status = ?, review_reason = ?, done_at = ? WHERE ticket_id = ?",
        &[status.into(), review_reason.map(|r| Val::Text(sa::cut(&r, 255))).unwrap_or(Val::Null), done_at.map(Val::Text).unwrap_or(Val::Null), tid.into()],
    )?;
    if cur.status != status {
        tx.act(tid, "Status changed", Some("status"), Some(cur.status.clone()), Some(status.into()));
    }
    tx.touch(tid);
    Ok(())
}

fn change_owner(tx: &mut WorkflowTx<'_, '_>, tid: &str, cur: &TicketCore, new_owner: Option<String>, reason: &str) -> SResult<()> {
    let now = tx.now();
    let who = tx.op().staff_name.clone();
    // close the open ownership period (history keeps every earlier owner and their dates)
    tx.exec(
        "UPDATE OwnershipHistory SET ended_at = ?, ended_by = ?, end_reason = ? WHERE ticket_id = ? AND ended_at IS NULL",
        &[now.clone().into(), sa::cut(&who, 100).into(), reason.into(), tid.into()],
    )?;
    if let Some(o) = &new_owner {
        tx.exec(
            "INSERT INTO OwnershipHistory (entry_id, ticket_id, staff_id, started_at, started_by, start_reason, op_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &[sa::new_id().into(), tid.into(), o.clone().into(), now.into(), sa::cut(&who, 100).into(), reason.into(), tx.op().op_id.clone().into()],
        )?;
    }
    tx.exec("UPDATE Tickets SET owner_staff_id = ? WHERE ticket_id = ?", &[new_owner.clone().map(Val::Text).unwrap_or(Val::Null), tid.into()])?;
    let old_name = staff_name(tx, &cur.owner_staff_id);
    let new_name = staff_name(tx, &new_owner);
    tx.act(
        tid,
        match reason {
            "claim" => "Claimed",
            "takeover" => "Taken over",
            "released" => "Ownership cleared",
            _ => "Owner changed",
        },
        Some("owner"),
        Some(old_name),
        Some(new_name),
    );
    tx.touch(tid);
    Ok(())
}

/// Latest accepted completion: (covered email ids, latest covered received time).
fn latest_completion(tx: &mut WorkflowTx<'_, '_>, tid: &str) -> SResult<Option<(Vec<String>, Option<String>)>> {
    let rows = tx.query("SELECT covered_email_ids, max_covered_received_at, decided_at, completion_state FROM CompletionHistory WHERE ticket_id = ? ORDER BY decided_at DESC", &[tid.into()])?;
    Ok(rows
        .iter()
        .find(|r| matches!(col_s(r, 3).as_str(), "accepted" | "legacy"))
        .map(|r| (serde_json::from_str(&col_s(r, 0)).unwrap_or_default(), col(r, 1).filter(|x| !x.is_empty()))))
}

/// Substantive incoming emails of the case (not internal, not automatic replies): (email id, received, sender).
fn incoming(tx: &mut WorkflowTx<'_, '_>, tid: &str) -> SResult<Vec<(String, String, String)>> {
    Ok(tx
        .query(
            "SELECT em.email_id, em.received_at, em.sender_name, em.sender_email FROM TicketEmails te INNER JOIN Emails em ON em.email_id = te.email_id WHERE te.ticket_id = ? AND te.is_active = 1 AND em.is_internal = 0 AND em.is_auto_reply = 0",
            &[tid.into()],
        )?
        .iter()
        .map(|r| (col_s(r, 0), col_s(r, 1), col(r, 2).filter(|x| !x.is_empty()).unwrap_or_else(|| col_s(r, 3))))
        .collect())
}

/// Is this email a reply that arrived after the given completion and is not covered by it?
/// An email received before the latest covered email (an older message found later) is not "new".
pub fn is_new_after(email_id: &str, received: &str, covered: &[String], max_covered: &Option<String>, done_at: &Option<String>) -> bool {
    if covered.iter().any(|c| c == email_id) {
        return false;
    }
    match (max_covered, done_at) {
        (Some(m), _) => received > m.as_str(),
        (None, Some(d)) => received > d.as_str(),
        (None, None) => true,
    }
}

pub fn apply_workflow(tx: &mut WorkflowTx<'_, '_>, op_type: &str, tid: &str, res: &mut OpResult) -> SResult<bool> {
    if !matches!(op_type, "ticket.claim" | "ticket.release" | "ticket.start" | "ticket.flag_review" | "ticket.resume" | "ticket.reopen" | "ticket.done" | "ticket.join") {
        return Ok(false);
    }
    let cur = tx.core(tid)?;
    let op = tx.op().clone();
    match op_type {
        "ticket.claim" => {
            let Some(want) = s(&op.proposed, "owner_staff_id").or(op.staff_id.clone()) else {
                res.result = "rejected".into();
                res.message = "Choose who you are (Settings › Staff Assignment › I am) before claiming.".into();
                return Ok(true);
            };
            let takeover = op.proposed.get("takeover").and_then(|v| v.as_bool()).unwrap_or(false);
            let saw = s(&op.original, "owner_staff_id");
            match cur.owner_staff_id.clone() {
                Some(o) if o == want => {} // already the owner: nothing to do
                None => {
                    change_owner(tx, tid, &cur, Some(want), "claim")?;
                    if cur.status == UNASSIGNED {
                        set_status(tx, tid, &cur, ASSIGNED, None, None)?;
                    }
                }
                Some(o) if takeover && saw.as_deref() == Some(o.as_str()) => {
                    change_owner(tx, tid, &cur, Some(want), "takeover")?;
                }
                Some(o) => {
                    let name = staff_name(tx, &Some(o.clone()));
                    let reason = if saw.is_none() {
                        format!("{name} claimed this case first. Your claim was not applied; use Take over if you should own it.")
                    } else {
                        format!("The owner changed to {name} after you looked. Nothing was changed; review before taking over.")
                    };
                    let saw_name = saw.map(|x| staff_name(tx, &Some(x)));
                    let want_name = staff_name(tx, &Some(want));
                    tx.conflict("owner", "owner", saw_name, Some(want_name), Some(name), &reason);
                }
            }
        }
        "ticket.release" => {
            let saw = s(&op.original, "owner_staff_id");
            if cur.owner_staff_id.is_none() {
                return Ok(true);
            }
            if saw != cur.owner_staff_id {
                let name = staff_name(tx, &cur.owner_staff_id);
                let saw_name = saw.map(|x| staff_name(tx, &Some(x)));
                tx.conflict("owner", "owner", saw_name, Some("nobody".into()), Some(name), "The owner changed after you looked; ownership was not cleared.");
                return Ok(true);
            }
            change_owner(tx, tid, &cur, None, "released")?;
            if cur.status == ASSIGNED || cur.status == IN_PROGRESS {
                set_status(tx, tid, &cur, UNASSIGNED, None, None)?;
            }
        }
        "ticket.start" => {
            if cur.status == IN_PROGRESS {
                return Ok(true);
            }
            if cur.owner_staff_id.is_none() {
                tx.conflict("status", "status", Some(cur.status.clone()), Some(IN_PROGRESS.into()), Some(cur.status.clone()), "The case has no owner in the shared database. Claim it first.");
            } else if cur.status == DONE {
                tx.conflict("status", "status", None, Some(IN_PROGRESS.into()), Some(DONE.into()), "The case was marked Done by someone else. Reopen it if work continues.");
            } else {
                set_status(tx, tid, &cur, IN_PROGRESS, None, None)?;
            }
        }
        "ticket.flag_review" => {
            let reason = s(&op.proposed, "reason").unwrap_or_else(|| "Flagged for review".into());
            if cur.status == REVIEW && cur.review_reason.as_deref() == Some(reason.as_str()) {
                return Ok(true);
            }
            set_status(tx, tid, &cur, REVIEW, Some(reason.clone()), cur.done_at.clone())?;
            tx.act(tid, "Flagged for review", Some("review_reason"), cur.review_reason.clone(), Some(reason));
        }
        "ticket.resume" => {
            if cur.status != REVIEW {
                return Ok(true);
            }
            let next = if cur.owner_staff_id.is_some() { IN_PROGRESS } else { UNASSIGNED };
            set_status(tx, tid, &cur, next, None, None)?;
        }
        "ticket.reopen" => {
            if cur.status != DONE {
                return Ok(true);
            }
            let next = if cur.owner_staff_id.is_some() { IN_PROGRESS } else { UNASSIGNED };
            set_status(tx, tid, &cur, next, None, None)?;
            tx.act(tid, "Reopened", None, None, s(&op.proposed, "reason"));
        }
        "ticket.done" => done(tx, tid, &cur, res)?,
        "ticket.join" => join(tx, tid, &cur, res)?,
        _ => return Ok(false),
    }
    Ok(true)
}

/// Mark Done: accepted unless a newer substantive reply exists that the decision did not cover (then the
/// case goes/stays in Review and the stale decision is kept as superseded history).
fn done(tx: &mut WorkflowTx<'_, '_>, tid: &str, cur: &TicketCore, res: &mut OpResult) -> SResult<()> {
    let op = tx.op().clone();
    let p = &op.proposed;
    let completion_id = s(p, "completion_id").unwrap_or_else(sa::new_id);
    let decided_at = s(p, "decided_at").unwrap_or_else(|| tx.now());
    let max_covered = s(p, "max_covered_received_at");
    // covered emails: shared ids when the client knew them, otherwise "imid:<Internet Message-ID>"
    let mailbox = {
        let r = tx.query("SELECT setting_value FROM SharedSettings WHERE setting_key = ?", &["mailbox_logical_key".into()])?;
        r.first().map(|r| col_s(r, 0)).unwrap_or_default()
    };
    let mut covered: Vec<String> = vec![];
    for c in p.get("covered").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let Some(c) = c.as_str() else { continue };
        if let Some(id) = c.strip_prefix("imid:") {
            let key = sa::message_match_key(&mailbox, id);
            if let Some(r) = tx.query("SELECT email_id FROM Emails WHERE match_key = ?", &[key.into()])?.first() {
                covered.push(col_s(r, 0));
            }
        } else {
            covered.push(c.to_string());
        }
    }
    let newer: Vec<(String, String, String)> = incoming(tx, tid)?.into_iter().filter(|(id, recv, _)| is_new_after(id, recv, &covered, &max_covered, &Some(decided_at.clone()))).collect();
    let now = tx.now();
    let record = |tx: &mut WorkflowTx<'_, '_>, state: &str, reason: Option<String>| -> SResult<()> {
        if tx.query("SELECT completion_id FROM CompletionHistory WHERE completion_id = ?", &[completion_id.clone().into()])?.is_empty() {
            tx.exec(
                "INSERT INTO CompletionHistory (completion_id, ticket_id, actor_staff_id, actor_name, decided_at, accepted_at, covered_email_ids, max_covered_received_at, op_id, completion_state, state_reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &[
                    completion_id.clone().into(), tid.into(), op.staff_id.clone().map(Val::Text).unwrap_or(Val::Null), sa::cut(&op.staff_name, 100).into(), sa::cut(&decided_at, 30).into(),
                    if state == "accepted" { Val::Text(now.clone()) } else { Val::Null }, serde_json::to_string(&covered).unwrap_or_default().into(),
                    max_covered.clone().map(Val::Text).unwrap_or(Val::Null), op.op_id.clone().into(), state.into(), reason.map(|r| Val::Text(sa::cut(&r, 255))).unwrap_or(Val::Null),
                ],
            )?;
        }
        Ok(())
    };
    if !newer.is_empty() {
        let (_, recv, who) = newer.iter().max_by(|a, b| a.1.cmp(&b.1)).cloned().unwrap();
        let reason = format!("New reply from {who} ({recv}) arrived after this Done decision");
        record(tx, "superseded", Some(reason.clone()))?;
        if cur.status != REVIEW {
            set_status(tx, tid, cur, REVIEW, Some(format!("New reply from {who}")), cur.done_at.clone())?;
        }
        tx.conflict("status", "stale_done", Some(cur.status.clone()), Some(DONE.into()), Some(REVIEW.into()), &format!("{reason}. The case stays in Review so the reply is not missed; mark Done again after handling it."));
        tx.touch(tid);
        res.message = "A newer reply exists; the case is in Review.".into();
        return Ok(());
    }
    if cur.status == DONE {
        // already Done (e.g. a colleague completed it): keep this decision as history too
        record(tx, "accepted", Some("Case was already Done".into()))?;
        tx.touch(tid);
        return Ok(());
    }
    record(tx, "accepted", None)?;
    set_status(tx, tid, cur, DONE, None, Some(now))?;
    tx.act(tid, "Marked Done", None, None, Some(format!("{} incoming email(s) covered", covered.len())));
    Ok(())
}

/// Add to Existing Case: this case (source) joins `target`. Emails, notes and issue types move to the
/// target; the source stays as a redirect (alias) and is never deleted. No Outlook side effects.
fn join(tx: &mut WorkflowTx<'_, '_>, src: &str, cur: &TicketCore, res: &mut OpResult) -> SResult<()> {
    let op = tx.op().clone();
    let Some(target_in) = s(&op.proposed, "target_ticket_id") else {
        res.result = "rejected".into();
        res.message = "No target case.".into();
        return Ok(());
    };
    let mut target = target_in;
    for _ in 0..8 {
        let r = tx.query("SELECT merged_into FROM Tickets WHERE ticket_id = ?", &[target.clone().into()])?;
        match r.first().and_then(|r| col(r, 0)).filter(|x| !x.is_empty()) {
            Some(next) => target = next,
            None => break,
        }
    }
    if target == src {
        return Ok(());
    }
    let tgt = tx.core(&target)?;
    if let (Some(a), Some(b)) = (&cur.owner_staff_id, &tgt.owner_staff_id) {
        if a != b {
            let an = staff_name(tx, &Some(a.clone()));
            let bn = staff_name(tx, &Some(b.clone()));
            tx.conflict("owner", "owner", None, Some(an), Some(bn), "The two cases have different owners. Agree on one owner first (Take over), then add the case.");
            return Ok(());
        }
    }
    let now = tx.now();
    for r in tx.query("SELECT email_id, link_type FROM TicketEmails WHERE ticket_id = ? AND is_active = 1", &[src.into()])? {
        let eid = col_s(&r, 0);
        if tx.query("SELECT email_id FROM TicketEmails WHERE ticket_id = ? AND email_id = ?", &[target.clone().into(), eid.clone().into()])?.is_empty() {
            tx.exec(
                "INSERT INTO TicketEmails (ticket_id, email_id, link_type, is_active, linked_at, linked_by_device) VALUES (?, ?, ?, 1, ?, ?)",
                &[target.clone().into(), eid.clone().into(), col_s(&r, 1).into(), now.clone().into(), op.device_id.clone().into()],
            )?;
        } else {
            tx.exec("UPDATE TicketEmails SET is_active = 1 WHERE ticket_id = ? AND email_id = ?", &[target.clone().into(), eid.clone().into()])?;
        }
        // the old association stays, inactive (association history)
        tx.exec("UPDATE TicketEmails SET is_active = 0 WHERE ticket_id = ? AND email_id = ?", &[src.into(), eid.into()])?;
    }
    tx.exec("UPDATE TicketNotes SET ticket_id = ? WHERE ticket_id = ?", &[target.clone().into(), src.into()])?;
    for r in tx.query("SELECT issue_type, origin FROM TicketIssueTypes WHERE ticket_id = ?", &[src.into()])? {
        if tx.query("SELECT issue_type FROM TicketIssueTypes WHERE ticket_id = ? AND issue_type = ?", &[target.clone().into(), col_s(&r, 0).into()])?.is_empty() {
            tx.exec("INSERT INTO TicketIssueTypes (ticket_id, issue_type, sort_order, origin) VALUES (?, ?, 200, ?)", &[target.clone().into(), col_s(&r, 0).into(), col_s(&r, 1).into()])?;
        }
    }
    tx.exec("UPDATE Tickets SET merged_into = ? WHERE ticket_id = ?", &[target.clone().into(), src.into()])?;
    if tx.query("SELECT alias_key FROM TicketAliases WHERE alias_key = ?", &[cur.ticket_ref.clone().into()])?.is_empty() {
        tx.exec("INSERT INTO TicketAliases (alias_key, ticket_id, alias_type, created_at) VALUES (?, ?, 'ref', ?)", &[sa::cut(&cur.ticket_ref, 64).into(), target.clone().into(), now.into()])?;
    }
    // an open case added to a Done case must not be lost: the result goes to Review
    if tgt.status == DONE && cur.status != DONE {
        set_status(tx, &target, &tgt, REVIEW, Some(format!("Case {} was added to this Done case", cur.ticket_ref)), tgt.done_at.clone())?;
    }
    tx.act(&target, "Case added to this case", None, Some(cur.ticket_ref.clone()), Some(tgt.ticket_ref.clone()));
    tx.touch(&target);
    tx.touch(src);
    res.ticket_id = Some(target);
    res.redirected_from = Some(src.to_string());
    Ok(())
}

/// Called after an email was attached to a case (same transaction): a new substantive reply moves a Done
/// case to Review. Older emails found later, internal emails and automatic replies do not.
pub fn after_email_attached(tx: &mut WorkflowTx<'_, '_>, tid: &str, email_id: &str, e: &EmailRec) -> SResult<()> {
    if e.is_internal || e.is_auto_reply || e.link_type == "internal" {
        return Ok(());
    }
    let cur = tx.core(tid)?;
    if cur.status != DONE {
        return Ok(());
    }
    let (covered, max) = latest_completion(tx, tid)?.unwrap_or_default();
    if is_new_after(email_id, &e.received_at, &covered, &max, &cur.done_at) {
        let who = if e.sender_name.trim().is_empty() { e.sender_email.clone() } else { e.sender_name.clone() };
        set_status(tx, tid, &cur, REVIEW, Some(format!("New reply from {who}")), cur.done_at.clone())?;
        tx.act(tid, "Reopened for review: new reply", None, None, Some(who));
    } else {
        tx.act(tid, "Earlier email added; the case stays Done", None, None, Some(sa::cut(&e.subject, 200)));
    }
    Ok(())
}

/// Mirror the shared workflow status/owner into the local ticket.
pub fn mirror_status(conn: &rusqlite::Connection, tid: i64, c: &TicketCore, owner_local: Option<i64>) -> crate::error::AppResult<()> {
    use rusqlite::params;
    conn.execute(
        "UPDATE tickets SET status=?2, assignee_staff_id=?3, review_reason=?4, completed_at=?5,
            assigned_at=CASE WHEN COALESCE(assignee_staff_id,-1)<>COALESCE(?3,-1) THEN ?6 ELSE assigned_at END
         WHERE id=?1",
        params![tid, c.status, owner_local, c.review_reason, c.done_at, crate::db::now()],
    )?;
    if c.merged_into.as_deref().map(|m| !m.is_empty()).unwrap_or(false) {
        // joined into another case: keep the local row as a hidden redirect once the target is mirrored
        if let Ok(Some(target)) = conn.query_row("SELECT id FROM tickets WHERE shared_id=?1", [c.merged_into.clone().unwrap()], |r| r.get::<_, i64>(0)).map(Some).or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        }) {
            if target != tid {
                crate::sync_queue::merge_local(conn, tid, target)?;
            }
        }
    }
    Ok(())
}
