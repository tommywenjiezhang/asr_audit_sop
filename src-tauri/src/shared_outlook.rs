//! Coordination of Outlook side effects between computers (shared schema v2, `OutlookOperations`).
//!
//! Outlook moves and handoff emails cannot be undone, and every computer sees the same team mailbox, so
//! before a computer moves emails for a case it must hold the case's *reservation* in the shared database:
//!
//!   outlook.reserve       {outlook_op_id, kind: "assign" | "archive", to_staff_id?, email_ids, note}
//!                         original {owner_staff_id} = the owner the user saw (stale protection for assign).
//!                         One transaction: refuse if another computer holds an unexpired reservation for
//!                         the same case and kind; for "assign" also change the owner (reason assign /
//!                         reassign, status Assigned) — ownership and the right to move change together.
//!   outlook.progress      {outlook_op_id, state: "moving" | "moved" | "failed", moved_ids, error}  (renews the lease)
//!   outlook.notify_claim  {outlook_op_id}  pending/failed -> sending (only one computer can win this)
//!   outlook.notify_result {outlook_op_id, state: "submitted" | "failed" | "uncertain", error}
//!   outlook.finish        {outlook_op_id, state: "done" | "failed" | "released"}  frees the reservation key
//!
//! These operations are applied ONLINE (never queued): an Outlook step only starts after the shared
//! database accepted the reservation. They reuse the processed-operation table, so a retry with the same
//! op id after a lost acknowledgement returns the stored answer. A refused reservation is a plain
//! "rejected" result with a message (not a synchronization conflict).
//!
//! Leases: a reservation expires LEASE_MINUTES after its last progress report; an expired reservation
//! (crashed computer) can be taken over by another computer. Lease times use the applying computer's
//! clock only to detect abandoned work, never to decide between two competing edits.

use crate::shared_apply::{self as sa, Op, OpResult, WorkflowTx};
use crate::shared_store::{col, col_s, ErrKind, SResult, SharedConn, SharedError, Val};
use crate::shared_workflow::{self as wf, ASSIGNED, DONE, IN_PROGRESS, REVIEW, UNASSIGNED};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const LEASE_MINUTES: i64 = 20;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OutlookOpRec {
    pub outlook_op_id: String,
    pub ticket_id: String,
    /// "assign" | "archive"
    pub kind: String,
    pub from_staff_id: Option<String>,
    pub to_staff_id: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub staff_name: Option<String>,
    /// reserved | moving | moved | done | failed | released | expired
    pub state: String,
    pub email_ids: Vec<String>,
    pub moved_ids: Vec<String>,
    /// pending | sending | submitted | failed | uncertain | skipped
    pub notify_state: Option<String>,
    pub notify_device: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
    pub finished_at: Option<String>,
    pub active: bool,
}

pub const COLS: &str = "outlook_op_id, ticket_id, op_kind, from_staff_id, to_staff_id, device_id, device_name, staff_name, op_state, email_ids, moved_ids, notify_state, notify_device, last_error, created_at, updated_at, expires_at, finished_at, active_key";

pub fn map(r: &crate::shared_store::Row) -> OutlookOpRec {
    let id = col_s(r, 0);
    let key = col_s(r, 18);
    OutlookOpRec {
        active: key != id,
        outlook_op_id: id,
        ticket_id: col_s(r, 1),
        kind: col_s(r, 2),
        from_staff_id: col(r, 3),
        to_staff_id: col(r, 4),
        device_id: col(r, 5),
        device_name: col(r, 6),
        staff_name: col(r, 7),
        state: col_s(r, 8),
        email_ids: serde_json::from_str(&col_s(r, 9)).unwrap_or_default(),
        moved_ids: serde_json::from_str(&col_s(r, 10)).unwrap_or_default(),
        notify_state: col(r, 11),
        notify_device: col(r, 12),
        last_error: col(r, 13),
        created_at: col_s(r, 14),
        updated_at: col_s(r, 15),
        expires_at: col(r, 16),
        finished_at: col(r, 17),
    }
}

