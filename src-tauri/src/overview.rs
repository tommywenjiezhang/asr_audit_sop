//! Manager overview: ticket counts per staff member.
//!
//! Ownership counts (Unassigned / Assigned / In Progress) come only from tickets in Current Tickets, i.e.
//! emails confirmed by a successful reconciliation of the selected folders. Staff folders outside the
//! selected scope are not reconciled by this computer and are listed as such. Done counts are this
//! computer's local records for the chosen completion date range (not synchronized between computers).

use crate::error::AppResult;
use crate::settings::Settings;
use crate::staff;
use crate::tickets::{ASSIGNED, DONE, IN_PROGRESS, REVIEW, UNASSIGNED};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OverviewRow {
    pub staff_id: Option<i64>,
    pub name: String,
    pub active: bool,
    pub folder_path: Option<String>,
    /// staff folder lies inside the selected scope (its current tickets are reconciled here)
    pub folder_in_scope: bool,
    pub unassigned: i64,
    pub assigned: i64,
    pub in_progress: i64,
    pub review: i64,
    pub done: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub scope_summary: String,
    pub last_reconciled_at: Option<String>,
    pub done_from: String,
    pub done_to: String,
    pub rows: Vec<OverviewRow>,
    pub awaiting_verification: i64,
    pub notes: Vec<String>,
}

fn path_in_scope(s: &Settings, path: &str) -> bool {
    if let Some(a) = &s.archive_folder {
        if path == a.path || path.starts_with(&format!("{}/", a.path)) {
            return false;
        }
    }
    s.monitored().iter().any(|(f, subs)| path == f.path || (*subs && path.starts_with(&format!("{}/", f.path))))
}

/// `done_from` / `done_to`: UTC instants (from inclusive, to exclusive); empty = no limit.
pub fn overview(conn: &Connection, s: &Settings, done_from: &str, done_to: &str) -> AppResult<Overview> {
    let count = |assignee: Option<i64>, status: &str, current_only: bool| -> AppResult<i64> {
        let who = match assignee { Some(_) => "assignee_staff_id = ?1", None => "assignee_staff_id IS NULL AND ?1 IS NULL" };
        let mut sql = format!("SELECT COUNT(*) FROM tickets WHERE {who} AND status = ?2");
        if current_only {
            sql.push_str(" AND scope_state = 'current'");
        } else {
            sql.push_str(" AND (?3 = '' OR completed_at >= ?3) AND (?4 = '' OR completed_at < ?4)");
        }
        let n: i64 = if current_only {
            conn.query_row(&sql, params![assignee, status], |r| r.get(0))?
        } else {
            conn.query_row(&sql, params![assignee, status, done_from, done_to], |r| r.get(0))?
        };
        Ok(n)
    };
    let mut rows = Vec::new();
    rows.push(OverviewRow {
        staff_id: None,
        name: "Unassigned".into(),
        active: true,
        folder_path: None,
        folder_in_scope: true,
        unassigned: count(None, UNASSIGNED, true)?,
        assigned: 0,
        in_progress: count(None, IN_PROGRESS, true)?,
        review: count(None, REVIEW, true)?,
        done: count(None, DONE, false)?,
    });
    let mut notes = Vec::new();
    let mut outside: Vec<String> = Vec::new();
    for st in staff::list(conn, true)? {
        let inside = path_in_scope(s, &st.folder_path);
        if !inside && st.active {
            outside.push(format!("{} ({})", st.display_name, st.folder_path));
        }
        let row = OverviewRow {
            staff_id: Some(st.id),
            name: st.display_name.clone(),
            active: st.active,
            folder_path: Some(st.folder_path.clone()),
            folder_in_scope: inside,
            unassigned: 0,
            assigned: count(Some(st.id), ASSIGNED, true)?,
            in_progress: count(Some(st.id), IN_PROGRESS, true)?,
            review: count(Some(st.id), REVIEW, true)?,
            done: count(Some(st.id), DONE, false)?,
        };
        if st.active || row.assigned + row.in_progress + row.review + row.done > 0 {
            rows.push(row);
        }
    }
    if !outside.is_empty() {
        notes.push(format!(
            "Not a complete team report: these staff folders are outside the selected folders, so their current tickets are not reconciled on this computer: {}.",
            outside.join(", ")
        ));
    }
    notes.push("Assigned / In Progress / Unassigned: tickets in Current Tickets (emails confirmed in the selected folders at the last successful check).".into());
    notes.push("Done: this computer's local records in the selected completion date range. Statuses, notes and history are not synchronized between computers.".into());
    let awaiting: i64 = conn.query_row("SELECT COUNT(*) FROM tickets WHERE scope_state='unverified'", [], |r| r.get(0))?;
    if awaiting > 0 {
        notes.push(format!("{awaiting} ticket(s) are awaiting verification and are not counted as current."));
    }
    Ok(Overview {
        scope_summary: if s.folder.is_some() { s.scan_summary() } else { "No folder selected".into() },
        last_reconciled_at: crate::db::get_setting(conn, "scope_last_ok_at")?,
        done_from: done_from.into(),
        done_to: done_to.into(),
        rows,
        awaiting_verification: awaiting,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::tests_support::settings_wen_scope;
    use crate::staff::tests::input;
    use crate::tickets::tests::{conn, msg};

    #[test]
    fn counts_by_staff_with_scope_labels() {
        let c = conn();
        let s = settings_wen_scope();
        let mut wi = input("Wenjie", "w@example.org", "F-WEN");
        wi.folder_path = "Inbox/Wenjie".into();
        let w = crate::staff::save(&c, &wi).unwrap();
        let mut ai = input("Aaron", "a@example.org", "F-AAR");
        ai.folder_path = "Inbox/Aaron".into();
        crate::staff::save(&c, &ai).unwrap();
        crate::tickets::ingest(&c, &[msg(1, "C1", "Help", "user guide", "a@b.org"), msg(2, "C2", "Q", "subtype missing", "b@b.org"), msg(3, "C3", "X", "user guide", "c@b.org")], &s).unwrap();
        c.execute("UPDATE tickets SET assignee_staff_id=?1, status='In Progress' WHERE id=2", [w.id]).unwrap();
        c.execute("UPDATE tickets SET assignee_staff_id=?1, status='Done', completed_at='2026-11-10T12:00:00Z' WHERE id=3", [w.id]).unwrap();
        let o = overview(&c, &s, "2026-11-01T00:00:00Z", "2026-11-30T00:00:00Z").unwrap();
        let un = &o.rows[0];
        assert_eq!((un.name.as_str(), un.unassigned), ("Unassigned", 1));
        let wr = o.rows.iter().find(|r| r.name == "Wenjie").unwrap();
        assert_eq!((wr.in_progress, wr.done, wr.folder_in_scope), (1, 1, true));
        let ar = o.rows.iter().find(|r| r.name == "Aaron").unwrap();
        assert!(!ar.folder_in_scope);
        assert!(o.notes[0].contains("Not a complete team report") && o.notes[0].contains("Aaron"));
        let o2 = overview(&c, &s, "2026-11-11T00:00:00Z", "").unwrap();
        assert_eq!(o2.rows.iter().find(|r| r.name == "Wenjie").unwrap().done, 0, "date range applies to Done");
    }
}
