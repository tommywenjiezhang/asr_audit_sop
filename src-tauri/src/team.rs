//! Team Overview (P3): workload and attention metrics over the team's cases (the shared case records on
//! this computer, any Outlook folder), plus a staff activity history.
//!
//! "Older" uses the case's first received time; "stale" means no activity of any kind for N days.

use crate::error::AppResult;
use crate::tickets::{ASSIGNED, DONE, IN_PROGRESS, REVIEW, UNASSIGNED};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct StaffLoad {
    pub staff_id: Option<i64>,
    pub name: String,
    pub active: bool,
    pub assigned: i64,
    pub in_progress: i64,
    pub review: i64,
    /// not Done
    pub unresolved: i64,
    /// open and first received more than `old_days` ago
    pub older: i64,
    /// age in days of this person's oldest open case
    pub oldest_days: Option<i64>,
    pub done_7d: i64,
    pub done_30d: i64,
    pub outlook_pending: i64,
    pub conflicts: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgingCase {
    pub ticket_id: i64,
    pub display_id: String,
    pub ticket_ref: Option<String>,
    pub status: String,
    pub owner: Option<String>,
    pub school: Option<String>,
    pub subject: Option<String>,
    pub age_days: i64,
    pub idle_days: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct TeamOverview {
    pub old_days: i64,
    pub total_open: i64,
    pub unassigned: i64,
    pub review: i64,
    pub older: i64,
    pub stale: i64,
    pub outlook_pending: i64,
    pub sync_pending: i64,
    pub sync_held: i64,
    pub conflicts_open: i64,
    pub done_7d: i64,
    pub rows: Vec<StaffLoad>,
    pub oldest: Vec<AgingCase>,
    pub shared_last_success_at: Option<String>,
    pub notes: Vec<String>,
}

fn days_ago(d: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(d)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn team_overview(conn: &Connection, old_days: i64, stale_days: i64) -> AppResult<TeamOverview> {
    let old_days = old_days.clamp(1, 365);
    let stale_days = stale_days.clamp(1, 365);
    let cutoff = days_ago(old_days);
    let stale_cut = days_ago(stale_days);
    let (d7, d30) = (days_ago(7), days_ago(30));
    let base = "merged_into IS NULL";
    let g = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> AppResult<i64> { Ok(conn.query_row(sql, p, |r| r.get(0))?) };
    let pending_sql = crate::tickets::OUTLOOK_PENDING_SQL;
    let activity = "MAX(t.updated_at, COALESCE((SELECT MAX(h.at) FROM ticket_history h WHERE h.ticket_id=t.id), ''))";
    let mut o = TeamOverview { old_days, ..Default::default() };
    o.total_open = g(&format!("SELECT COUNT(*) FROM tickets WHERE {base} AND status<>'{DONE}'"), &[])?;
    o.unassigned = g(&format!("SELECT COUNT(*) FROM tickets WHERE {base} AND status='{UNASSIGNED}'"), &[])?;
    o.review = g(&format!("SELECT COUNT(*) FROM tickets WHERE {base} AND status='{REVIEW}'"), &[])?;
    o.older = g(&format!("SELECT COUNT(*) FROM tickets WHERE {base} AND status<>'{DONE}' AND COALESCE(first_received_at, created_at) < ?1"), &[&cutoff])?;
    o.stale = g(&format!("SELECT COUNT(*) FROM tickets t WHERE t.{base} AND t.status<>'{DONE}' AND {activity} < ?1"), &[&stale_cut])?;
    o.outlook_pending = g(&format!("SELECT COUNT(*) FROM tickets t WHERE t.{base} AND ({pending_sql}) IS NOT NULL"), &[])?;
    o.done_7d = g(&format!("SELECT COUNT(*) FROM tickets WHERE {base} AND status='{DONE}' AND completed_at >= ?1"), &[&d7])?;
    let q = crate::sync_queue::summary(conn)?;
    o.sync_pending = q.pending;
    o.sync_held = q.held;
    o.conflicts_open = q.conflicts;
    o.shared_last_success_at = crate::db::get_setting(conn, "shared_last_success_at")?;

    let per = |who: &str, p: Option<i64>| -> AppResult<StaffLoad> {
        let w = |extra: &str| format!("SELECT COUNT(*) FROM tickets t WHERE t.{base} AND {who} {extra}");
        let mut r = StaffLoad::default();
        r.assigned = conn.query_row(&w(&format!("AND t.status='{ASSIGNED}'")), params![p], |x| x.get(0))?;
        r.in_progress = conn.query_row(&w(&format!("AND t.status='{IN_PROGRESS}'")), params![p], |x| x.get(0))?;
        r.review = conn.query_row(&w(&format!("AND t.status='{REVIEW}'")), params![p], |x| x.get(0))?;
        r.unresolved = conn.query_row(&w(&format!("AND t.status<>'{DONE}'")), params![p], |x| x.get(0))?;
        r.older = conn.query_row(&w(&format!("AND t.status<>'{DONE}' AND COALESCE(t.first_received_at, t.created_at) < ?2")), params![p, cutoff], |x| x.get(0))?;
        r.done_7d = conn.query_row(&w(&format!("AND t.status='{DONE}' AND t.completed_at >= ?2")), params![p, d7], |x| x.get(0))?;
        r.done_30d = conn.query_row(&w(&format!("AND t.status='{DONE}' AND t.completed_at >= ?2")), params![p, d30], |x| x.get(0))?;
        r.outlook_pending = conn.query_row(&w(&format!("AND ({pending_sql}) IS NOT NULL")), params![p], |x| x.get(0))?;
        r.conflicts = conn.query_row(&w("AND EXISTS(SELECT 1 FROM sync_conflicts c WHERE c.ticket_id=t.id AND c.status<>'resolved')"), params![p], |x| x.get(0))?;
        let oldest: Option<String> = conn.query_row(&format!("SELECT MIN(COALESCE(t.first_received_at, t.created_at)) FROM tickets t WHERE t.{base} AND {who} AND t.status<>'{DONE}'"), params![p], |x| x.get(0))?;
        r.oldest_days = oldest.and_then(|d| chrono::DateTime::parse_from_rfc3339(&d).ok()).map(|d| (chrono::Utc::now() - d.with_timezone(&chrono::Utc)).num_days());
        Ok(r)
    };
    let mut un = per("t.assignee_staff_id IS NULL AND ?1 IS NULL", None)?;
    un.name = "Unassigned".into();
    un.active = true;
    o.rows.push(un);
    for st in crate::staff::list(conn, true)? {
        let mut r = per("t.assignee_staff_id = ?1", Some(st.id))?;
        r.staff_id = Some(st.id);
        r.name = st.display_name.clone();
        r.active = st.active;
        if st.active || r.unresolved + r.done_30d > 0 {
            o.rows.push(r);
        }
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT t.id, t.ticket_ref, t.status, (SELECT display_name FROM staff s WHERE s.id=t.assignee_staff_id), t.school_name, t.original_subject,
                COALESCE(t.first_received_at, t.created_at), {activity}
         FROM tickets t WHERE t.{base} AND t.status<>'{DONE}' ORDER BY COALESCE(t.first_received_at, t.created_at) ASC LIMIT 15"
    ))?;
    let now = chrono::Utc::now();
    let age = |s: &str| chrono::DateTime::parse_from_rfc3339(s).map(|d| (now - d.with_timezone(&chrono::Utc)).num_days()).unwrap_or(0);
    o.oldest = stmt
        .query_map([], |r| {
            let id: i64 = r.get(0)?;
            let recv: String = r.get(6)?;
            let act: String = r.get(7)?;
            Ok(AgingCase { ticket_id: id, display_id: crate::tickets::display_id(id), ticket_ref: r.get(1)?, status: r.get(2)?, owner: r.get(3)?, school: r.get(4)?, subject: r.get(5)?, age_days: age(&recv), idle_days: age(&act) })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    o.notes.push(format!("Counts cover the team's cases on this computer (shared records, any Outlook folder). Older = first received more than {old_days} days ago; stale = no activity for {stale_days} days."));
    if o.sync_pending + o.sync_held > 0 {
        o.notes.push(format!("{} change(s) made on this computer are not in the shared database yet; colleagues do not see them until they synchronize.", o.sync_pending + o.sync_held));
    }
    Ok(o)
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityRow {
    pub at: String,
    pub actor: String,
    pub action: String,
    pub ticket_id: i64,
    pub display_id: String,
    pub ticket_ref: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivitySummary {
    pub actor: String,
    pub actions_7d: i64,
    pub cases_7d: i64,
    pub last_at: Option<String>,
}

/// Staff activity history: per-person counts for the last 7 days and the newest actions (optionally of
/// one person). System and synchronization bookkeeping rows are left out.
pub fn staff_activity(conn: &Connection, actor: &str, limit: i64) -> AppResult<(Vec<ActivitySummary>, Vec<ActivityRow>)> {
    let d7 = days_ago(7);
    let filter = "h.actor NOT IN ('system','sync','staff') AND h.action NOT LIKE 'Opened%'";
    let mut st = conn.prepare(&format!(
        "SELECT h.actor, SUM(CASE WHEN h.at >= ?1 THEN 1 ELSE 0 END), COUNT(DISTINCT CASE WHEN h.at >= ?1 THEN h.ticket_id END), MAX(h.at)
         FROM ticket_history h WHERE {filter} GROUP BY h.actor ORDER BY MAX(h.at) DESC LIMIT 50"
    ))?;
    let summary = st.query_map([&d7], |r| Ok(ActivitySummary { actor: r.get(0)?, actions_7d: r.get(1)?, cases_7d: r.get(2)?, last_at: r.get(3)? }))?.collect::<Result<Vec<_>, _>>()?;
    let mut st = conn.prepare(&format!(
        "SELECT h.at, h.actor, h.action, h.ticket_id, t.ticket_ref, COALESCE(h.new_value, h.old_value)
         FROM ticket_history h JOIN tickets t ON t.id = h.ticket_id
         WHERE {filter} AND (?1 = '' OR h.actor = ?1) ORDER BY h.at DESC, h.id DESC LIMIT ?2"
    ))?;
    let rows = st
        .query_map(params![actor, limit.clamp(1, 1000)], |r| {
            let id: i64 = r.get(3)?;
            let detail: Option<String> = r.get(5)?;
            Ok(ActivityRow { at: r.get(0)?, actor: r.get(1)?, action: r.get(2)?, ticket_id: id, display_id: crate::tickets::display_id(id), ticket_ref: r.get(4)?, detail: detail.map(|d| d.chars().take(200).collect()) })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((summary, rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::tests::{conn, msg};

    #[test]
    fn team_counts_cover_all_cases_and_aging() {
        let c = conn();
        let s = crate::settings::Settings::default();
        let w = crate::staff::save(&c, &crate::staff::tests::input("Wenjie", "w@example.org", "F-W")).unwrap();
        crate::tickets::ingest(&c, &[msg(1, "C1", "Help", "user guide", "a@b.org"), msg(2, "C2", "Q", "subtype", "b@b.org"), msg(3, "C3", "X", "x", "c@b.org")], &s).unwrap();
        c.execute("UPDATE tickets SET assignee_staff_id=?1, status='In Progress', first_received_at='2020-01-01T00:00:00Z' WHERE id=2", [w.id]).unwrap();
        c.execute("UPDATE tickets SET assignee_staff_id=?1, status='Done', completed_at=?2 WHERE id=3", params![w.id, crate::db::now()]).unwrap();
        let o = team_overview(&c, 14, 7).unwrap();
        assert_eq!((o.total_open, o.unassigned, o.done_7d), (2, 1, 1));
        let wr = o.rows.iter().find(|r| r.name == "Wenjie").unwrap();
        assert_eq!((wr.in_progress, wr.unresolved, wr.older, wr.done_7d), (1, 1, 1, 1));
        assert!(wr.oldest_days.unwrap() > 300);
        assert_eq!(o.oldest[0].ticket_id, 2, "oldest open case first");
        crate::tickets::history(&c, 1, "Wenjie", "Claimed", None, None, None).unwrap();
        let (sum, rows) = staff_activity(&c, "Wenjie", 10).unwrap();
        assert_eq!(sum.iter().find(|s| s.actor == "Wenjie").unwrap().actions_7d, 1);
        assert_eq!(rows.len(), 1);
    }
}
