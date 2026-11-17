//! Current Tickets vs. Ticket History: which known emails are currently inside the selected scope.
//!
//! Discovery (importing new mail, with date window / unread filter) is separate from verification:
//! a reconciliation takes a complete *inventory* of the scope folders (all items, no filters) and compares
//! it with every known email. Absence is applied only when the inventory is complete and was taken for the
//! scope that is still selected. Emails that left the scope are then *located* (lookup only) to record why.
//!
//! Ticket placement (`tickets.scope_state`) is independent of the workflow status:
//!   current    - at least one incoming source email is confirmed in the scope
//!   history    - all incoming source emails are confirmed outside the scope
//!   unverified - not confirmed yet ("Awaiting Verification"); never shown as current
//! Owners are inferred from staff folders; emails in two staff folders flag an ownership conflict.

use crate::db::now;
use crate::error::AppResult;
use crate::settings::Settings;
use crate::staff;
use crate::tickets::{self, history};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// The scope a scan or reconciliation was made for. Results for another key are discarded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeCfg {
    pub key: String,
    /// (store_id, folder EntryID, include subfolders)
    pub folders: Vec<(String, String, bool)>,
    /// excluded subtrees (the Mark Done completion folder)
    pub exclude: Vec<String>,
    pub archive: Option<String>,
}

pub fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn cfg(s: &Settings) -> ScopeCfg {
    let folders: Vec<(String, String, bool)> = s.monitored().into_iter().map(|(f, subs)| (f.store_id, f.entry_id, subs)).collect();
    let exclude: Vec<String> = s.archive_folder.iter().map(|a| a.entry_id.clone()).collect();
    let mut sig = String::new();
    for (st, e, subs) in &folders {
        sig.push_str(&format!("{st}|{e}|{subs};"));
    }
    sig.push_str(&format!("x:{}", exclude.join(",")));
    ScopeCfg { key: format!("{:016x}", fnv64(&sig)), folders, exclude, archive: s.archive_folder.as_ref().map(|a| a.entry_id.clone()) }
}

/// `chain` = [folder, parent, grandparent, ...] EntryIDs of the folder an email is in.
pub fn in_scope(c: &ScopeCfg, chain: &[String]) -> bool {
    if chain.is_empty() || chain.iter().any(|id| c.exclude.contains(id)) {
        return false;
    }
    c.folders.iter().any(|(_, e, subs)| &chain[0] == e || (*subs && chain[1..].contains(e)))
}

fn chain_of(folder_entry_id: &str, ancestors: &[String]) -> Vec<String> {
    let mut v = vec![folder_entry_id.to_string()];
    v.extend(ancestors.iter().cloned());
    v
}