fn plus_minutes(ts: &str, m: i64) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| (t.with_timezone(&chrono::Utc) + chrono::Duration::minutes(m)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|_| ts.to_string())
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(String::from).filter(|x| !x.is_empty())
}

fn get(tx: &mut WorkflowTx<'_, '_>, id: &str) -> SResult<Option<OutlookOpRec>> {
    Ok(tx.query(&format!("SELECT {COLS} FROM OutlookOperations WHERE outlook_op_id = ?"), &[id.into()])?.first().map(map))
}

fn device_name(tx: &mut WorkflowTx<'_, '_>, device_id: &str) -> String {
    tx.query("SELECT device_name FROM Devices WHERE device_id = ?", &[device_id.into()]).ok().and_then(|r| r.first().and_then(|r| col(r, 0))).unwrap_or_else(|| "another computer".into())
}

fn reject(res: &mut OpResult, msg: impl Into<String>, extra: Value) {
    res.result = "rejected".into();
    res.message = msg.into();
    res.extra = extra;
}

/// Apply an `outlook.*` operation inside the shared transaction. Returns false for other op types.
pub fn apply(tx: &mut WorkflowTx<'_, '_>, op_type: &str, tid: &str, res: &mut OpResult) -> SResult<bool> {
    match op_type {
        "outlook.reserve" => reserve(tx, tid, res)?,
        "outlook.progress" => progress(tx, tid, res)?,
        "outlook.notify_claim" => notify_claim(tx, tid, res)?,
        "outlook.notify_result" => notify_result(tx, tid, res)?,
        "outlook.finish" => finish(tx, tid, res)?,
        _ => return Ok(false),
    }
    Ok(true)
}

