//! Local side of the shared database: offline operation queue, identity binding, mirroring of shared
//! cases into the local SQLite cache, and local conflict records.
//!
//! Rules:
//! - Every local edit and its queued operation are written in ONE SQLite transaction (callers pass their
//!   open transaction to `enqueue`).
//! - Operations are bound to a confirmed shared database identity (`target_db_id`). Operations created
//!   before an identity was confirmed are `held` and need an explicit review before they are submitted.
//!   Changing the database path or identity never replays operations into a different database:
//!   operations of another identity become `isolated`.
//! - Standalone mode keeps everything local and queues nothing; existing local cases reach a shared
//!   database only through the explicit import (`queue_import`).
//! - Pulling shared state never overwrites unsent local work: values touched by held/pending operations
//!   keep their local (provisional) value until the operation is settled.

use crate::db::{get_setting, now, set_setting};
use crate::error::{AppError, AppResult};
use crate::shared_apply::{self as sa, CompletionRec, EmailRec, FieldRec, Op, OpResult, TicketBundle, TicketCore, TicketSnapshot};
use crate::shared_store::DeviceInfo;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;

pub const MODE_SHARED: &str = "shared";
pub const MODE_STANDALONE: &str = "standalone";

// ------------------------------------------------------------------ settings mirrored into SQLite

/// "shared" (default: work locally, sync when the configured database is reachable) | "standalone".
pub fn mode(conn: &Connection) -> String {
    get_setting(conn, "shared_mode").ok().flatten().filter(|m| m == MODE_STANDALONE || m == MODE_SHARED).unwrap_or_else(|| MODE_SHARED.into())
}

pub fn set_mode(conn: &Connection, m: &str) -> AppResult<()> {
    if m != MODE_SHARED && m != MODE_STANDALONE {
        return Err(AppError::coded("INVALID", "Unknown shared database mode"));
    }
    set_setting(conn, "shared_mode", m)
}

/// Identity of the shared database this computer's queue is bound to.
pub fn bound_db(conn: &Connection) -> Option<String> {
    get_setting(conn, "shared_db_id").ok().flatten().filter(|s| !s.is_empty())
}

pub fn device_id(conn: &Connection) -> AppResult<String> {
    if let Some(d) = get_setting(conn, "device_id")?.filter(|d| !d.is_empty()) {
        return Ok(d);
    }
    let d = sa::new_id();
    set_setting(conn, "device_id", &d)?;
    Ok(d)
}

pub fn windows_account() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let dom = std::env::var("USERDOMAIN").unwrap_or_default();
    if dom.is_empty() { user } else { format!("{dom}\\{user}") }
}

pub fn device_info(conn: &Connection, app_version: &str) -> AppResult<DeviceInfo> {
    Ok(DeviceInfo {
        device_id: device_id(conn)?,
        device_name: std::env::var("COMPUTERNAME").unwrap_or_default(),
        windows_account: windows_account(),
        app_version: app_version.into(),
    })
}

/// Who is acting on this computer: the selected staff member (stable shared id) or the settings name.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Actor {
    pub staff_id: Option<i64>,
    pub staff_shared_id: Option<String>,
    pub name: String,
}

