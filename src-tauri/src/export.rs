//! Reports, CSV export (Excel-compatible), database backup and restore.

use crate::catalog::catalog;
use crate::error::{AppError, AppResult};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Report {
    pub total: i64,
    pub open: i64,
    pub by_status: Vec<(String, i64)>,
    pub by_issue: Vec<(String, String, i64)>,
    pub by_school_verification: Vec<(String, i64)>,
    pub created_last_7_days: i64,
    pub created_last_30_days: i64,
    pub open_older_than_7_days: i64,
    pub followups_due: i64,
    pub outlook_drafts_created: i64,
    pub by_week: Vec<(String, i64)>,
}

fn pairs(conn: &Connection, sql: &str) -> AppResult<Vec<(String, i64)>> {
    let mut st = conn.prepare(sql)?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

fn scalar(conn: &Connection, sql: &str) -> AppResult<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

pub fn report(conn: &Connection) -> AppResult<Report> {
    let open_set = "('New','Needs Review','In Progress','Waiting for User','Waiting for Internal Action')";
    let by_issue_raw = pairs(conn, "SELECT issue_type, COUNT(*) FROM tickets GROUP BY issue_type ORDER BY 2 DESC")?;
    let by_issue = by_issue_raw
        .into_iter()
        .map(|(id, n)| {
            let name = catalog().issue(&id).map(|i| i.name.clone()).unwrap_or_else(|| id.clone());
            (id, name, n)
        })
        .collect();
    Ok(Report {
        total: scalar(conn, "SELECT COUNT(*) FROM tickets")?,
        open: scalar(conn, &format!("SELECT COUNT(*) FROM tickets WHERE status IN {open_set}"))?,
        by_status: pairs(conn, "SELECT status, COUNT(*) FROM tickets GROUP BY status ORDER BY 2 DESC")?,
        by_issue,
        by_school_verification: pairs(conn, "SELECT school_verification, COUNT(*) FROM tickets GROUP BY school_verification")?,
        created_last_7_days: scalar(conn, "SELECT COUNT(*) FROM tickets WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days')")?,
        created_last_30_days: scalar(conn, "SELECT COUNT(*) FROM tickets WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-30 days')")?,
        open_older_than_7_days: scalar(conn, &format!("SELECT COUNT(*) FROM tickets WHERE status IN {open_set} AND created_at < strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days')"))?,
        followups_due: scalar(conn, &format!("SELECT COUNT(*) FROM tickets WHERE status IN {open_set} AND next_followup_date IS NOT NULL AND next_followup_date <= date('now','localtime')"))?,
        outlook_drafts_created: scalar(conn, "SELECT COUNT(*) FROM email_drafts WHERE outlook_entry_id IS NOT NULL")?,
        by_week: pairs(conn, "SELECT strftime('%Y-W%W', created_at) w, COUNT(*) FROM tickets WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-84 days') GROUP BY w ORDER BY w")?,
    })
}

fn csv_cell(s: &str) -> String {
    // neutralize spreadsheet formula injection
    let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Export tickets (optionally filtered by status group) with all field values to CSV.
pub fn export_csv(conn: &Connection, path: &Path, status_group: &str) -> AppResult<usize> {
    let q = crate::tickets::ListQuery { group: status_group.to_string(), limit: 1000, ..Default::default() };
    let mut ids: Vec<i64> = Vec::new();
    let mut offset = 0;
    loop {
        let res = crate::tickets::list(conn, &crate::tickets::ListQuery { offset, ..q.clone() })?;
        if res.rows.is_empty() {
            break;
        }
        offset += res.rows.len() as i64;
        ids.extend(res.rows.iter().map(|r| r.id));
        if offset >= res.total {
            break;
        }
    }
    let mut field_keys: BTreeSet<String> = BTreeSet::new();
    let mut st = conn.prepare("SELECT DISTINCT field_key FROM ticket_fields")?;
    for k in st.query_map([], |r| r.get::<_, String>(0))? {
        field_keys.insert(k?);
    }
    let header = [
        "Ticket ID", "Status", "Priority", "Issue Type", "School NJIIS ID", "School Name", "School Verification", "Sender",
        "Sender Email", "Original Subject", "Assigned Staff", "Date Created", "Last Updated", "Last Email Received",
        "Last Response", "Next Follow-up", "Possible Duplicate Of", "Internal Notes", "Resolution Notes",
    ];
    let mut out = String::from("\u{feff}");
    out.push_str(&header.iter().map(|h| csv_cell(h)).chain(field_keys.iter().map(|k| csv_cell(&format!("Field: {k}")))).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for id in &ids {
        let t = crate::tickets::get(conn, *id)?;
        let fs = crate::tickets::fields(conn, *id)?;
        let issue = catalog().issue(&t.issue_type).map(|i| i.name.clone()).unwrap_or(t.issue_type.clone());
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        let mut row = vec![
            t.display_id.clone(), t.status.clone(), t.priority.clone(), issue, o(&t.school_njiis_id), o(&t.school_name),
            t.school_verification.clone(), o(&t.sender_name), o(&t.sender_email), o(&t.original_subject), o(&t.assigned_staff),
            t.created_at.clone(), t.updated_at.clone(), o(&t.last_email_received_at), o(&t.last_response_at), o(&t.next_followup_date),
            t.possible_duplicate_of.map(crate::tickets::display_id).unwrap_or_default(), o(&t.internal_notes), o(&t.resolution_notes),
        ];
        for k in &field_keys {
            row.push(fs.iter().find(|f| &f.key == k).map(|f| crate::render::display_value(&f.effective)).unwrap_or_default());
        }
        out.push_str(&row.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    std::fs::write(path, out)?;
    Ok(ids.len())
}

/// Consistent online backup (VACUUM INTO).
pub fn backup(conn: &Connection, dest: &Path) -> AppResult<()> {
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().to_string()])?;
    Ok(())
}

/// Validate a backup file before restoring it.
pub fn validate_backup(src: &Path) -> AppResult<()> {
    let c = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| AppError::coded("INVALID_BACKUP", "The selected file is not a readable database."))?;
    let ok: i64 = c
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('tickets','email_templates','ticket_fields')", [], |r| r.get(0))
        .map_err(|_| AppError::coded("INVALID_BACKUP", "The selected file is not an ASR/Audit Support Desk backup."))?;
    if ok != 3 {
        return Err(AppError::coded("INVALID_BACKUP", "The selected file is not an ASR/Audit Support Desk backup."));
    }
    let integrity: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(AppError::coded("INVALID_BACKUP", format!("Backup failed the integrity check: {integrity}")));
    }
    Ok(())
}

/// Restore a backup into the live connection (online backup API), then migrate.
pub fn restore(conn: &mut Connection, src: &Path, safety_copy: &Path) -> AppResult<()> {
    validate_backup(src)?;
    backup(conn, safety_copy)?;
    let source = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    {
        let b = rusqlite::backup::Backup::new(&source, conn)?;
        b.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
    }
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    crate::db::migrate(conn)?;
    crate::templates_store::seed_embedded(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::tickets::tests::{conn, msg};

    #[test]
    fn report_csv_backup_restore() {
        let c = conn();
        crate::tickets::ingest(&c, &[msg(1, "C1", "Subtype", "=cmd School ID: 123456, missing subtype", "a@b.org")], &Settings::default()).unwrap();
        let r = report(&c).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.open, 1);
        let dir = tempfile::tempdir().unwrap();
        let csvp = dir.path().join("t.csv");
        assert_eq!(export_csv(&c, &csvp, "all").unwrap(), 1);
        let text = std::fs::read_to_string(&csvp).unwrap();
        assert!(text.contains("T-000001"));
        assert!(text.contains("123456"));
        assert!(!text.contains("\n=cmd"));

        let bk = dir.path().join("b.db");
        backup(&c, &bk).unwrap();
        validate_backup(&bk).unwrap();
        // change live data then restore
        let mut live = crate::db::open(&dir.path().join("live.db")).unwrap();
        crate::templates_store::seed_embedded(&live).unwrap();
        restore(&mut live, &bk, &dir.path().join("safety.db")).unwrap();
        let n: i64 = live.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert!(dir.path().join("safety.db").exists());
        std::fs::write(dir.path().join("junk.db"), b"not a db").unwrap();
        assert!(validate_backup(&dir.path().join("junk.db")).is_err());
    }
}