fn reserve(tx: &mut WorkflowTx<'_, '_>, tid: &str, res: &mut OpResult) -> SResult<()> {
    let op = tx.op().clone();
    let p = &op.proposed;
    let kind = s(p, "kind").unwrap_or_default();
    if kind != "assign" && kind != "archive" {
        reject(res, "Unknown Outlook operation kind.", json!({}));
        return Ok(());
    }
    let oid = s(p, "outlook_op_id").unwrap_or_else(sa::new_id);
    let now = tx.now();
    let key = sa::cut(&format!("{tid}|{kind}"), 80);
    let cur = tx.core(tid)?;
    // 1. an active reservation for this case and kind
    if let Some(r) = tx.query(&format!("SELECT {COLS} FROM OutlookOperations WHERE active_key = ?"), &[key.clone().into()])?.first().map(map) {
        let mine = r.device_id.as_deref() == Some(op.device_id.as_str());
        let expired = r.expires_at.as_deref().map(|e| e < now.as_str()).unwrap_or(false);
        if r.outlook_op_id == oid || (mine && kind == "archive") {
            // resuming my own run (retry after a failure or a restart)
            res.extra = json!({"outlook_op_id": r.outlook_op_id, "notify_state": r.notify_state, "resumed": true, "owner_changed": false});
            return Ok(());
        }
        if !expired && !mine {
            let who = r.staff_name.clone().unwrap_or_else(|| "A colleague".into());
            let dev = r.device_name.clone().unwrap_or_else(|| "another computer".into());
            let what = if kind == "assign" { "an assignment" } else { "archiving" };
            reject(res, format!("{who} ({dev}) is already running {what} in Outlook for this case (started {}). Nothing was changed; try again after it finishes.", r.created_at), json!({"busy": true, "holder": who, "holder_device": dev}));
            return Ok(());
        }
        // expired (abandoned) or my own earlier unfinished run: close it and start a new one
        let reason = if expired { "expired" } else { "released" };
        tx.exec(
            "UPDATE OutlookOperations SET op_state = ?, active_key = outlook_op_id, finished_at = ?, updated_at = ?, last_error = ? WHERE outlook_op_id = ?",
            &[reason.into(), now.clone().into(), now.clone().into(), format!("Replaced by a new {kind} run").into(), r.outlook_op_id.clone().into()],
        )?;
        tx.act(tid, if expired { "Abandoned Outlook reservation expired" } else { "Unfinished Outlook run replaced" }, None, None, Some(kind.clone()));
    }
    // 2. kind-specific rules
    let mut from = cur.owner_staff_id.clone();
    let mut notify = "skipped".to_string();
    let mut owner_changed = false;
    match kind.as_str() {
        "assign" => {
            let Some(to) = s(p, "to_staff_id") else {
                reject(res, "No staff member chosen.", json!({}));
                return Ok(());
            };
            if cur.status == DONE {
                reject(res, "This case is Done in the shared database. Reopen it before assigning it.", json!({"stale": true}));
                return Ok(());
            }
            let active: i64 = tx.query("SELECT is_active FROM Staff WHERE staff_id = ?", &[to.clone().into()])?.first().map(|r| crate::shared_store::col_i(r, 0)).unwrap_or(0);
            if active == 0 {
                reject(res, "The chosen staff member is not an active staff record in the shared database (sync first, or reactivate them).", json!({}));
                return Ok(());
            }
            let saw = op.original.get("owner_staff_id").map(|v| v.as_str().map(String::from).filter(|x| !x.is_empty()));
            if let Some(saw) = saw {
                if saw != cur.owner_staff_id {
                    let now_owner = wf::staff_name(tx, &cur.owner_staff_id);
                    reject(res, format!("The owner changed to {now_owner} after you looked. Nothing was moved or sent; review the case and assign again if needed."), json!({"stale": true, "owner": now_owner}));
                    return Ok(());
                }
            }
            from = cur.owner_staff_id.clone();
            if cur.owner_staff_id.as_deref() != Some(to.as_str()) {
                owner_changed = true;
                notify = "pending".into();
                let reason = if cur.owner_staff_id.is_some() { "reassign" } else { "assign" };
                wf::change_owner(tx, tid, &cur, Some(to.clone()), reason)?;
                let after = tx.core(tid)?;
                // the new owner has not started: Unassigned / In Progress / Review -> Assigned
                if matches!(cur.status.as_str(), UNASSIGNED | IN_PROGRESS | REVIEW | ASSIGNED) && cur.status != ASSIGNED {
                    wf::set_status(tx, tid, &after, ASSIGNED, None, None)?;
                }
            }
        }
        _ => {
            if cur.status != DONE {
                reject(res, "Only Done cases are archived in Outlook (the case is not Done in the shared database).", json!({"stale": true}));
                return Ok(());
            }
        }
    }
    let to_staff = if kind == "assign" { s(p, "to_staff_id") } else { None };
    let emails = p.get("email_ids").cloned().unwrap_or(json!([]));
    let dev_name = device_name(tx, &op.device_id);
    tx.exec(
        "INSERT INTO OutlookOperations (outlook_op_id, ticket_id, op_kind, active_key, from_staff_id, to_staff_id, device_id, device_name, staff_id, staff_name, op_state, email_ids, moved_ids, notify_state, notify_id, handoff_note, created_at, updated_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'reserved', ?, '[]', ?, ?, ?, ?, ?, ?)",
        &[
            oid.clone().into(), tid.into(), kind.clone().into(), key.into(), from.clone().map(Val::Text).unwrap_or(Val::Null), to_staff.clone().map(Val::Text).unwrap_or(Val::Null),
            op.device_id.clone().into(), sa::cut(&dev_name, 128).into(), op.staff_id.clone().map(Val::Text).unwrap_or(Val::Null), sa::cut(&op.staff_name, 100).into(),
            emails.to_string().into(), notify.clone().into(), s(p, "notify_id").map(|n| Val::Text(sa::cut(&n, 100))).unwrap_or(Val::Null), s(p, "note").map(Val::Text).unwrap_or(Val::Null),
            now.clone().into(), now.clone().into(), plus_minutes(&now, LEASE_MINUTES).into(),
        ],
    )?;
    tx.act(tid, if kind == "assign" { "Outlook assignment reserved" } else { "Outlook archiving reserved" }, None, None, Some(dev_name));
    tx.touch(tid);
    res.extra = json!({"outlook_op_id": oid, "notify_state": notify, "owner_changed": owner_changed, "from_staff_id": from, "resumed": false});
    Ok(())
}