// ------------------------------------------------------------------ inventory (verification)

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct InvFolder {
    pub entry_id: String,
    pub store_id: String,
    pub path: String,
    pub ancestors: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct InvItem {
    pub imid: String,
    pub entry_id: String,
    pub folder_entry_id: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Inventory {
    pub scope_key: String,
    /// true only when every scope folder (and subfolder) was read without error
    pub complete: bool,
    pub folders: Vec<InvFolder>,
    pub items: Vec<InvItem>,
    pub errors: Vec<String>,
}

/// A known email that is no longer in the scope; looked up to find out where it went.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NeedLocate {
    pub message_id: i64,
    pub entry_id: String,
    pub store_id: String,
    pub imid: String,
    pub folder_entry_id: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReconcileReport {
    pub applied: bool,
    pub discarded: Option<String>,
    pub complete: bool,
    pub in_scope: usize,
    pub newly_out: usize,
    pub current: i64,
    pub history: i64,
    pub unverified: i64,
    #[serde(skip)]
    pub to_locate: Vec<NeedLocate>,
}

fn update_location(conn: &Connection, mid: i64, entry: &str, store: &str, folder: &InvFolder, owner: Option<i64>, scope: &str, key: &str, reason: Option<&str>) -> AppResult<()> {
    conn.execute(
        "UPDATE email_messages SET entry_id=COALESCE(NULLIF(?2,''),entry_id), store_id=COALESCE(NULLIF(?3,''),store_id), folder_entry_id=?4, folder_path=?5,
           folder_ancestors=?6, owner_staff_id=?7, scope_state=?8, scope_key=?9, location_status='found', location_checked_at=?10, out_reason=?11
         WHERE id=?1",
        params![mid, entry, store, folder.entry_id, folder.path, serde_json::to_string(&folder.ancestors)?, owner, scope, key, now(), reason],
    )?;
    Ok(())
}

/// Apply a scope inventory. Presence is always recorded; absence only when the inventory is complete.
pub fn apply_inventory(conn: &Connection, current: &ScopeCfg, inv: &Inventory) -> AppResult<ReconcileReport> {
    let mut rep = ReconcileReport { complete: inv.complete, ..Default::default() };
    if inv.scope_key != current.key {
        rep.discarded = Some("The folder selection changed while checking; the results were discarded.".into());
        return Ok(rep);
    }
    let staff = staff::list(conn, true)?;
    let folders: HashMap<&str, &InvFolder> = inv.folders.iter().map(|f| (f.entry_id.as_str(), f)).collect();
    let by_imid: HashMap<&str, &InvItem> = inv.items.iter().filter(|i| !i.imid.is_empty()).map(|i| (i.imid.as_str(), i)).collect();
    let by_entry: HashMap<&str, &InvItem> = inv.items.iter().map(|i| (i.entry_id.as_str(), i)).collect();
    let tx = conn.unchecked_transaction()?;
    // emails whose location is unknown (not found / lookup error) are looked up again, at most every 6 hours
    let recheck_before = (chrono::Utc::now() - chrono::Duration::hours(6)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let known: Vec<(i64, String, String, String, String, Option<String>, String, bool)> = {
        let mut st = tx.prepare(
            "SELECT DISTINCT m.id, COALESCE(m.entry_id,''), COALESCE(m.store_id,''), COALESCE(m.internet_message_id,''), COALESCE(m.folder_entry_id,''), m.scope_key, m.scope_state,
                    (COALESCE(m.location_status,'') IN ('not_found','error') AND COALESCE(m.location_checked_at,'') < ?1)
             FROM email_messages m JOIN ticket_email_links l ON l.message_id=m.id
             WHERE COALESCE(m.entry_id,'')<>'' OR COALESCE(m.internet_message_id,'')<>''",
        )?;
        let v = st.query_map([&recheck_before], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?.collect::<Result<_, _>>()?;
        v
    };
    for (mid, entry, store, imid, folder_id, key, state, recheck) in known {
        let hit = (!imid.is_empty()).then(|| by_imid.get(imid.as_str())).flatten().or_else(|| by_entry.get(entry.as_str())).copied();
        match hit.and_then(|i| folders.get(i.folder_entry_id.as_str()).map(|f| (i, *f))) {
            Some((item, folder)) => {
                let chain = chain_of(&folder.entry_id, &folder.ancestors);
                let owner = staff::owner_for(&staff, &chain);
                update_location(&tx, mid, &item.entry_id, &folder.store_id, folder, owner, "in", &current.key, None)?;
                rep.in_scope += 1;
            }
            None if inv.complete => {
                if state != "out" || key.as_deref() != Some(current.key.as_str()) {
                    tx.execute("UPDATE email_messages SET scope_state='out', scope_key=?2, location_checked_at=?3 WHERE id=?1", params![mid, current.key, now()])?;
                    rep.newly_out += 1;
                    rep.to_locate.push(NeedLocate { message_id: mid, entry_id: entry, store_id: store, imid, folder_entry_id: folder_id });
                } else if recheck {
                    rep.to_locate.push(NeedLocate { message_id: mid, entry_id: entry, store_id: store, imid, folder_entry_id: folder_id });
                }
            }
            None => {} // incomplete check: keep the last confirmed membership
        }
    }
    recompute_all(&tx, "system")?;
    tx.commit()?;
    rep.applied = true;
    let (c, h, u) = counts(conn)?;
    (rep.current, rep.history, rep.unverified) = (c, h, u);
    Ok(rep)
}

// ------------------------------------------------------------------ locating emails that left the scope

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LocateResult {
    pub message_id: i64,
    /// "found" | "not_found" (every candidate folder was searched successfully) | "error"
    pub status: String,
    pub entry_id: String,
    pub store_id: String,
    pub folder_entry_id: String,
    pub folder_path: String,
    pub ancestors: Vec<String>,
    pub error: Option<String>,
}

/// Resolve emails that left the scope from a wide inventory (whole Inbox trees, completion and staff
/// folders - one fast table read). Returns the found locations and the emails still to look up one by one.
pub fn locate_from_inventory(wide: &Inventory, items: &[NeedLocate]) -> (Vec<LocateResult>, Vec<NeedLocate>) {
    let folders: HashMap<&str, &InvFolder> = wide.folders.iter().map(|f| (f.entry_id.as_str(), f)).collect();
    let by_imid: HashMap<&str, &InvItem> = wide.items.iter().filter(|i| !i.imid.is_empty()).map(|i| (i.imid.as_str(), i)).collect();
    let by_entry: HashMap<&str, &InvItem> = wide.items.iter().map(|i| (i.entry_id.as_str(), i)).collect();
    let mut found = Vec::new();
    let mut rest = Vec::new();
    for n in items {
        let hit = (!n.imid.is_empty()).then(|| by_imid.get(n.imid.as_str())).flatten().or_else(|| by_entry.get(n.entry_id.as_str())).copied();
        match hit.and_then(|i| folders.get(i.folder_entry_id.as_str()).map(|f| (i, *f))) {
            Some((i, f)) => found.push(LocateResult {
                message_id: n.message_id,
                status: "found".into(),
                entry_id: i.entry_id.clone(),
                store_id: f.store_id.clone(),
                folder_entry_id: f.entry_id.clone(),
                folder_path: f.path.clone(),
                ancestors: f.ancestors.clone(),
                error: None,
            }),
            None => rest.push(n.clone()),
        }
    }
    (found, rest)
}

pub fn apply_locate(conn: &Connection, current: &ScopeCfg, results: &[LocateResult]) -> AppResult<()> {
    let staff = staff::list(conn, true)?;
    let tx = conn.unchecked_transaction()?;
    for r in results {
        match r.status.as_str() {
            "found" => {
                let chain = chain_of(&r.folder_entry_id, &r.ancestors);
                let owner = staff::owner_for(&staff, &chain);
                let inside = in_scope(current, &chain);
                let reason = if inside {
                    None
                } else if current.archive.as_ref().map(|a| chain.contains(a)).unwrap_or(false) {
                    Some("completed")
                } else if owner.is_some() {
                    Some("reassigned")
                } else {
                    Some("moved")
                };
                let f = InvFolder { entry_id: r.folder_entry_id.clone(), store_id: r.store_id.clone(), path: r.folder_path.clone(), ancestors: r.ancestors.clone() };
                update_location(&tx, r.message_id, &r.entry_id, &r.store_id, &f, owner, if inside { "in" } else { "out" }, &current.key, reason)?;
            }
            "not_found" => {
                tx.execute(
                    "UPDATE email_messages SET location_status='not_found', out_reason='not_found', location_checked_at=?2 WHERE id=?1",
                    params![r.message_id, now()],
                )?;
            }
            _ => {
                // lookup error: membership stays as confirmed by the inventory, location needs verification
                tx.execute(
                    "UPDATE email_messages SET location_status='error', out_reason=COALESCE(out_reason,'unknown'), location_checked_at=?2 WHERE id=?1",
                    params![r.message_id, now()],
                )?;
            }
        }
    }
    recompute_all(&tx, "system")?;
    tx.commit()?;
    Ok(())
}

/// Record a location that Outlook just reported for a message this app moved (assignment / Mark Done).
pub fn record_move(conn: &Connection, current: &ScopeCfg, message_id: i64, entry_id: &str, store_id: &str, folder_entry_id: &str, folder_path: &str, ancestors: &[String], reason_if_out: &str) -> AppResult<()> {
    let staff = staff::list(conn, true)?;
    let chain = chain_of(folder_entry_id, ancestors);
    let owner = staff::owner_for(&staff, &chain);
    let inside = !ancestors.is_empty() && in_scope(current, &chain);
    let scope = if ancestors.is_empty() { "unknown" } else if inside { "in" } else { "out" };
    let f = InvFolder { entry_id: folder_entry_id.into(), store_id: store_id.into(), path: folder_path.into(), ancestors: ancestors.to_vec() };
    update_location(conn, message_id, entry_id, store_id, &f, owner, scope, &current.key, if scope == "out" { Some(reason_if_out) } else { None })
}

// ------------------------------------------------------------------ ticket placement + owner inference

pub fn recompute_all(conn: &Connection, actor: &str) -> AppResult<()> {
    let ids: Vec<i64> = {
        let mut st = conn.prepare("SELECT id FROM tickets")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for id in ids {
        recompute_ticket(conn, id, actor)?;
    }
    Ok(())
}

pub const REASONS: &[(&str, &str)] = &[
    ("completed", "Completed"),
    ("reassigned", "Reassigned outside the selected scope"),
    ("moved", "Moved elsewhere"),
    ("not_found", "No longer found after successful reconciliation"),
    ("unknown", "Outside the scope; current location not verified"),
];

pub fn reason_label(r: &str) -> &str {
    REASONS.iter().find(|(k, _)| *k == r).map(|(_, v)| *v).unwrap_or(r)
}

/// Decide current / history / unverified for one ticket and infer its owner from staff folders.
pub fn recompute_ticket(conn: &Connection, id: i64, actor: &str) -> AppResult<()> {
    let Some((status, assignee, old_scope, old_conflict)): Option<(String, Option<i64>, String, i64)> = conn
        .query_row("SELECT status, assignee_staff_id, scope_state, ownership_conflict FROM tickets WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .optional()?
    else {
        return Ok(());
    };
    let rows: Vec<(String, Option<String>, Option<String>, Option<i64>)> = {
        let mut st = conn.prepare(
            "SELECT m.scope_state, m.out_reason, m.location_status, m.owner_staff_id
             FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
             WHERE l.ticket_id=?1 AND l.link_type IN ('original','reply') AND m.is_internal=0
               AND (COALESCE(m.entry_id,'')<>'' OR COALESCE(m.internet_message_id,'')<>'')",
        )?;
        let v = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
        v
    };
    let (scope, reason): (&str, Option<String>) = if rows.is_empty() {
        // manual tickets have no Outlook email: open ones are current, finished ones history
        if status == tickets::DONE { ("history", Some("completed".into())) } else { ("current", None) }
    } else if rows.iter().any(|r| r.0 == "in") {
        ("current", None)
    } else if rows.iter().all(|r| r.0 == "out") {
        let rs: Vec<&str> = rows.iter().map(|r| r.1.as_deref().unwrap_or("unknown")).collect();
        let reason = if status == tickets::DONE || rs.contains(&"completed") {
            "completed"
        } else if rs.contains(&"reassigned") {
            "reassigned"
        } else if rs.contains(&"moved") {
            "moved"
        } else if rs.iter().all(|r| *r == "not_found") {
            "not_found"
        } else {
            "unknown"
        };
        ("history", Some(reason.to_string()))
    } else {
        ("unverified", None)
    };
    let owners: BTreeSet<i64> = rows.iter().filter(|r| r.2.as_deref() == Some("found")).filter_map(|r| r.3).collect();
    let conflict = owners.len() > 1;
    let ts = now();
    if conflict != (old_conflict != 0) {
        conn.execute("UPDATE tickets SET ownership_conflict=?2 WHERE id=?1", params![id, conflict as i64])?;
        if conflict {
            let names: Vec<String> = owners.iter().filter_map(|o| staff::name_of(conn, Some(*o)).ok().flatten()).collect();
            history(conn, id, actor, "Ownership conflict: emails are in more than one staff folder", None, None, Some(&names.join(", ")))?;
        }
    }
    if !conflict && owners.len() == 1 {
        let owner = *owners.iter().next().unwrap();
        if Some(owner) != assignee {
            let new_status = if status == tickets::UNASSIGNED { tickets::ASSIGNED } else { status.as_str() };
            conn.execute("UPDATE tickets SET assignee_staff_id=?2, assigned_at=?3, status=?4, updated_at=?3 WHERE id=?1", params![id, owner, ts, new_status])?;
            history(conn, id, actor, "Owner updated from Outlook folder (no notification)", Some("assignee"),
                staff::name_of(conn, assignee)?.as_deref(), staff::name_of(conn, Some(owner))?.as_deref())?;
            if new_status != status {
                history(conn, id, actor, "Status changed", Some("status"), Some(&status), Some(new_status))?;
            }
        }
    }
    if scope != old_scope {
        conn.execute("UPDATE tickets SET scope_state=?2, scope_reason=?3, scope_changed_at=?4 WHERE id=?1", params![id, scope, reason, ts])?;
        let action = match scope {
            "current" => "In Current Tickets (email confirmed in the selected folders)".to_string(),
            "history" => format!("Moved to Ticket History: {}", reason_label(reason.as_deref().unwrap_or("unknown"))),
            _ => "Awaiting verification of the email location".to_string(),
        };
        // "unverified -> current" happens for every new or newly confirmed ticket: not worth a history line
        if !(old_scope == "unverified" && scope == "current") {
            history(conn, id, actor, &action, Some("scope"), Some(&old_scope), Some(scope))?;
        }
    } else if scope == "history" {
        conn.execute("UPDATE tickets SET scope_reason=?2 WHERE id=?1 AND COALESCE(scope_reason,'')<>COALESCE(?2,'')", params![id, reason])?;
    }
    Ok(())
}

pub fn counts(conn: &Connection) -> AppResult<(i64, i64, i64)> {
    let g = |s: &str| conn.query_row("SELECT COUNT(*) FROM tickets WHERE scope_state=?1", [s], |r| r.get::<_, i64>(0));
    Ok((g("current")?, g("history")?, g("unverified")?))
}

/// When the folder selection changes, memberships confirmed for the old selection become unknown again
/// (nothing is deleted); the next reconciliation decides. Already-unknown/new rows are untouched.
pub fn invalidate_for_new_scope(conn: &Connection, new_key: &str) -> AppResult<usize> {
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute("UPDATE email_messages SET scope_state='unknown' WHERE COALESCE(scope_key,'')<>?1 AND scope_state<>'unknown'", [new_key])?;
    recompute_all(&tx, "system")?;
    tx.commit()?;
    Ok(n)
}

#[cfg(test)]
pub mod tests_support {
    use crate::settings::{FolderRef, Settings};

    pub fn f(id: &str, path: &str) -> FolderRef {
        FolderRef { store_id: "S1".into(), entry_id: id.into(), path: path.into() }
    }

    /// Scope = Inbox/Wenjie and its subfolders; Inbox/Completed is the completion folder.
    pub fn settings_wen_scope() -> Settings {
        let mut s = Settings::default();
        s.folder = Some(f("F-WEN", "Inbox/Wenjie"));
        s.subfolder_mode = "all".into();
        s.archive_folder = Some(f("F-DONE", "Inbox/Completed"));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{f, settings_wen_scope as settings};
    use super::*;
    use crate::staff::tests::input;
    use crate::tickets::tests::{conn, msg};
    use crate::tickets::{get, ingest};

    fn folder(id: &str, path: &str, anc: &[&str]) -> InvFolder {
        InvFolder { entry_id: id.into(), store_id: "S1".into(), path: path.into(), ancestors: anc.iter().map(|s| s.to_string()).collect() }
    }

    fn inv(key: &str, complete: bool, items: &[(&str, &str, &str)]) -> Inventory {
        Inventory {
            scope_key: key.into(),
            complete,
            folders: vec![folder("F-WEN", "Inbox/Wenjie", &["F-INBOX", "F-ROOT"]), folder("F-WEN-SUB", "Inbox/Wenjie/Sub", &["F-WEN", "F-INBOX", "F-ROOT"])],
            items: items.iter().map(|(imid, entry, fid)| InvItem { imid: imid.to_string(), entry_id: entry.to_string(), folder_entry_id: fid.to_string() }).collect(),
            errors: vec![],
        }
    }

    fn scope_of(c: &Connection, id: i64) -> (String, Option<String>) {
        c.query_row("SELECT scope_state, scope_reason FROM tickets WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[test]
    fn scope_membership_rules() {
        let c = cfg(&settings());
        let ch = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(in_scope(&c, &ch(&["F-WEN", "F-INBOX"])));
        assert!(in_scope(&c, &ch(&["F-WEN-SUB", "F-WEN", "F-INBOX"])), "subfolders included");
        assert!(!in_scope(&c, &ch(&["F-AAR", "F-INBOX"])));
        let mut s = settings();
        s.archive_folder = Some(f("F-WEN-SUB", "Inbox/Wenjie/Sub"));
        let c2 = cfg(&s);
        assert!(!in_scope(&c2, &ch(&["F-WEN-SUB", "F-WEN"])), "completion folder subtree always excluded");
        assert_ne!(c.key, c2.key);
        s.subfolder_mode = "none".into();
        assert!(!in_scope(&cfg(&s), &ch(&["F-LEAF", "F-WEN"])), "subfolders only when selected");
    }

    #[test]
    fn verification_moves_tickets_between_current_history_and_back() {
        let c = conn();
        let s = settings();
        let sc = cfg(&s);
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org"), msg(2, "C2", "Subtype", "missing the subtype", "b@b.org")], &s).unwrap();
        assert_eq!(scope_of(&c, 1).0, "current", "freshly imported email is in scope");
        // email 1 moved by hand into a subfolder (EntryID changed), email 2 moved out of the scope
        let r = apply_inventory(&c, &sc, &inv(&sc.key, true, &[("<m1@x>", "E1-NEW", "F-WEN-SUB")])).unwrap();
        assert!(r.applied);
        assert_eq!(r.newly_out, 1);
        assert_eq!(scope_of(&c, 1).0, "current", "moving within the scope keeps the ticket visible");
        let e1: String = c.query_row("SELECT entry_id FROM email_messages WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(e1, "E1-NEW", "location followed without a duplicate");
        assert_eq!(scope_of(&c, 2).0, "history");
        // where did email 2 go? located in a non-staff folder
        apply_locate(&c, &sc, &[LocateResult { message_id: r.to_locate[0].message_id, status: "found".into(), entry_id: "E2-X".into(), store_id: "S1".into(),
            folder_entry_id: "F-OTHER".into(), folder_path: "Inbox/Other".into(), ancestors: vec!["F-INBOX".into()], error: None }]).unwrap();
        assert_eq!(scope_of(&c, 2), ("history".into(), Some("moved".into())));
        // it comes back into the scope: same ticket returns, no duplicate
        apply_inventory(&c, &sc, &inv(&sc.key, true, &[("<m1@x>", "E1-NEW", "F-WEN-SUB"), ("<m2@x>", "E2-BACK", "F-WEN")])).unwrap();
        assert_eq!(scope_of(&c, 2).0, "current");
        let n: i64 = c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn incomplete_or_stale_scans_never_remove_tickets() {
        let c = conn();
        let s = settings();
        let sc = cfg(&s);
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org")], &s).unwrap();
        // a failed/partial inventory that does not contain the email: membership is kept
        let r = apply_inventory(&c, &sc, &inv(&sc.key, false, &[])).unwrap();
        assert!(r.applied);
        assert_eq!(scope_of(&c, 1).0, "current");
        // results for an older folder selection are discarded (folder changed during the scan)
        let r = apply_inventory(&c, &sc, &inv("old-selection-key", true, &[])).unwrap();
        assert!(!r.applied && r.discarded.is_some());
        assert_eq!(scope_of(&c, 1).0, "current");
        // locate error after a complete inventory: outside scope, but the location stays unverified
        let r = apply_inventory(&c, &sc, &inv(&sc.key, true, &[])).unwrap();
        apply_locate(&c, &sc, &[LocateResult { message_id: r.to_locate[0].message_id, status: "error".into(), error: Some("access denied".into()), ..Default::default() }]).unwrap();
        assert_eq!(scope_of(&c, 1), ("history".into(), Some("unknown".into())));
        let ls: String = c.query_row("SELECT location_status FROM email_messages WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(ls, "error");
    }

    #[test]
    fn wide_inventory_explains_where_emails_went() {
        let c = conn();
        let s = settings();
        let sc = cfg(&s);
        crate::staff::save(&c, &input("Teammate", "t@example.org", "F-TM")).unwrap();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org"), msg(2, "C2", "Q", "user guide", "z@b.org"), msg(3, "C3", "X", "user guide", "y@b.org")], &s).unwrap();
        let r = apply_inventory(&c, &sc, &inv(&sc.key, true, &[])).unwrap();
        assert_eq!(r.to_locate.len(), 3);
        let wide = Inventory {
            scope_key: "wide".into(),
            complete: true,
            folders: vec![folder("F-DONE", "Inbox/Completed", &["F-INBOX", "F-ROOT"]), folder("F-TM", "Inbox/Teammate", &["F-INBOX", "F-ROOT"])],
            items: vec![
                InvItem { imid: "<m1@x>".into(), entry_id: "X1".into(), folder_entry_id: "F-DONE".into() },
                InvItem { imid: "<m2@x>".into(), entry_id: "X2".into(), folder_entry_id: "F-TM".into() },
            ],
            errors: vec![],
        };
        let (found, rest) = locate_from_inventory(&wide, &r.to_locate);
        assert_eq!((found.len(), rest.len()), (2, 1));
        apply_locate(&c, &sc, &found).unwrap();
        assert_eq!(scope_of(&c, 1), ("history".into(), Some("completed".into())));
        assert_eq!(scope_of(&c, 2), ("history".into(), Some("reassigned".into())), "found in a staff folder outside the scope");
        // not found / error emails are re-checked later, not every time
        apply_locate(&c, &sc, &[LocateResult { message_id: rest[0].message_id, status: "not_found".into(), ..Default::default() }]).unwrap();
        let r2 = apply_inventory(&c, &sc, &inv(&sc.key, true, &[])).unwrap();
        assert!(r2.to_locate.is_empty(), "checked moments ago: no immediate re-lookup");
        c.execute("UPDATE email_messages SET location_checked_at='2026-01-01T00:00:00Z' WHERE location_status='not_found'", []).unwrap();
        let r3 = apply_inventory(&c, &sc, &inv(&sc.key, true, &[])).unwrap();
        assert_eq!(r3.to_locate.len(), 1, "looked up again after 6 hours");
    }

    #[test]
    fn not_found_is_not_called_deleted_and_completed_reason() {
        let c = conn();
        let s = settings();
        let sc = cfg(&s);
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org"), msg(2, "C2", "Q", "user guide", "z@b.org")], &s).unwrap();
        let r = apply_inventory(&c, &sc, &inv(&sc.key, true, &[])).unwrap();
        let ids: Vec<i64> = r.to_locate.iter().map(|x| x.message_id).collect();
        apply_locate(&c, &sc, &[
            LocateResult { message_id: ids[0], status: "not_found".into(), ..Default::default() },
            LocateResult { message_id: ids[1], status: "found".into(), entry_id: "E2-D".into(), store_id: "S1".into(), folder_entry_id: "F-DONE".into(),
                folder_path: "Inbox/Completed".into(), ancestors: vec!["F-INBOX".into()], error: None },
        ]).unwrap();
        assert_eq!(scope_of(&c, 1), ("history".into(), Some("not_found".into())));
        assert_eq!(reason_label("not_found"), "No longer found after successful reconciliation");
        assert_eq!(scope_of(&c, 2), ("history".into(), Some("completed".into())));
    }

    #[test]
    fn owner_inferred_from_staff_folder_and_conflicts_flagged() {
        let c = conn();
        let s = settings();
        let sc = cfg(&s);
        let w = crate::staff::save(&c, &input("Wenjie", "w@example.org", "F-WEN")).unwrap();
        let a = crate::staff::save(&c, &input("Aaron", "a@example.org", "F-AAR")).unwrap();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org"), msg(2, "C1", "RE: Help", "more", "a@b.org")], &s).unwrap();
        apply_inventory(&c, &sc, &inv(&sc.key, true, &[("<m1@x>", "E1", "F-WEN"), ("<m2@x>", "E2", "F-WEN-SUB")])).unwrap();
        let t = get(&c, 1).unwrap();
        assert_eq!(t.assignee_staff_id, Some(w.id), "owner from the staff folder (descendants included)");
        assert_eq!(t.status, tickets::ASSIGNED);
        assert!(!t.ownership_conflict);
        // one email filed into Aaron's folder by hand -> conflict, owner not changed silently
        let r = apply_inventory(&c, &sc, &inv(&sc.key, true, &[("<m1@x>", "E1", "F-WEN")])).unwrap();
        apply_locate(&c, &sc, &[LocateResult { message_id: r.to_locate[0].message_id, status: "found".into(), entry_id: "E2-A".into(), store_id: "S1".into(),
            folder_entry_id: "F-AAR".into(), folder_path: "Inbox/Aaron".into(), ancestors: vec!["F-INBOX".into()], error: None }]).unwrap();
        let t = get(&c, 1).unwrap();
        assert!(t.ownership_conflict);
        assert_eq!(t.assignee_staff_id, Some(w.id));
        assert_eq!(scope_of(&c, 1).0, "current", "one email still in scope: shown once");
        let _ = a;
    }

    #[test]
    fn done_ticket_is_not_reopened_by_reconciliation() {
        let c = conn();
        let s = settings();
        let sc = cfg(&s);
        crate::staff::save(&c, &input("Wenjie", "w@example.org", "F-WEN")).unwrap();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org")], &s).unwrap();
        tickets::set_done(&c, 1, "staff", "Marked Done").unwrap();
        apply_inventory(&c, &sc, &inv(&sc.key, true, &[("<m1@x>", "E1", "F-WEN")])).unwrap();
        let t = get(&c, 1).unwrap();
        assert_eq!(t.status, tickets::DONE);
        assert_eq!(scope_of(&c, 1).0, "current", "placement is separate from the workflow status");
    }

    #[test]
    fn changing_the_folder_selection_marks_membership_unverified() {
        let c = conn();
        let s = settings();
        ingest(&c, &[msg(1, "C1", "Help", "user guide please", "a@b.org")], &s).unwrap();
        let mut s2 = s.clone();
        s2.folder = Some(f("F-AAR", "Inbox/Aaron"));
        invalidate_for_new_scope(&c, &cfg(&s2).key).unwrap();
        assert_eq!(scope_of(&c, 1).0, "unverified", "not claimed current for the new selection");
        let n: i64 = c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "nothing deleted");
    }
}