pub fn actor(conn: &Connection, fallback_name: &str) -> Actor {
    let sid = crate::staff::current_staff_id(conn).ok().flatten();
    if let Some(id) = sid {
        if let Ok(Some((name, shared))) = conn
            .query_row("SELECT display_name, shared_id FROM staff WHERE id=?1", [id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
            .optional()
        {
            return Actor { staff_id: Some(id), staff_shared_id: shared, name };
        }
    }
    Actor { staff_id: None, staff_shared_id: None, name: if fallback_name.trim().is_empty() { "staff".into() } else { fallback_name.trim().into() } }
}

// ------------------------------------------------------------------ v4 backfill (repeatable)

pub fn backfill_v4(conn: &Connection) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    let missing: Vec<i64> = {
        let mut st = tx.prepare("SELECT id FROM tickets WHERE shared_id IS NULL")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for id in missing {
        tx.execute("UPDATE tickets SET shared_id=?2 WHERE id=?1", params![id, sa::new_id()])?;
    }
    let staff: Vec<i64> = {
        let mut st = tx.prepare("SELECT id FROM staff WHERE shared_id IS NULL")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for id in staff {
        tx.execute("UPDATE staff SET shared_id=?2 WHERE id=?1", params![id, sa::new_id()])?;
    }
    if get_setting(&tx, "v4_history_backfill")?.is_none() {
        // ownership periods from finished assignments, then the current owner
        let ops: Vec<(i64, i64, String, Option<String>)> = {
            let mut st = tx.prepare("SELECT ticket_id, to_staff_id, created_at, finished_at FROM assignment_ops WHERE state='done' ORDER BY ticket_id, id")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
            v
        };
        for (tid, staff_id, created, finished) in ops {
            let at = finished.unwrap_or(created);
            open_ownership(&tx, tid, Some(staff_id), &at, "assignment (earlier version)", "migration", "legacy")?;
        }
        let owners: Vec<(i64, i64, Option<String>, String)> = {
            let mut st = tx.prepare("SELECT id, assignee_staff_id, assigned_at, created_at FROM tickets WHERE assignee_staff_id IS NOT NULL")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
            v
        };
        for (tid, staff_id, assigned_at, created) in owners {
            let cur: Option<i64> = tx
                .query_row("SELECT staff_id FROM ownership_history WHERE ticket_id=?1 AND ended_at IS NULL ORDER BY id DESC LIMIT 1", [tid], |r| r.get(0))
                .optional()?
                .flatten();
            if cur != Some(staff_id) {
                open_ownership(&tx, tid, Some(staff_id), &assigned_at.unwrap_or(created), "owner from Outlook folder (earlier version)", "migration", "legacy")?;
            }
        }
        // Done tickets: one completion decision covering the incoming emails linked at the time
        let done: Vec<(i64, String)> = {
            let mut st = tx.prepare("SELECT id, COALESCE(completed_at, updated_at) FROM tickets WHERE status='Done'")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            v
        };
        for (tid, at) in done {
            let (covered, max_recv) = incoming_coverage(&tx, tid)?;
            tx.execute(
                "INSERT INTO completion_history(completion_id, ticket_id, actor_name, decided_at, covered_json, max_covered_received_at, state, state_reason, origin)
                 VALUES(?1, ?2, NULL, ?3, ?4, ?5, 'legacy', 'Done before the shared workflow (completion verified by Outlook steps in the earlier version)', 'legacy')",
                params![sa::new_id(), tid, at, serde_json::to_string(&covered)?, max_recv],
            )?;
        }
        set_setting(&tx, "v4_history_backfill", &now())?;
    }
    tx.commit()?;
    Ok(())
}

/// Incoming (non-internal) emails of a ticket and the latest received time among them.
pub fn incoming_coverage(conn: &Connection, tid: i64) -> AppResult<(Vec<i64>, Option<String>)> {
    let mut st = conn.prepare(
        "SELECT m.id, m.received_at FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
         WHERE l.ticket_id=?1 AND l.link_type IN ('original','reply','manual') AND m.is_internal=0 ORDER BY m.received_at, m.id",
    )?;
    let rows: Vec<(i64, Option<String>)> = st.query_map([tid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let max = rows.iter().filter_map(|r| r.1.clone()).max();
    Ok((rows.into_iter().map(|r| r.0).collect(), max))
}

/// Close the open ownership period of a ticket and open one for `staff_id` (None = no owner).
pub fn open_ownership(conn: &Connection, tid: i64, staff_id: Option<i64>, at: &str, reason: &str, by: &str, origin: &str) -> AppResult<()> {
    let open: Option<(i64, Option<i64>)> = conn
        .query_row("SELECT id, staff_id FROM ownership_history WHERE ticket_id=?1 AND ended_at IS NULL ORDER BY id DESC LIMIT 1", [tid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    if let Some((_, cur)) = &open {
        if *cur == staff_id {
            return Ok(());
        }
    }
    if let Some((oid, _)) = open {
        conn.execute("UPDATE ownership_history SET ended_at=?2, ended_by=?3, end_reason=?4 WHERE id=?1", params![oid, at, by, reason])?;
    }
    if let Some(sid) = staff_id {
        let (name, shared): (Option<String>, Option<String>) =
            conn.query_row("SELECT display_name, shared_id FROM staff WHERE id=?1", [sid], |r| Ok((r.get(0)?, r.get(1)?))).optional()?.unwrap_or((None, None));
        conn.execute(
            "INSERT INTO ownership_history(entry_id, ticket_id, staff_id, staff_shared_id, staff_name, started_at, started_by, start_reason, origin) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![sa::new_id(), tid, sid, shared, name, at, by, reason, origin],
        )?;
    }
    Ok(())
}

// ------------------------------------------------------------------ queueing

/// Every ticket gets its stable shared id (UUID) when it is created or first queued.
pub fn ensure_shared_id(conn: &Connection, tid: i64) -> AppResult<String> {
    if let Some(s) = conn.query_row("SELECT shared_id FROM tickets WHERE id=?1", [tid], |r| r.get::<_, Option<String>>(0)).optional()?.flatten() {
        return Ok(s);
    }
    let id = sa::new_id();
    conn.execute("UPDATE tickets SET shared_id=?2 WHERE id=?1", params![tid, id])?;
    Ok(id)
}

/// Ticket-level sharing state: "local_only" tickets are carried by the import snapshot, not by edits.
pub fn ticket_shared_state(conn: &Connection, tid: i64) -> AppResult<String> {
    Ok(conn.query_row("SELECT shared_state FROM tickets WHERE id=?1", [tid], |r| r.get(0)).optional()?.unwrap_or_else(|| "local_only".into()))
}

pub struct NewOp {
    pub ticket_id: Option<i64>,
    pub entity_type: &'static str,
    pub entity_id: Option<String>,
    pub op_type: &'static str,
    pub original: Value,
    pub proposed: Value,
}

/// Queue an operation inside the caller's transaction. Returns the op id, or None when nothing is
/// queued (standalone mode, or a ticket that has never been submitted - its later import carries the edit).
pub fn enqueue(conn: &Connection, actor_name: &str, op: NewOp) -> AppResult<Option<String>> {
    if mode(conn) != MODE_SHARED {
        return Ok(None);
    }
    if let Some(tid) = op.ticket_id {
        if ticket_shared_state(conn, tid)? == "local_only" {
            return Ok(None);
        }
        ensure_shared_id(conn, tid)?;
    }
    let a = actor(conn, actor_name);
    let base: i64 = match op.ticket_id {
        Some(t) => conn.query_row("SELECT shared_version FROM tickets WHERE id=?1", [t], |r| r.get(0)).optional()?.unwrap_or(0),
        None => 0,
    };
    let target = bound_db(conn);
    let (state, hold) = match &target {
        Some(_) => ("pending", None),
        None => ("held", Some("before_identity")),
    };
    let op_id = sa::new_id();
    conn.execute(
        "INSERT INTO sync_ops(op_id, ticket_id, entity_type, entity_id, op_type, staff_shared_id, staff_name, device_id, windows_account, base_version, original_json, proposed_json, created_at, state, target_db_id, hold_reason)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![op_id, op.ticket_id, op.entity_type, op.entity_id, op.op_type, a.staff_shared_id, a.name, device_id(conn)?, windows_account(), base,
            serde_json::to_string(&op.original)?, serde_json::to_string(&op.proposed)?, now(), state, target, hold],
    )?;
    Ok(Some(op_id))
}

/// Tag local history rows written in the same transaction with the op (prevents duplicates on pull).
pub fn tag_history(conn: &Connection, tid: i64, op_id: &Option<String>, since_row: i64) -> AppResult<()> {
    if let Some(op) = op_id {
        conn.execute("UPDATE ticket_history SET op_id=?3 WHERE ticket_id=?1 AND id>?2 AND op_id IS NULL", params![tid, since_row, op])?;
    }
    Ok(())
}

pub fn last_history_row(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(id),0) FROM ticket_history", [], |r| r.get(0))?)
}

/// Normalize what Outlook returns to the stored local form "<...>".
pub fn outlook_imid(imid: &str) -> String {
    let n = sa::normalize_message_id(imid);
    if n.is_empty() { String::new() } else { format!("<{n}>") }
}

pub fn is_auto_reply(subject: &str) -> bool {
    let s = subject.trim().to_lowercase();
    ["automatic reply", "auto-reply", "autoreply", "auto reply", "out of office", "undeliverable", "delivery status notification", "delivery has failed", "mail delivery failed"]
        .iter()
        .any(|p| s.starts_with(p))
}

pub fn email_rec(conn: &Connection, message_id: i64, link_type: &str) -> AppResult<EmailRec> {
    conn.query_row(
        "SELECT COALESCE(shared_email_id,''), COALESCE(internet_message_id,''), COALESCE(conversation_id,''), COALESCE(subject,''), COALESCE(sender_name,''), COALESCE(sender_email,''),
                COALESCE(received_at,''), is_internal, COALESCE(body_text,''), body_truncated
         FROM email_messages WHERE id=?1",
        [message_id],
        |r| {
            let subject: String = r.get(3)?;
            Ok(EmailRec {
                email_id: r.get(0)?,
                internet_message_id: r.get(1)?,
                conversation_id: r.get(2)?,
                is_auto_reply: is_auto_reply(&subject),
                subject,
                sender_name: r.get(4)?,
                sender_email: r.get(5)?,
                received_at: r.get(6)?,
                is_internal: r.get::<_, i64>(7)? != 0,
                body_text: r.get(8)?,
                body_truncated: r.get::<_, i64>(9)? != 0,
                link_type: link_type.into(),
                is_active: true,
                ..Default::default()
            })
        },
    )
    .map_err(Into::into)
}

/// The whole case as this computer knows it (payload of ticket.create).
pub fn snapshot(conn: &Connection, tid: i64) -> AppResult<TicketSnapshot> {
    let t = crate::tickets::get(conn, tid)?;
    let owner_shared: Option<String> = match t.assignee_staff_id {
        Some(s) => conn.query_row("SELECT shared_id FROM staff WHERE id=?1", [s], |r| r.get(0)).optional()?.flatten(),
        None => None,
    };
    let mut emails = vec![];
    let links: Vec<(i64, String)> = {
        let mut st = conn.prepare("SELECT message_id, link_type FROM ticket_email_links WHERE ticket_id=?1 ORDER BY linked_at, message_id")?;
        let v = st.query_map([tid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        v
    };
    for (mid, lt) in &links {
        emails.push(email_rec(conn, *mid, lt)?);
    }
    let issues = crate::tickets::issues(conn, tid)?.into_iter().map(|i| (i.issue_type, i.source)).collect();
    let schools: Vec<(i64, bool)> = {
        let mut st = conn.prepare("SELECT slot, removed FROM ticket_schools WHERE ticket_id=?1 ORDER BY slot")?;
        let v = st.query_map([tid], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)))?.collect::<Result<_, _>>()?;
        v
    };
    let fields = crate::tickets::fields(conn, tid)?
        .into_iter()
        .map(|f| FieldRec { key: f.key, extracted: f.extracted_value, has_manual: f.manual_value.is_some(), manual: f.manual_value, verified: f.verified, ..Default::default() })
        .collect();
    let completion = if t.status == crate::tickets::DONE {
        let row: Option<(String, Option<String>, String, String, Option<String>, String)> = conn
            .query_row(
                "SELECT completion_id, actor_name, decided_at, covered_json, max_covered_received_at, state FROM completion_history WHERE ticket_id=?1 ORDER BY decided_at DESC, id DESC LIMIT 1",
                [tid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        row.map(|(cid, actor, at, covered, max, state)| {
            let ids: Vec<i64> = serde_json::from_str(&covered).unwrap_or_default();
            let imids: Vec<String> = ids
                .iter()
                .filter_map(|m| conn.query_row("SELECT internet_message_id FROM email_messages WHERE id=?1", [m], |r| r.get::<_, Option<String>>(0)).ok().flatten())
                .collect();
            CompletionRec { completion_id: cid, actor_name: actor.unwrap_or_default(), decided_at: at, covered: imids, max_covered_received_at: max, state: if state == "legacy" { "legacy".into() } else { "accepted".into() }, ..Default::default() }
        })
    } else {
        None
    };
    Ok(TicketSnapshot {
        core: TicketCore {
            ticket_id: t.ticket_ref.clone().unwrap_or_default(),
            ticket_ref: t.ticket_ref.clone().unwrap_or_default(),
            subject: t.original_subject.clone(),
            sender_name: t.sender_name.clone(),
            sender_email: t.sender_email.clone(),
            school_name: t.school_name.clone(),
            njiis_id: t.school_njiis_id.clone(),
            priority: Some(t.priority.clone()),
            status: t.status.clone(),
            owner_staff_id: owner_shared,
            review_reason: None,
            staff_actions: t.staff_actions.clone(),
            first_received_at: t.first_received_at.clone(),
            last_incoming_at: t.last_email_received_at.clone(),
            done_at: t.completed_at.clone(),
            created_at: t.created_at.clone(),
            origin: Some(t.source.clone()),
            ..Default::default()
        },
        emails,
        issues,
        schools,
        fields,
        completion,
    })
}

/// Queue the creation of a case (new import from Outlook, manual ticket, or migration of an existing
/// local case). Marks the ticket "queued"; no-op in standalone mode.
pub fn queue_create(conn: &Connection, tid: i64, actor_name: &str, origin: &str) -> AppResult<Option<String>> {
    ensure_shared_id(conn, tid)?;
    if mode(conn) != MODE_SHARED {
        return Ok(None);
    }
    if ticket_shared_state(conn, tid)? != "local_only" {
        return Ok(None);
    }
    let mut snap = snapshot(conn, tid)?;
    snap.core.origin = Some(origin.into());
    conn.execute("UPDATE tickets SET shared_state='queued' WHERE id=?1", [tid])?;
    let op = enqueue(conn, actor_name, NewOp { ticket_id: Some(tid), entity_type: "ticket", entity_id: None, op_type: "ticket.create", original: json!({}), proposed: serde_json::to_value(&snap)? })?;
    // notes already written for the case travel as their own operations
    let notes: Vec<(String, String, String, Option<i64>, String)> = {
        let mut st = conn.prepare("SELECT note_id, body, author_name, author_staff_id, created_at FROM ticket_notes WHERE ticket_id=?1 AND deleted=0 ORDER BY id")?;
        let v = st.query_map([tid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
        v
    };
    for (nid, body, author, author_staff, created) in notes {
        let author_shared: Option<String> = match author_staff {
            Some(s) => conn.query_row("SELECT shared_id FROM staff WHERE id=?1", [s], |r| r.get(0)).optional()?.flatten(),
            None => None,
        };
        enqueue(conn, actor_name, NewOp {
            ticket_id: Some(tid),
            entity_type: "note",
            entity_id: Some(nid.clone()),
            op_type: "note.add",
            original: json!({}),
            proposed: json!({"note_id": nid, "text": body, "author_name": author, "author_staff_id": author_shared, "created_at": created}),
        })?;
    }
    Ok(op)
}

/// Called when a ticket was just created locally (ingest or manual): queue it unless standalone.
pub fn on_ticket_created(conn: &Connection, tid: i64, actor_name: &str, origin: &str) -> AppResult<()> {
    queue_create(conn, tid, actor_name, origin)?;
    Ok(())
}

/// A new email was linked to an existing ticket locally.
pub fn on_email_linked(conn: &Connection, tid: i64, message_id: i64, link_type: &str, actor_name: &str) -> AppResult<()> {
    if ticket_shared_state(conn, tid)? == "local_only" {
        return Ok(());
    }
    let rec = email_rec(conn, message_id, link_type)?;
    enqueue(conn, actor_name, NewOp { ticket_id: Some(tid), entity_type: "email", entity_id: Some(message_id.to_string()), op_type: "email.attach", original: json!({}), proposed: serde_json::to_value(&rec)? })?;
    Ok(())
}

pub fn queue_staff(conn: &Connection, staff_id: i64, original: Option<&crate::staff::Staff>, actor_name: &str) -> AppResult<()> {
    let s = crate::staff::get(conn, staff_id)?;
    let (shared, wa): (Option<String>, Option<String>) = conn.query_row("SELECT shared_id, windows_account FROM staff WHERE id=?1", [staff_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let rec = |st: &crate::staff::Staff, wa: Option<String>| json!({"staff_id": shared, "display_name": st.display_name, "email": st.email, "windows_account": wa, "active": st.active});
    let orig = match original {
        Some(o) => rec(o, wa.clone()),
        None => json!({}),
    };
    enqueue(conn, actor_name, NewOp { ticket_id: None, entity_type: "staff", entity_id: shared.clone(), op_type: "staff.upsert", original: orig, proposed: rec(&s, wa) })?;
    Ok(())
}

// ------------------------------------------------------------------ import of existing local cases

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportPreview {
    /// local cases never submitted to a shared database
    pub local_only: i64,
    /// of which have at least one Outlook Message-ID (can be matched to the same email on other computers)
    pub with_message_id: i64,
    /// matched only by fallback evidence; never merged automatically (possible-duplicate review instead)
    pub without_message_id: i64,
    pub done: i64,
    pub open: i64,
    pub staff: i64,
    /// filled when a shared database is connected: local cases whose email is already in the shared database
    pub already_shared: Option<i64>,
    pub held_ops: i64,
}

pub fn import_preview(conn: &Connection) -> AppResult<ImportPreview> {
    let g = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0));
    Ok(ImportPreview {
        local_only: g("SELECT COUNT(*) FROM tickets WHERE shared_state='local_only' AND merged_into IS NULL")?,
        with_message_id: g("SELECT COUNT(*) FROM tickets t WHERE shared_state='local_only' AND merged_into IS NULL AND EXISTS(SELECT 1 FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id WHERE l.ticket_id=t.id AND COALESCE(m.internet_message_id,'')<>'')")?,
        without_message_id: g("SELECT COUNT(*) FROM tickets t WHERE shared_state='local_only' AND merged_into IS NULL AND NOT EXISTS(SELECT 1 FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id WHERE l.ticket_id=t.id AND COALESCE(m.internet_message_id,'')<>'')")?,
        done: g("SELECT COUNT(*) FROM tickets WHERE shared_state='local_only' AND merged_into IS NULL AND status='Done'")?,
        open: g("SELECT COUNT(*) FROM tickets WHERE shared_state='local_only' AND merged_into IS NULL AND status<>'Done'")?,
        staff: g("SELECT COUNT(*) FROM staff")?,
        already_shared: None,
        held_ops: g("SELECT COUNT(*) FROM sync_ops WHERE state='held'")?,
    })
}

/// Message-IDs of local-only cases (for the connected dry run).
pub fn local_only_message_ids(conn: &Connection) -> AppResult<Vec<(i64, String)>> {
    let mut st = conn.prepare(
        "SELECT t.id, m.internet_message_id FROM tickets t JOIN ticket_email_links l ON l.ticket_id=t.id JOIN email_messages m ON m.id=l.message_id
         WHERE t.shared_state='local_only' AND t.merged_into IS NULL AND COALESCE(m.internet_message_id,'')<>''",
    )?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// Queue up to `limit` local-only cases for import (restartable: each batch commits; already queued
/// cases are skipped). Staff records are queued first so owners resolve. Returns the number queued.
pub fn queue_import(conn: &Connection, actor_name: &str, limit: usize) -> AppResult<usize> {
    if mode(conn) != MODE_SHARED {
        return Err(AppError::coded("STANDALONE", "Switch Settings › Shared Database to Shared mode before importing local cases."));
    }
    if bound_db(conn).is_none() {
        return Err(AppError::coded("NOT_CONNECTED", "Connect to the shared database (Test Connection) before importing, so the import is bound to that database."));
    }
    let tx = conn.unchecked_transaction()?;
    if get_setting(&tx, &format!("staff_imported:{}", bound_db(&tx).unwrap_or_default()))?.is_none() {
        let ids: Vec<i64> = {
            let mut st = tx.prepare("SELECT id FROM staff ORDER BY id")?;
            let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            v
        };
        for id in ids {
            queue_staff(&tx, id, None, actor_name)?;
        }
        set_setting(&tx, &format!("staff_imported:{}", bound_db(&tx).unwrap_or_default()), &now())?;
    }
    let ids: Vec<i64> = {
        let mut st = tx.prepare("SELECT id FROM tickets WHERE shared_state='local_only' AND merged_into IS NULL ORDER BY COALESCE(first_received_at, created_at), id LIMIT ?1")?;
        let v = st.query_map([limit as i64], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for id in &ids {
        queue_create(&tx, *id, actor_name, "migration")?;
    }
    tx.commit()?;
    Ok(ids.len())
}

// ------------------------------------------------------------------ identity binding and held review

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// first contact: identity recorded now; earlier operations stay held for review
    NewlyBound,
    Same,
    /// queue belongs to another database; nothing is sent or pulled
    Mismatch { bound: String },
}

pub fn check_binding(conn: &Connection, db_id: &str, db_name: &str) -> AppResult<Binding> {
    match bound_db(conn) {
        None => {
            set_setting(conn, "shared_db_id", db_id)?;
            set_setting(conn, "shared_db_name", db_name)?;
            set_setting(conn, "shared_db_bound_at", &now())?;
            Ok(Binding::NewlyBound)
        }
        Some(b) if b == db_id => Ok(Binding::Same),
        Some(b) => Ok(Binding::Mismatch { bound: b }),
    }
}

/// Staff explicitly switch to a different shared database. Operations bound to the old identity are
/// isolated (kept, never sent anywhere automatically); cases mirrored from the old database become
/// local-only again (they can be imported deliberately).
pub fn rebind(conn: &Connection, new_db: &str, name: &str) -> AppResult<usize> {
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute(
        "UPDATE sync_ops SET state='isolated', hold_reason='database_changed' WHERE state IN ('pending','held') AND COALESCE(target_db_id,'')<>?1",
        [new_db],
    )?;
    tx.execute("UPDATE tickets SET shared_state='local_only', shared_version=0 WHERE shared_state<>'local_only'", [])?;
    set_setting(&tx, "shared_db_id", new_db)?;
    set_setting(&tx, "shared_db_name", name)?;
    set_setting(&tx, "shared_db_bound_at", &now())?;
    tx.commit()?;
    Ok(n)
}

/// Submit held operations (explicit review in Settings) to the bound database.
pub fn release_held(conn: &Connection) -> AppResult<usize> {
    let db = bound_db(conn).ok_or_else(|| AppError::coded("NOT_CONNECTED", "No shared database identity is confirmed yet."))?;
    Ok(conn.execute("UPDATE sync_ops SET state='pending', target_db_id=?1, hold_reason=NULL WHERE state='held'", [db])?)
}

/// Keep held operations local only (they are marked superseded; local data is unchanged). Cases whose
/// creation was held return to local-only so they can be imported deliberately later.
pub fn discard_held(conn: &Connection) -> AppResult<usize> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE tickets SET shared_state='local_only' WHERE id IN (SELECT ticket_id FROM sync_ops WHERE state='held' AND op_type='ticket.create')", [])?;
    let n = tx.execute("UPDATE sync_ops SET state='superseded', settled_at=?1, last_error='Kept on this computer only (held changes not submitted)' WHERE state='held'", [now()])?;
    tx.commit()?;
    Ok(n)
}

/// After restoring a local backup: queued operations may be stale relative to newer shared work, so they
/// are held for review instead of being replayed, and the next pull rebuilds the shared cache.
pub fn after_restore(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute("UPDATE sync_ops SET state='held', hold_reason='restored_backup' WHERE state='pending'", [])?;
    conn.execute("DELETE FROM app_settings WHERE key LIKE 'shared_feed_seq:%' OR key LIKE 'shared_full_at:%'", [])?;
    Ok(n)
}

// ------------------------------------------------------------------ sending and results

pub fn next_batch(conn: &Connection, db_id: &str, max: usize) -> AppResult<Vec<(i64, Op)>> {
    let mut st = conn.prepare(
        "SELECT o.id, o.op_id, o.op_type, o.ticket_id, o.entity_id, o.staff_shared_id, o.staff_name, o.device_id, COALESCE(o.windows_account,''), o.base_version,
                o.original_json, o.proposed_json, o.created_at, t.shared_id
         FROM sync_ops o LEFT JOIN tickets t ON t.id=o.ticket_id
         WHERE o.state='pending' AND o.target_db_id=?1 ORDER BY o.id LIMIT ?2",
    )?;
    let v = st
        .query_map(params![db_id, max as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Op {
                    op_id: r.get(1)?,
                    op_type: r.get(2)?,
                    ticket_id: r.get(13)?,
                    entity_id: r.get(4)?,
                    staff_id: r.get(5)?,
                    staff_name: r.get(6)?,
                    device_id: r.get(7)?,
                    windows_account: r.get(8)?,
                    base_version: r.get(9)?,
                    original: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or(Value::Null),
                    proposed: serde_json::from_str(&r.get::<_, String>(11)?).unwrap_or(Value::Null),
                    created_at: r.get(12)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // ticket.create payloads carry the ticket id from the current local mapping
    Ok(v.into_iter()
        .map(|(id, mut op)| {
            if op.op_type == "ticket.create" {
                if let Ok(Value::Object(mut m)) = serde_json::to_value(&op.proposed) {
                    if let Some(Value::Object(core)) = m.get_mut("core") {
                        core.insert("ticket_id".into(), json!(op.ticket_id.clone().unwrap_or_default()));
                    }
                    op.proposed = Value::Object(m);
                }
            }
            (id, op)
        })
        .collect())
}

pub fn record_attempt(conn: &Connection, row: i64, error: Option<&str>) -> AppResult<()> {
    conn.execute(
        "UPDATE sync_ops SET attempts=attempts+1, last_attempt_at=?2, last_error=?3 WHERE id=?1",
        params![row, now(), error.map(|e| crate::diag::redact(e))],
    )?;
    Ok(())
}

/// Store a shared result locally (accepted / conflict / rejected), remap redirected cases, record conflicts.
/// Returns the local ticket id touched (to refresh it from the shared state).
pub fn record_result(conn: &Connection, row: i64, res: &OpResult) -> AppResult<Option<i64>> {
    let tx = conn.unchecked_transaction()?;
    let (op_id, op_type, tid, entity_id): (String, String, Option<i64>, Option<String>) =
        tx.query_row("SELECT op_id, op_type, ticket_id, entity_id FROM sync_ops WHERE id=?1", [row], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    let state = match res.result.as_str() {
        "accepted" => "accepted",
        "conflict" => "conflict",
        _ => "rejected",
    };
    tx.execute(
        "UPDATE sync_ops SET state=?2, settled_at=?3, result_json=?4, last_error=?5, attempts=attempts+1, last_attempt_at=?3 WHERE id=?1",
        params![row, state, now(), serde_json::to_string(res)?, if state == "rejected" { Some(res.message.clone()) } else { None }],
    )?;
    let mut local = tid;
    if let (Some(t), Some(shared)) = (tid, res.ticket_id.clone()) {
        local = Some(remap_ticket(&tx, t, &shared)?);
        if op_type == "ticket.create" && state != "rejected" {
            tx.execute("UPDATE tickets SET shared_state='shared' WHERE id=?1", [local.unwrap()])?;
        }
        if let Some(r) = &res.ticket_ref {
            tx.execute("UPDATE tickets SET ticket_ref=?2 WHERE id=?1 AND COALESCE(ticket_ref,'')<>?2", params![local.unwrap(), r])?;
        }
    }
    if op_type == "ticket.create" && state == "rejected" {
        if let Some(t) = tid {
            tx.execute("UPDATE tickets SET shared_state='local_only' WHERE id=?1 AND shared_state='queued'", [t])?;
        }
    }
    if op_type == "staff.upsert" {
        if let (Some(old), Some(new)) = (entity_id.clone(), res.extra.get("staff_id").and_then(|v| v.as_str())) {
            if old != new {
                // the same person already existed in the shared database: adopt its id
                tx.execute("UPDATE staff SET shared_id=?2 WHERE shared_id=?1 AND NOT EXISTS(SELECT 1 FROM staff WHERE shared_id=?2)", params![old, new])?;
            }
        }
    }
    if op_type == "conflict.resolve" && state == "accepted" {
        if let Some(cid) = entity_id.clone() {
            tx.execute("UPDATE sync_conflicts SET status='resolved', resolved_at=?2 WHERE conflict_id=?1", params![cid, now()])?;
        }
    }
    for c in &res.conflicts {
        tx.execute(
            "INSERT OR IGNORE INTO sync_conflicts(conflict_id, op_id, ticket_id, field_name, conflict_type, original_value, proposed_value, shared_value, reason, status, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'open',?10)",
            params![c.conflict_id, op_id, local, c.field_name, c.conflict_type, c.original, c.proposed, c.shared, c.reason, now()],
        )?;
    }
    // Done decisions: accepted, or superseded by a newer reply (kept as history either way)
    if op_type == "ticket.done" {
        let st = if state == "accepted" { "accepted" } else { "superseded" };
        tx.execute("UPDATE completion_history SET state=?2, state_reason=COALESCE(?3, state_reason) WHERE op_id=?1", params![op_id, st, if st == "superseded" { Some(res.message.clone()) } else { None }])?;
    }
    // a successful retry settles the conflict it was made for
    if state == "accepted" {
        tx.execute("UPDATE sync_conflicts SET status='resolved', resolved_at=?2, resolution='retried' WHERE resolve_op_id=?1", params![op_id, now()])?;
    } else if state == "conflict" {
        tx.execute("UPDATE sync_conflicts SET status='open' WHERE resolve_op_id=?1 AND status='resolving'", [&op_id])?;
    }
    tx.commit()?;
    Ok(local)
}

/// Point a local ticket at the shared case that accepted it. If another local ticket already mirrors that
/// shared case (the same email imported twice), the two local tickets are combined without losing data.
fn remap_ticket(conn: &Connection, tid: i64, shared: &str) -> AppResult<i64> {
    let cur: Option<String> = conn.query_row("SELECT shared_id FROM tickets WHERE id=?1", [tid], |r| r.get(0))?;
    if cur.as_deref() == Some(shared) {
        return Ok(tid);
    }
    let other: Option<i64> = conn.query_row("SELECT id FROM tickets WHERE shared_id=?1 AND id<>?2", params![shared, tid], |r| r.get(0)).optional()?;
    match other {
        None => {
            conn.execute("UPDATE tickets SET shared_id=?2 WHERE id=?1", params![tid, shared])?;
            crate::tickets::history(conn, tid, "sync", "Joined the shared case for the same email (imported on another computer too)", None, cur.as_deref(), Some(shared))?;
            Ok(tid)
        }
        Some(keep) => {
            merge_local(conn, tid, keep)?;
            Ok(keep)
        }
    }
}

/// Combine local ticket `from` into `into`: links, notes, history, fields, issues, completion and
/// ownership rows move; `from` stays as a hidden redirect (merged_into) - nothing is deleted.
pub fn merge_local(conn: &Connection, from: i64, into: i64) -> AppResult<()> {
    conn.execute("UPDATE OR IGNORE ticket_email_links SET ticket_id=?2 WHERE ticket_id=?1", params![from, into])?;
    for t in ["ticket_notes", "ticket_history", "ownership_history", "completion_history", "message_completion", "sync_conflicts"] {
        conn.execute(&format!("UPDATE {t} SET ticket_id=?2 WHERE ticket_id=?1"), params![from, into])?;
    }
    conn.execute("UPDATE sync_ops SET ticket_id=?2 WHERE ticket_id=?1 AND state IN ('held','pending')", params![from, into])?;
    conn.execute(
        "INSERT OR IGNORE INTO ticket_fields(ticket_id, field_key, extracted_value, extraction_confidence, manual_value, verified, updated_at)
         SELECT ?2, field_key, extracted_value, extraction_confidence, manual_value, verified, updated_at FROM ticket_fields WHERE ticket_id=?1",
        params![from, into],
    )?;
    conn.execute("INSERT OR IGNORE INTO ticket_issues(ticket_id, issue_type, source, position) SELECT ?2, issue_type, source, position+100 FROM ticket_issues WHERE ticket_id=?1", params![from, into])?;
    // the hidden row keeps its own shared id (a redirect in the shared database); it is never listed
    conn.execute("UPDATE tickets SET merged_into=?2, updated_at=?3 WHERE id=?1", params![from, into, now()])?;
    crate::tickets::history(conn, into, "sync", "Combined with a duplicate local ticket for the same email", None, Some(&crate::tickets::display_id(from)), None)?;
    crate::search::reindex(conn, into)?;
    crate::search::reindex(conn, from)?;
    Ok(())
}

// ------------------------------------------------------------------ pulling shared state

/// Keys of a ticket that unsent (held/pending) operations change; pulled values for these keys are
/// stored in the snapshot but not applied over the local provisional value.
pub fn touched_keys(conn: &Connection, tid: i64) -> AppResult<HashSet<String>> {
    let mut st = conn.prepare("SELECT op_type, entity_id, proposed_json FROM sync_ops WHERE ticket_id=?1 AND state IN ('held','pending')")?;
    let rows: Vec<(String, Option<String>, String)> = st.query_map([tid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let mut keys = HashSet::new();
    for (t, e, p) in rows {
        match t.as_str() {
            "ticket.create" => {
                keys.insert("*".to_string());
            }
            "ticket.update" => {
                if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&p) {
                    keys.extend(m.keys().map(|k| format!("t:{k}")));
                }
            }
            "field.set" => {
                keys.insert(format!("f:{}", e.unwrap_or_default()));
            }
            "issues.set" => {
                keys.insert("issues".into());
            }
            "school.add" | "school.remove" => {
                keys.insert("schools".into());
            }
            "note.add" | "note.edit" | "note.delete" => {
                keys.insert(format!("n:{}", e.unwrap_or_default()));
            }
            other => {
                for k in crate::shared_workflow::touched_by(other) {
                    keys.insert(k.to_string());
                }
            }
        }
    }
    Ok(keys)
}

pub fn apply_staff(conn: &Connection, staff: &[sa::StaffRec]) -> AppResult<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for s in staff {
        let local: Option<i64> = tx.query_row("SELECT id FROM staff WHERE shared_id=?1", [&s.staff_id], |r| r.get(0)).optional()?;
        let pending: i64 = tx.query_row("SELECT COUNT(*) FROM sync_ops WHERE entity_type='staff' AND entity_id=?1 AND state IN ('held','pending')", [&s.staff_id], |r| r.get(0))?;
        match local {
            Some(id) if pending == 0 => {
                n += tx.execute(
                    "UPDATE staff SET display_name=?2, email=COALESCE(?3, email), windows_account=?4, active=?5, updated_at=?6
                     WHERE id=?1 AND (display_name<>?2 OR COALESCE(email,'')<>COALESCE(?3,email,'') OR COALESCE(windows_account,'')<>COALESCE(?4,'') OR active<>?5)",
                    params![id, s.display_name, s.email, s.windows_account, s.active as i64, now()],
                )?;
            }
            Some(_) => {}
            None => {
                // a colleague's staff record: no Outlook folder on this computer until someone maps it here
                tx.execute(
                    "INSERT INTO staff(display_name, email, mailbox_name, store_id, folder_entry_id, folder_path, active, created_at, updated_at, shared_id, windows_account)
                     VALUES(?1, COALESCE(?2,''), NULL, '', '', '', ?3, ?4, ?4, ?5, ?6)",
                    params![s.display_name, s.email, s.active as i64, now(), s.staff_id, s.windows_account],
                )?;
                n += 1;
            }
        }
    }
    tx.commit()?;
    Ok(n)
}

fn local_staff(conn: &Connection, shared: &Option<String>) -> AppResult<Option<i64>> {
    match shared.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Ok(conn.query_row("SELECT id FROM staff WHERE shared_id=?1", [s], |r| r.get(0)).optional()?),
        None => Ok(None),
    }
}

fn find_local_email(conn: &Connection, e: &EmailRec) -> AppResult<Option<i64>> {
    if let Some(id) = conn.query_row("SELECT id FROM email_messages WHERE shared_email_id=?1", [&e.email_id], |r| r.get(0)).optional()? {
        return Ok(Some(id));
    }
    let n = sa::normalize_message_id(&e.internet_message_id);
    if n.is_empty() {
        return Ok(None);
    }
    Ok(conn.query_row("SELECT id FROM email_messages WHERE internet_message_id IN (?1, ?2) ORDER BY id LIMIT 1", params![n, format!("<{n}>")], |r| r.get(0)).optional()?)
}

/// Mirror one shared case into the local cache. Values touched by unsent local operations are kept.
pub fn apply_bundle(conn: &Connection, b: &TicketBundle) -> AppResult<i64> {
    let tx = conn.unchecked_transaction()?;
    let c = &b.core;
    let ts = now();
    let existing: Option<i64> = tx.query_row("SELECT id FROM tickets WHERE shared_id=?1", [&c.ticket_id], |r| r.get(0)).optional()?;
    let owner_local = local_staff(&tx, &c.owner_staff_id)?;
    let primary_issue = b.issues.first().map(|i| i.0.clone()).filter(|i| crate::catalog::catalog().issue(i).is_some()).unwrap_or_else(|| "other".into());
    let tid = match existing {
        Some(id) => id,
        None => {
            let conv = b.emails.iter().find(|e| e.link_type == "original").or(b.emails.first()).map(|e| e.conversation_id.clone()).filter(|s| !s.is_empty());
            tx.execute(
                "INSERT INTO tickets(status, priority, issue_type, issue_type_source, sender_name, sender_email, original_subject, conversation_id, source, created_at, updated_at,
                                     first_received_at, last_email_received_at, completed_at, ticket_ref, shared_id, shared_state, assignee_staff_id, assigned_at, review_reason)
                 VALUES(?1, COALESCE(?2,'Normal'), ?3, 'auto', ?4, ?5, ?6, ?7, 'shared', ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'shared', ?15, CASE WHEN ?15 IS NULL THEN NULL ELSE ?9 END, ?16)",
                params![c.status, c.priority, primary_issue, c.sender_name, c.sender_email, c.subject, conv, c.created_at, ts, c.first_received_at, c.last_incoming_at, c.done_at,
                    c.ticket_ref, c.ticket_id, owner_local, c.review_reason],
            )?;
            let id = tx.last_insert_rowid();
            crate::tickets::history(&tx, id, "sync", "Case received from the shared database", None, None, Some(&c.ticket_ref))?;
            id
        }
    };
    let keys = if existing.is_some() { touched_keys(&tx, tid)? } else { HashSet::new() };
    let all = keys.contains("*");
    let k = |key: &str| all || keys.contains(key);

    // case summary
    let set = |col: &str, key: &str, v: &Option<String>| -> AppResult<()> {
        if !k(key) {
            tx.execute(&format!("UPDATE tickets SET {col}=?2 WHERE id=?1 AND COALESCE({col},'')<>COALESCE(?2,'')"), params![tid, v])?;
        }
        Ok(())
    };
    set("priority", "t:priority", &c.priority.clone().or(Some("Normal".into())))?;
    set("sender_name", "t:sender_name", &c.sender_name)?;
    set("sender_email", "t:sender_email", &c.sender_email)?;
    set("staff_actions", "t:staff_actions", &c.staff_actions)?;
    set("original_subject", "t:subject", &c.subject)?;
    if crate::shared_workflow::WORKFLOW_SYNCED && !k("status") && !k("owner") {
        crate::shared_workflow::mirror_status(&tx, tid, c, owner_local)?;
    }
    tx.execute(
        "UPDATE tickets SET ticket_ref=?2, shared_version=?3, shared_snapshot=?4, shared_refreshed_at=?5, shared_state='shared',
           first_received_at=COALESCE(?6, first_received_at), last_email_received_at=NULLIF(MAX(COALESCE(last_email_received_at,''), COALESCE(?7,'')),'')
         WHERE id=?1",
        params![tid, c.ticket_ref, c.version, serde_json::to_string(c)?, ts, c.first_received_at, c.last_incoming_at],
    )?;

    // emails (links are additive; location fields stay local)
    for e in &b.emails {
        let mid = match find_local_email(&tx, e)? {
            Some(m) => {
                tx.execute("UPDATE email_messages SET shared_email_id=?2, match_key=?3, match_method=?4 WHERE id=?1", params![m, e.email_id, e.match_key, e.match_method])?;
                m
            }
            None => {
                let imid = outlook_imid(&e.internet_message_id);
                tx.execute(
                    "INSERT INTO email_messages(internet_message_id, conversation_id, subject, sender_name, sender_email, received_at, body_text, body_truncated, is_internal, imported_at,
                                                scope_state, shared_email_id, match_key, match_method)
                     VALUES(NULLIF(?1,''), NULLIF(?2,''), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'unknown', ?11, ?12, ?13)",
                    params![imid, e.conversation_id, e.subject, e.sender_name, e.sender_email, e.received_at, e.body_text, e.body_truncated as i64, e.is_internal as i64, ts, e.email_id, e.match_key, e.match_method],
                )?;
                tx.last_insert_rowid()
            }
        };
        if e.is_active {
            tx.execute("INSERT OR IGNORE INTO ticket_email_links(ticket_id, message_id, link_type, linked_at) VALUES(?1, ?2, ?3, ?4)", params![tid, mid, e.link_type, ts])?;
        }
    }
    // issue types
    if !k("issues") && !b.issues.is_empty() {
        let cur: Vec<String> = crate::tickets::issues(&tx, tid)?.into_iter().map(|i| i.issue_type).collect();
        let want: Vec<String> = b.issues.iter().map(|i| i.0.clone()).filter(|i| crate::catalog::catalog().issue(i).is_some()).collect();
        if cur != want && !want.is_empty() {
            tx.execute("DELETE FROM ticket_issues WHERE ticket_id=?1", [tid])?;
            for (pos, (it, origin)) in b.issues.iter().enumerate() {
                if crate::catalog::catalog().issue(it).is_some() {
                    tx.execute("INSERT OR IGNORE INTO ticket_issues(ticket_id, issue_type, source, position) VALUES(?1,?2,?3,?4)", params![tid, it, origin, pos as i64])?;
                }
            }
            let manual = b.issues.iter().any(|i| i.1 == "manual");
            tx.execute("UPDATE tickets SET issue_type=?2, issue_type_source=?3 WHERE id=?1", params![tid, want[0], if manual { "manual" } else { "auto" }])?;
        }
    }
    if !k("schools") {
        for (slot, removed) in &b.schools {
            tx.execute(
                "INSERT INTO ticket_schools(ticket_id, slot, removed, created_by, created_at) VALUES(?1,?2,?3,'sync',?4) ON CONFLICT(ticket_id, slot) DO UPDATE SET removed=excluded.removed",
                params![tid, slot, *removed as i64, ts],
            )?;
        }
    }
    for f in &b.fields {
        if k(&format!("f:{}", f.key)) {
            continue;
        }
        tx.execute(
            "INSERT INTO ticket_fields(ticket_id, field_key, extracted_value, manual_value, verified, updated_at) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(ticket_id, field_key) DO UPDATE SET
               manual_value=excluded.manual_value, verified=excluded.verified,
               extracted_value=CASE WHEN trim(COALESCE(ticket_fields.extracted_value,''))='' THEN excluded.extracted_value ELSE ticket_fields.extracted_value END,
               updated_at=CASE WHEN COALESCE(ticket_fields.manual_value,'')<>COALESCE(excluded.manual_value,'') OR ticket_fields.verified<>excluded.verified THEN excluded.updated_at ELSE ticket_fields.updated_at END",
            params![tid, f.key, f.extracted, if f.has_manual { f.manual.clone() } else { None }, f.verified as i64, ts],
        )?;
    }
    for n in &b.notes {
        if k(&format!("n:{}", n.note_id)) {
            continue;
        }
        let author_local = local_staff(&tx, &n.author_staff_id)?;
        tx.execute(
            "INSERT INTO ticket_notes(note_id, ticket_id, author_staff_id, author_name, body, version, deleted, created_at, updated_at, origin) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'shared')
             ON CONFLICT(note_id) DO UPDATE SET body=excluded.body, version=excluded.version, deleted=excluded.deleted, updated_at=excluded.updated_at, ticket_id=excluded.ticket_id",
            params![n.note_id, tid, author_local, n.author_name, n.text, n.version, n.deleted as i64, n.created_at, n.updated_at],
        )?;
    }
    for o in &b.ownership {
        let local = local_staff(&tx, &Some(o.staff_id.clone()))?;
        let name: Option<String> = match local {
            Some(l) => tx.query_row("SELECT display_name FROM staff WHERE id=?1", [l], |r| r.get(0)).optional()?,
            None => None,
        };
        tx.execute(
            "INSERT INTO ownership_history(entry_id, ticket_id, staff_id, staff_shared_id, staff_name, started_at, ended_at, started_by, ended_by, start_reason, end_reason, origin)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'shared')
             ON CONFLICT(entry_id) DO UPDATE SET ended_at=excluded.ended_at, ended_by=excluded.ended_by, end_reason=excluded.end_reason, ticket_id=excluded.ticket_id",
            params![o.entry_id, tid, local, o.staff_id, name, o.started_at, o.ended_at, o.started_by, o.ended_by, o.start_reason, o.end_reason],
        )?;
    }
    if crate::shared_workflow::WORKFLOW_SYNCED {
        // local-origin open periods were provisional; the shared ownership history is authoritative now
        if !k("owner") && !b.ownership.is_empty() {
            tx.execute("DELETE FROM ownership_history WHERE ticket_id=?1 AND origin='local' AND ended_at IS NULL", [tid])?;
        }
    }
    for cpl in &b.completions {
        let covered_local: Vec<i64> = cpl
            .covered
            .iter()
            .filter_map(|eid| tx.query_row("SELECT id FROM email_messages WHERE shared_email_id=?1", [eid], |r| r.get(0)).optional().ok().flatten())
            .collect();
        tx.execute(
            "INSERT INTO completion_history(completion_id, ticket_id, actor_name, decided_at, covered_json, max_covered_received_at, state, state_reason, origin)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'shared')
             ON CONFLICT(completion_id) DO UPDATE SET state=excluded.state, state_reason=excluded.state_reason, ticket_id=excluded.ticket_id",
            params![cpl.completion_id, tid, cpl.actor_name, cpl.decided_at, serde_json::to_string(&covered_local)?, cpl.max_covered_received_at, cpl.state, cpl.state_reason],
        )?;
    }
    for a in &b.activity {
        // the local history already has rows written for this computer's own operation
        if let Some(op) = &a.op_id {
            let mine: i64 = tx.query_row("SELECT COUNT(*) FROM ticket_history WHERE ticket_id=?1 AND op_id=?2", params![tid, op], |r| r.get(0))?;
            if mine > 0 {
                continue;
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO ticket_history(ticket_id, at, actor, action, field, old_value, new_value, shared_activity_id, op_id, source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![tid, a.occurred_at, a.actor_name.clone().unwrap_or_else(|| a.source.clone()), a.action, a.field_name, a.old_value, a.new_value, a.activity_id, a.op_id, a.source],
        )?;
    }
    crate::tickets::refresh_school_columns_pub(&tx, tid)?;
    crate::scope::recompute_ticket(&tx, tid, "sync")?;
    crate::search::reindex(&tx, tid)?;
    tx.commit()?;
    Ok(tid)
}

// ------------------------------------------------------------------ conflicts

#[derive(Debug, Clone, Serialize)]
pub struct LocalConflict {
    pub conflict_id: String,
    pub op_id: String,
    pub ticket_id: Option<i64>,
    pub ticket_ref: Option<String>,
    pub field_name: Option<String>,
    pub conflict_type: String,
    pub original_value: Option<String>,
    pub proposed_value: Option<String>,
    pub shared_value: Option<String>,
    pub reason: Option<String>,
    pub status: String,
    pub op_type: Option<String>,
    pub staff_name: Option<String>,
    pub created_at: String,
}

pub fn conflicts(conn: &Connection, ticket_id: Option<i64>, include_resolved: bool) -> AppResult<Vec<LocalConflict>> {
    let mut st = conn.prepare(
        "SELECT c.conflict_id, c.op_id, c.ticket_id, t.ticket_ref, c.field_name, c.conflict_type, c.original_value, c.proposed_value, c.shared_value, c.reason, c.status, o.op_type, o.staff_name, c.created_at
         FROM sync_conflicts c LEFT JOIN tickets t ON t.id=c.ticket_id LEFT JOIN sync_ops o ON o.op_id=c.op_id
         WHERE (?1 IS NULL OR c.ticket_id=?1) AND (?2=1 OR c.status<>'resolved') ORDER BY c.id DESC LIMIT 500",
    )?;
    let v = st
        .query_map(params![ticket_id, include_resolved as i64], |r| {
            Ok(LocalConflict {
                conflict_id: r.get(0)?,
                op_id: r.get(1)?,
                ticket_id: r.get(2)?,
                ticket_ref: r.get(3)?,
                field_name: r.get(4)?,
                conflict_type: r.get(5)?,
                original_value: r.get(6)?,
                proposed_value: r.get(7)?,
                shared_value: r.get(8)?,
                reason: r.get(9)?,
                status: r.get(10)?,
                op_type: r.get(11)?,
                staff_name: r.get(12)?,
                created_at: r.get(13)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// Accept the shared value: the conflict stays "resolving" until the shared database accepts the resolution.
pub fn accept_shared(conn: &Connection, conflict_id: &str, actor_name: &str) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    let tid: Option<i64> = tx.query_row("SELECT ticket_id FROM sync_conflicts WHERE conflict_id=?1 AND status<>'resolved'", [conflict_id], |r| r.get(0)).optional()?.flatten();
    let op = enqueue(&tx, actor_name, NewOp {
        ticket_id: None,
        entity_type: "conflict",
        entity_id: Some(conflict_id.into()),
        op_type: "conflict.resolve",
        original: json!({}),
        proposed: json!({"resolution": "accept_shared"}),
    })?;
    match op {
        Some(op) => {
            tx.execute("UPDATE sync_conflicts SET status='resolving', resolution='accept_shared', resolve_op_id=?2 WHERE conflict_id=?1", params![conflict_id, op])?;
        }
        None => {
            tx.execute("UPDATE sync_conflicts SET status='resolved', resolution='accept_shared', resolved_at=?2 WHERE conflict_id=?1", params![conflict_id, now()])?;
        }
    }
    if let Some(t) = tid {
        crate::tickets::history(&tx, t, actor_name, "Conflict: kept the shared value", None, None, None)?;
    }
    tx.commit()?;
    Ok(())
}

/// Link a conflict to the explicit retry operation created for it (resolved when that op is accepted).
pub fn mark_retrying(conn: &Connection, conflict_id: &str, op_id: &str) -> AppResult<()> {
    conn.execute("UPDATE sync_conflicts SET status='resolving', resolution='retry', resolve_op_id=?2 WHERE conflict_id=?1", params![conflict_id, op_id])?;
    Ok(())
}

// ------------------------------------------------------------------ status

#[derive(Debug, Clone, Default, Serialize)]
pub struct QueueSummary {
    pub pending: i64,
    pub held: i64,
    pub conflicts: i64,
    pub rejected: i64,
    pub isolated: i64,
    pub oldest_pending_at: Option<String>,
    pub last_error: Option<String>,
}

pub fn summary(conn: &Connection) -> AppResult<QueueSummary> {
    let g = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0));
    Ok(QueueSummary {
        pending: g("SELECT COUNT(*) FROM sync_ops WHERE state='pending'")?,
        held: g("SELECT COUNT(*) FROM sync_ops WHERE state='held'")?,
        conflicts: g("SELECT COUNT(*) FROM sync_conflicts WHERE status<>'resolved'")?,
        rejected: g("SELECT COUNT(*) FROM sync_ops WHERE state='rejected' AND settled_at >= datetime('now','-7 days')")?,
        isolated: g("SELECT COUNT(*) FROM sync_ops WHERE state='isolated'")?,
        oldest_pending_at: conn.query_row("SELECT MIN(created_at) FROM sync_ops WHERE state IN ('pending','held')", [], |r| r.get(0))?,
        last_error: conn.query_row("SELECT last_error FROM sync_ops WHERE state='pending' AND last_error IS NOT NULL ORDER BY last_attempt_at DESC LIMIT 1", [], |r| r.get(0)).optional()?.flatten(),
    })
}

/// "local" | "pending" | "synced" | "conflict" | "failed" for one ticket (badge in lists and the case view).
pub const TICKET_SYNC_SQL: &str = "CASE
    WHEN EXISTS(SELECT 1 FROM sync_conflicts sc WHERE sc.ticket_id=t.id AND sc.status<>'resolved') THEN 'conflict'
    WHEN EXISTS(SELECT 1 FROM sync_ops so WHERE so.ticket_id=t.id AND so.state='rejected' AND so.settled_at >= datetime('now','-7 days')) THEN 'failed'
    WHEN EXISTS(SELECT 1 FROM sync_ops so WHERE so.ticket_id=t.id AND so.state IN ('pending','held')) THEN 'pending'
    WHEN t.shared_state='shared' THEN 'synced'
    ELSE 'local' END";

pub fn ticket_sync_state(conn: &Connection, tid: i64) -> AppResult<String> {
    Ok(conn.query_row(&format!("SELECT {TICKET_SYNC_SQL} FROM tickets t WHERE t.id=?1"), [tid], |r| r.get(0))?)
}

#[derive(Debug, Clone, Serialize)]
pub struct OpRow {
    pub op_id: String,
    pub op_type: String,
    pub state: String,
    pub created_at: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub hold_reason: Option<String>,
    pub staff_name: String,
}

pub fn ticket_ops(conn: &Connection, tid: i64) -> AppResult<Vec<OpRow>> {
    let mut st = conn.prepare(
        "SELECT op_id, op_type, state, created_at, attempts, last_error, hold_reason, staff_name FROM sync_ops WHERE ticket_id=?1 AND state NOT IN ('accepted','superseded') ORDER BY id DESC LIMIT 50",
    )?;
    let v = st
        .query_map([tid], |r| Ok(OpRow { op_id: r.get(0)?, op_type: r.get(1)?, state: r.get(2)?, created_at: r.get(3)?, attempts: r.get(4)?, last_error: r.get(5)?, hold_reason: r.get(6)?, staff_name: r.get(7)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}