/// The reservation named by the op, held by this device (or rejected).
fn held(tx: &mut WorkflowTx<'_, '_>, tid: &str, res: &mut OpResult) -> SResult<Option<OutlookOpRec>> {
    let op = tx.op().clone();
    let Some(oid) = s(&op.proposed, "outlook_op_id") else {
        reject(res, "No Outlook operation named.", json!({}));
        return Ok(None);
    };
    match get(tx, &oid)? {
        None => {
            reject(res, "The Outlook reservation does not exist in the shared database.", json!({}));
            Ok(None)
        }
        Some(r) if r.ticket_id != tid => {
            reject(res, "The Outlook reservation belongs to another case.", json!({}));
            Ok(None)
        }
        Some(r) if r.device_id.as_deref() != Some(op.device_id.as_str()) => {
            reject(res, "The Outlook reservation is held by another computer.", json!({"holder_device": r.device_name}));
            Ok(None)
        }
        Some(r) => Ok(Some(r)),
    }
}

fn progress(tx: &mut WorkflowTx<'_, '_>, tid: &str, res: &mut OpResult) -> SResult<()> {
    let Some(r) = held(tx, tid, res)? else { return Ok(()) };
    if !r.active {
        reject(res, format!("This Outlook run already ended ({}).", r.state), json!({"state": r.state}));
        return Ok(());
    }
    let p = tx.op().proposed.clone();
    let now = tx.now();
    let state = s(&p, "state").unwrap_or_else(|| "moving".into());
    let mut moved = r.moved_ids.clone();
    for m in p.get("moved_ids").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        if let Some(m) = m.as_str() {
            if !moved.iter().any(|x| x == m) {
                moved.push(m.to_string());
            }
        }
    }
    tx.exec(
        "UPDATE OutlookOperations SET op_state = ?, moved_ids = ?, last_error = ?, updated_at = ?, expires_at = ? WHERE outlook_op_id = ?",
        &[sa::cut(&state, 16).into(), serde_json::to_string(&moved).unwrap_or_default().into(), s(&p, "error").map(Val::Text).unwrap_or(Val::Null), now.clone().into(), plus_minutes(&now, LEASE_MINUTES).into(), r.outlook_op_id.clone().into()],
    )?;
    if state != r.state {
        let label = match state.as_str() {
            "moved" => if r.kind == "assign" { "Emails moved to the staff folder" } else { "Emails flagged and moved to the Completed folder" },
            "failed" => "Outlook step failed - retry available",
            _ => "Outlook step in progress",
        };
        tx.act(tid, label, None, None, s(&p, "error").or(Some(format!("{} of {} email(s)", moved.len(), r.email_ids.len()))));
    }
    tx.touch(tid);
    res.extra = json!({"outlook_op_id": r.outlook_op_id, "moved": moved.len()});
    Ok(())
}

fn notify_claim(tx: &mut WorkflowTx<'_, '_>, tid: &str, res: &mut OpResult) -> SResult<()> {
    let Some(r) = held(tx, tid, res)? else { return Ok(()) };
    let now = tx.now();
    match r.notify_state.as_deref().unwrap_or("pending") {
        "pending" | "failed" => {
            let dev = tx.op().device_id.clone();
            let n = tx.exec(
                "UPDATE OutlookOperations SET notify_state = 'sending', notify_device = ?, notify_at = ?, updated_at = ?, expires_at = ? WHERE outlook_op_id = ? AND (notify_state = 'pending' OR notify_state = 'failed' OR notify_state IS NULL)",
                &[dev.into(), now.clone().into(), now.clone().into(), plus_minutes(&now, LEASE_MINUTES).into(), r.outlook_op_id.clone().into()],
            )?;
            if n == 0 {
                reject(res, "The handoff email is already being sent.", json!({"notify_state": "sending"}));
                return Ok(());
            }
            tx.touch(tid);
            res.extra = json!({"outlook_op_id": r.outlook_op_id, "notify_state": "sending"});
        }
        "submitted" => reject(res, "The handoff email was already submitted to Outlook. It is not sent again.", json!({"notify_state": "submitted"})),
        "uncertain" => reject(res, "The earlier send attempt is uncertain. Check Sent Items before sending again.", json!({"notify_state": "uncertain"})),
        "skipped" => reject(res, "No handoff is needed (the owner did not change).", json!({"notify_state": "skipped"})),
        other => reject(res, format!("The handoff email is {other}."), json!({"notify_state": other})),
    }
    Ok(())
}

fn notify_result(tx: &mut WorkflowTx<'_, '_>, tid: &str, res: &mut OpResult) -> SResult<()> {
    let Some(r) = held(tx, tid, res)? else { return Ok(()) };
    let p = tx.op().proposed.clone();
    let state = s(&p, "state").unwrap_or_else(|| "failed".into());
    if !matches!(state.as_str(), "submitted" | "failed" | "uncertain" | "pending") {
        reject(res, "Unknown handoff state.", json!({}));
        return Ok(());
    }
    // "pending" = staff confirmed a resend after an uncertain result (explicit, from the UI)
    if state == "pending" && r.notify_state.as_deref() != Some("uncertain") {
        reject(res, "Only an uncertain handoff can be released for a resend.", json!({}));
        return Ok(());
    }
    let now = tx.now();
    tx.exec(
        "UPDATE OutlookOperations SET notify_state = ?, notify_at = ?, last_error = ?, updated_at = ? WHERE outlook_op_id = ?",
        &[state.clone().into(), now.clone().into(), s(&p, "error").map(Val::Text).unwrap_or(Val::Null), now.into(), r.outlook_op_id.clone().into()],
    )?;
    let label = match state.as_str() {
        "submitted" => "Handoff email submitted to Outlook",
        "uncertain" => "Handoff email outcome uncertain - check Sent Items",
        "pending" => "Handoff resend confirmed by staff",
        _ => "Handoff email failed - retry available",
    };
    tx.act(tid, label, None, None, s(&p, "to"));
    tx.touch(tid);
    res.extra = json!({"outlook_op_id": r.outlook_op_id, "notify_state": state});
    Ok(())
}

fn finish(tx: &mut WorkflowTx<'_, '_>, tid: &str, res: &mut OpResult) -> SResult<()> {
    let Some(r) = held(tx, tid, res)? else { return Ok(()) };
    let p = tx.op().proposed.clone();
    let state = s(&p, "state").unwrap_or_else(|| "done".into());
    if !matches!(state.as_str(), "done" | "failed" | "released") {
        reject(res, "Unknown final state.", json!({}));
        return Ok(());
    }
    let now = tx.now();
    // a failed run keeps its key so nobody else starts a conflicting run until it is retried or released
    let free = state != "failed";
    tx.exec(
        &format!("UPDATE OutlookOperations SET op_state = ?, finished_at = ?, updated_at = ?, last_error = ?{} WHERE outlook_op_id = ?", if free { ", active_key = outlook_op_id" } else { "" }),
        &[state.clone().into(), now.clone().into(), now.into(), s(&p, "error").map(Val::Text).unwrap_or(Val::Null), r.outlook_op_id.clone().into()],
    )?;
    let label = match (r.kind.as_str(), state.as_str()) {
        ("assign", "done") => "Outlook assignment finished",
        ("archive", "done") => "Outlook archiving finished",
        (_, "released") => "Outlook reservation released",
        _ => "Outlook step failed - retry available",
    };
    tx.act(tid, label, None, None, None);
    tx.touch(tid);
    res.extra = json!({"outlook_op_id": r.outlook_op_id, "state": state});
    Ok(())
}

/// Outlook runs of these cases (for mirroring and the pending-operation indicators).
pub fn for_tickets(c: &mut dyn SharedConn, ids: &[String]) -> SResult<Vec<OutlookOpRec>> {
    let mut out = vec![];
    for chunk in ids.chunks(40) {
        let ph = vec!["?"; chunk.len()].join(", ");
        let p: Vec<Val> = chunk.iter().map(|x| x.clone().into()).collect();
        for r in c.query(&format!("SELECT {COLS} FROM OutlookOperations WHERE ticket_id IN ({ph}) ORDER BY created_at"), &p)? {
            out.push(map(&r));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ online client side

/// Build an online `outlook.*` op for a case.
pub fn new_op(op_type: &str, ticket_id: &str, dev: &crate::shared_store::DeviceInfo, staff_id: Option<String>, staff_name: &str, original: Value, proposed: Value) -> Op {
    Op {
        op_id: sa::new_id(),
        op_type: op_type.into(),
        ticket_id: Some(ticket_id.into()),
        staff_id,
        staff_name: staff_name.into(),
        device_id: dev.device_id.clone(),
        windows_account: dev.windows_account.clone(),
        original,
        proposed,
        created_at: crate::db::now(),
        ..Default::default()
    }
}

/// Apply one op online on the given connection. The database identity must be the one this computer is
/// bound to. Transient errors are retried with the SAME op id (a lost acknowledgement returns the stored
/// result instead of applying twice).
pub fn apply_online(c: &mut dyn SharedConn, bound_db: &str, op: &Op) -> SResult<OpResult> {
    let id = crate::shared_store::identity(c)?;
    if id.database_id != bound_db {
        return Err(SharedError::new(ErrKind::Incompatible, "The shared database at the configured path is not the one this computer is connected to (different identity). Nothing was changed."));
    }
    if crate::shared_store::compatibility(&id) != "ok" {
        return Err(SharedError::new(ErrKind::Incompatible, "The shared database schema does not match this app version."));
    }
    if id.maintenance {
        return Err(SharedError::new(ErrKind::Maintenance, "The shared database is in maintenance mode."));
    }
    let mut last = None;
    for i in 0..3 {
        match sa::apply(c, op) {
            Ok(r) => return Ok(r),
            Err(e) if e.kind.transient() || e.kind == ErrKind::Locked => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(300 * (i + 1)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| SharedError::new(ErrKind::Other, "not applied")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_apply::{EmailRec, TicketCore, TicketSnapshot};
    use crate::shared_store::{DeviceInfo, SqliteShared};

    fn dev(n: &str) -> DeviceInfo {
        DeviceInfo { device_id: format!("dev-{n}"), device_name: format!("PC-{n}"), windows_account: n.into(), app_version: "t".into() }
    }

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = crate::shared_store::tests::new_shared(dir.path());
        let mut c = crate::shared_store::open(&p, false).unwrap();
        for d in ["A", "B"] {
            crate::shared_store::heartbeat(c.as_mut(), &dev(d)).unwrap();
        }
        let mk_staff = |c: &mut dyn SharedConn, name: &str| -> String {
            let op = Op { op_id: sa::new_id(), op_type: "staff.upsert".into(), proposed: json!({"staff_id": sa::new_id(), "display_name": name, "email": format!("{name}@example.org"), "active": true}), device_id: "dev-A".into(), ..Default::default() };
            sa::apply(c, &op).unwrap().extra["staff_id"].as_str().unwrap().to_string()
        };
        let a = mk_staff(c.as_mut(), "Aaron");
        let w = mk_staff(c.as_mut(), "Wenjie");
        let snap = TicketSnapshot {
            core: TicketCore { ticket_ref: "ASR-T1".into(), status: "Unassigned".into(), ..Default::default() },
            emails: vec![EmailRec { internet_message_id: "<m1@x>".into(), received_at: "2026-11-18T10:00:00Z".into(), link_type: "original".into(), is_active: true, ..Default::default() }],
            ..Default::default()
        };
        let op = Op { op_id: sa::new_id(), op_type: "ticket.create".into(), ticket_id: Some(sa::new_id()), proposed: serde_json::to_value(&snap).unwrap(), device_id: "dev-A".into(), ..Default::default() };
        let t = sa::apply(c.as_mut(), &op).unwrap().ticket_id.unwrap();
        (dir, p, t, a, w)
    }

    fn bound(p: &std::path::Path) -> String {
        let mut c = crate::shared_store::open(p, true).unwrap();
        crate::shared_store::identity(c.as_mut()).unwrap().database_id
    }

    #[test]
    fn only_one_computer_gets_the_reservation_and_assign_changes_owner_atomically() {
        let (_d, p, t, a, w) = setup();
        let db = bound(&p);
        let mut c = crate::shared_store::open(&p, false).unwrap();
        let ra = apply_online(c.as_mut(), &db, &new_op("outlook.reserve", &t, &dev("A"), None, "Admin A", json!({"owner_staff_id": null}), json!({"kind": "assign", "to_staff_id": a, "email_ids": ["e1"]}))).unwrap();
        assert_eq!(ra.result, "accepted", "{ra:?}");
        assert_eq!(ra.extra["notify_state"], json!("pending"));
        let core = sa::get_core(c.as_mut(), &t).unwrap().unwrap();
        assert_eq!((core.owner_staff_id.as_deref(), core.status.as_str()), (Some(a.as_str()), "Assigned"));
        // B saw the case unassigned and tries the same: refused (stale owner), nothing changed
        let rb = apply_online(c.as_mut(), &db, &new_op("outlook.reserve", &t, &dev("B"), None, "Admin B", json!({"owner_staff_id": null}), json!({"kind": "assign", "to_staff_id": w}))).unwrap();
        assert_eq!(rb.result, "rejected");
        assert!(rb.extra["busy"].as_bool().unwrap_or(false), "A holds the reservation: {rb:?}");
        assert_eq!(sa::get_core(c.as_mut(), &t).unwrap().unwrap().owner_staff_id.as_deref(), Some(a.as_str()));
        let oid = ra.extra["outlook_op_id"].as_str().unwrap().to_string();
        // only the holder reports progress; the handoff can be claimed once
        let pr = apply_online(c.as_mut(), &db, &new_op("outlook.progress", &t, &dev("B"), None, "B", json!({}), json!({"outlook_op_id": oid, "state": "moved"}))).unwrap();
        assert_eq!(pr.result, "rejected");
        assert_eq!(apply_online(c.as_mut(), &db, &new_op("outlook.progress", &t, &dev("A"), None, "A", json!({}), json!({"outlook_op_id": oid, "state": "moved", "moved_ids": ["e1"]}))).unwrap().result, "accepted");
        let claim = new_op("outlook.notify_claim", &t, &dev("A"), None, "A", json!({}), json!({"outlook_op_id": oid}));
        assert_eq!(apply_online(c.as_mut(), &db, &claim).unwrap().result, "accepted");
        // lost acknowledgement: the same op id returns the stored acceptance, a NEW claim is refused
        assert!(apply_online(c.as_mut(), &db, &claim).unwrap().duplicate);
        assert_eq!(apply_online(c.as_mut(), &db, &new_op("outlook.notify_claim", &t, &dev("A"), None, "A", json!({}), json!({"outlook_op_id": oid}))).unwrap().result, "rejected");
        apply_online(c.as_mut(), &db, &new_op("outlook.notify_result", &t, &dev("A"), None, "A", json!({}), json!({"outlook_op_id": oid, "state": "submitted"}))).unwrap();
        assert_eq!(apply_online(c.as_mut(), &db, &new_op("outlook.notify_claim", &t, &dev("A"), None, "A", json!({}), json!({"outlook_op_id": oid}))).unwrap().result, "rejected", "never sent twice");
        apply_online(c.as_mut(), &db, &new_op("outlook.finish", &t, &dev("A"), None, "A", json!({}), json!({"outlook_op_id": oid, "state": "done"}))).unwrap();
        // finished: B may now reassign (seeing A as owner)
        let rb = apply_online(c.as_mut(), &db, &new_op("outlook.reserve", &t, &dev("B"), None, "Admin B", json!({"owner_staff_id": a}), json!({"kind": "assign", "to_staff_id": w}))).unwrap();
        assert_eq!(rb.result, "accepted", "{rb:?}");
        let hist = c.query("SELECT staff_id, ended_at, start_reason FROM OwnershipHistory WHERE ticket_id = ?", &[t.clone().into()]).unwrap();
        assert_eq!(hist.len(), 2, "A's period ended, W's started");
        let open: Vec<_> = hist.iter().filter(|r| col(r, 1).is_none()).collect();
        assert_eq!(open.len(), 1);
        assert_eq!((col_s(open[0], 0), col_s(open[0], 2)), (w.clone(), "reassign".to_string()));
        assert!(hist.iter().any(|r| col_s(r, 0) == a && col(r, 1).is_some() && col_s(r, 2) == "assign"));
        let ops = for_tickets(c.as_mut(), &[t.clone()]).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(!ops[0].active && ops[1].active);
    }

    #[test]
    fn expired_reservation_can_be_taken_over_and_archive_requires_done() {
        let (_d, p, t, a, _w) = setup();
        let db = bound(&p);
        let mut c = crate::shared_store::open(&p, false).unwrap();
        let r = apply_online(c.as_mut(), &db, &new_op("outlook.reserve", &t, &dev("A"), None, "A", json!({"owner_staff_id": null}), json!({"kind": "assign", "to_staff_id": a}))).unwrap();
        assert_eq!(r.result, "accepted");
        // A crashed; its lease ran out
        c.exec("UPDATE OutlookOperations SET expires_at = '2000-01-01T00:00:00Z'", &[]).unwrap();
        let r = apply_online(c.as_mut(), &db, &new_op("outlook.reserve", &t, &dev("B"), None, "B", json!({"owner_staff_id": a}), json!({"kind": "assign", "to_staff_id": a}))).unwrap();
        assert_eq!(r.result, "accepted", "{r:?}");
        assert_eq!(r.extra["notify_state"], json!("skipped"), "same owner: moves only, no handoff");
        let rows = c.query("SELECT op_state FROM OutlookOperations ORDER BY created_at", &[]).unwrap();
        assert!(rows.iter().any(|r| col_s(r, 0) == "expired"));
        let r = apply_online(c.as_mut(), &db, &new_op("outlook.reserve", &t, &dev("A"), None, "A", json!({}), json!({"kind": "archive"}))).unwrap();
        assert_eq!(r.result, "rejected", "not Done yet");
        // the wrong database identity is refused before anything is applied
        assert!(apply_online(c.as_mut(), "other-db", &new_op("outlook.reserve", &t, &dev("A"), None, "A", json!({}), json!({"kind": "archive"}))).is_err());
        let _ = SqliteShared::open_in_memory();
    }
}
