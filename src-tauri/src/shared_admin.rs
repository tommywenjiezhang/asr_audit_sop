//! Administration of the shared database: controlled schema upgrade with a verified backup, integrity
//! verification (read-only), and an on-demand verified backup copy.
//!
//! Nothing here deletes or rewrites case data. The upgrade only adds tables/indexes (idempotent) and is
//! refused while another computer has the file open, so the backup is a consistent copy.

use crate::shared_store::{self as ss, col_i, col_s, ErrKind, SResult, SharedConn, SharedError};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpgradeReport {
    pub from_version: i64,
    pub to_version: i64,
    pub backup_path: String,
    pub backup_sha256: String,
    pub tables_added: Vec<String>,
}

fn lock_file(path: &Path) -> std::path::PathBuf {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    path.with_extension(if ext == "mdb" { "ldb" } else { "laccdb" })
}

/// Upgrade an older ASR shared database to the current schema in place. Steps: read-only identity check →
/// refuse if open elsewhere (lock file) → verified backup next to the file → exclusive connection →
/// maintenance on → idempotent DDL → version/protocol recorded → maintenance off.
pub fn upgrade_with_backup(path: &Path, actor: &str) -> SResult<UpgradeReport> {
    let from = {
        let mut ro = ss::open(path, true)?;
        ss::identity(ro.as_mut())?
    };
    if from.schema_version > ss::SHARED_SCHEMA_VERSION {
        return Err(SharedError::new(ErrKind::Incompatible, "The shared database is newer than this app. Install the newer app version instead."));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let access = ext == "accdb" || ext == "mdb";
    if access && lock_file(path).exists() {
        return Err(SharedError::new(ErrKind::Locked, "The shared database is open on a computer (lock file present). Ask everyone to close the app (or switch to Standalone), then upgrade."));
    }
    let (backup, sha) = ss::verified_backup(path)?;
    let mut c: Box<dyn SharedConn> = if access { Box::new(ss::AccessConn::open_with(path, false, "Exclusive=1;")?) } else { ss::open(path, false)? };
    let before = ss::missing_tables(c.as_mut())?;
    let id = ss::upgrade_schema(c.as_mut(), actor).map_err(|e| SharedError { message: format!("{} — the database stays in maintenance mode; the verified backup is {}", e.message, backup.display()), ..e })?;
    let after = ss::missing_tables(c.as_mut())?;
    Ok(UpgradeReport {
        from_version: from.schema_version,
        to_version: id.schema_version,
        backup_path: backup.display().to_string(),
        backup_sha256: sha,
        tables_added: before.into_iter().filter(|t| !after.contains(t)).collect(),
    })
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct IntegrityReport {
    pub database_id: String,
    pub display_name: String,
    pub environment: String,
    pub schema_version: i64,
    pub missing_tables: Vec<String>,
    /// (table, rows)
    pub counts: Vec<(String, i64)>,
    /// (check, problem rows) — every entry should be 0
    pub checks: Vec<(String, i64)>,
    pub ok: bool,
}

/// Read-only integrity verification: required tables, row counts and referential checks.
pub fn integrity(c: &mut dyn SharedConn) -> SResult<IntegrityReport> {
    let id = ss::identity(c)?;
    let mut r = IntegrityReport { database_id: id.database_id, display_name: id.display_name, environment: id.environment, schema_version: id.schema_version, ..Default::default() };
    r.missing_tables = ss::missing_tables(c)?;
    let mut n = |c: &mut dyn SharedConn, sql: &str| -> SResult<i64> { Ok(c.query(sql, &[])?.first().map(|x| col_i(x, 0)).unwrap_or(0)) };
    for t in ss::REQUIRED_TABLES {
        if r.missing_tables.iter().any(|m| m == t) {
            continue;
        }
        let v = n(c, &format!("SELECT COUNT(*) FROM {t}"))?;
        r.counts.push((t.to_string(), v));
    }
    let checks: &[(&str, &str)] = &[
        ("Case emails without a case", "SELECT COUNT(*) FROM TicketEmails te LEFT JOIN Tickets t ON t.ticket_id = te.ticket_id WHERE t.ticket_id IS NULL"),
        ("Case emails without an email", "SELECT COUNT(*) FROM TicketEmails te LEFT JOIN Emails e ON e.email_id = te.email_id WHERE e.email_id IS NULL"),
        ("Notes without a case", "SELECT COUNT(*) FROM TicketNotes n LEFT JOIN Tickets t ON t.ticket_id = n.ticket_id WHERE t.ticket_id IS NULL"),
        ("Fields without a case", "SELECT COUNT(*) FROM TicketFields f LEFT JOIN Tickets t ON t.ticket_id = f.ticket_id WHERE t.ticket_id IS NULL"),
        ("Ownership periods without a case", "SELECT COUNT(*) FROM OwnershipHistory o LEFT JOIN Tickets t ON t.ticket_id = o.ticket_id WHERE t.ticket_id IS NULL"),
        ("Activity without a case", "SELECT COUNT(*) FROM ActivityLog a LEFT JOIN Tickets t ON t.ticket_id = a.ticket_id WHERE t.ticket_id IS NULL"),
        ("Combined cases pointing to a missing case", "SELECT COUNT(*) FROM Tickets s LEFT JOIN Tickets t ON t.ticket_id = s.merged_into WHERE s.merged_into IS NOT NULL AND s.merged_into <> '' AND t.ticket_id IS NULL"),
        ("Cases combined into themselves", "SELECT COUNT(*) FROM Tickets WHERE merged_into = ticket_id"),
        ("Aliases pointing to a missing case", "SELECT COUNT(*) FROM TicketAliases a LEFT JOIN Tickets t ON t.ticket_id = a.ticket_id WHERE t.ticket_id IS NULL"),
        ("Owners that are not staff records", "SELECT COUNT(*) FROM Tickets t LEFT JOIN Staff s ON s.staff_id = t.owner_staff_id WHERE t.owner_staff_id IS NOT NULL AND s.staff_id IS NULL"),
        ("Duplicate email matching keys", "SELECT COUNT(*) FROM (SELECT match_key FROM Emails GROUP BY match_key HAVING COUNT(*) > 1) x"),
        ("Duplicate case references", "SELECT COUNT(*) FROM (SELECT ticket_ref FROM Tickets GROUP BY ticket_ref HAVING COUNT(*) > 1) x"),
        ("Cases with more than one open ownership period", "SELECT COUNT(*) FROM (SELECT ticket_id FROM OwnershipHistory WHERE ended_at IS NULL GROUP BY ticket_id HAVING COUNT(*) > 1) x"),
        ("Unknown statuses", "SELECT COUNT(*) FROM Tickets WHERE ticket_status NOT IN ('Unassigned','Assigned','In Progress','Review','Done')"),
    ];
    for (label, sql) in checks {
        let v = n(c, sql)?;
        r.checks.push((label.to_string(), v));
    }
    if !r.missing_tables.iter().any(|m| m == "OutlookOperations") && r.schema_version >= 2 {
        let v = n(c, "SELECT COUNT(*) FROM OutlookOperations o LEFT JOIN Tickets t ON t.ticket_id = o.ticket_id WHERE t.ticket_id IS NULL")?;
        r.checks.push(("Outlook runs without a case".into(), v));
    }
    // a merge cycle would make a case unreachable: follow every redirect chain (bounded)
    let redirects: std::collections::HashMap<String, String> =
        c.query("SELECT ticket_id, merged_into FROM Tickets WHERE merged_into IS NOT NULL AND merged_into <> ''", &[])?.iter().map(|x| (col_s(x, 0), col_s(x, 1))).collect();
    let cycles = redirects
        .keys()
        .filter(|start| {
            let mut cur = (*start).clone();
            for _ in 0..64 {
                match redirects.get(&cur) {
                    Some(next) if next == *start => return true,
                    Some(next) => cur = next.clone(),
                    None => return false,
                }
            }
            true
        })
        .count() as i64;
    r.checks.push(("Circular merges".into(), cycles));
    r.ok = r.missing_tables.is_empty() && r.checks.iter().all(|(_, v)| *v == 0);
    Ok(r)
}

/// Verified copy of the shared database file into `dest_dir` (or next to it). A copy taken while someone
/// writes is detected (hash before/after differ) and refused rather than kept.
pub fn backup_now(path: &Path, dest_dir: Option<&Path>) -> SResult<(String, String)> {
    let (tmp, sha) = ss::verified_backup(path)?;
    match dest_dir {
        None => Ok((tmp.display().to_string(), sha)),
        Some(d) => {
            std::fs::create_dir_all(d).map_err(|e| SharedError::new(ErrKind::Unavailable, format!("backup folder: {e}")))?;
            let dest = d.join(tmp.file_name().unwrap_or_default());
            std::fs::copy(&tmp, &dest).map_err(|e| SharedError::new(ErrKind::Unavailable, format!("backup copy: {e}")))?;
            let _ = std::fs::remove_file(&tmp);
            Ok((dest.display().to_string(), sha))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_store::tests::new_shared;

    #[test]
    fn integrity_of_a_fresh_database_is_clean_and_problems_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let p = new_shared(dir.path());
        let mut c = ss::open(&p, false).unwrap();
        let r = integrity(c.as_mut()).unwrap();
        assert!(r.ok, "{r:?}");
        c.exec("INSERT INTO Tickets (ticket_id, ticket_ref, ticket_status, row_version, created_at, updated_at, merged_into) VALUES ('a','R-A','Done',1,'x','x','b')", &[]).unwrap();
        c.exec("INSERT INTO Tickets (ticket_id, ticket_ref, ticket_status, row_version, created_at, updated_at, merged_into) VALUES ('b','R-B','Done',1,'x','x','a')", &[]).unwrap();
        let r = integrity(c.as_mut()).unwrap();
        assert!(!r.ok);
        assert_eq!(r.checks.iter().find(|x| x.0 == "Circular merges").unwrap().1, 2);
    }

    #[test]
    fn upgrade_with_backup_keeps_a_verified_copy() {
        let dir = tempfile::tempdir().unwrap();
        let p = new_shared(dir.path());
        {
            let mut c = ss::open(&p, false).unwrap();
            c.exec("DROP TABLE OutlookOperations", &[]).unwrap();
            ss::set_meta(c.as_mut(), "schema_version", "1").unwrap();
        }
        let r = upgrade_with_backup(&p, "test").unwrap();
        assert_eq!((r.from_version, r.to_version), (1, ss::SHARED_SCHEMA_VERSION));
        assert_eq!(r.tables_added, vec!["OutlookOperations".to_string()]);
        assert!(std::path::Path::new(&r.backup_path).exists());
        // running it again changes nothing and still succeeds
        assert!(upgrade_with_backup(&p, "test").unwrap().tables_added.is_empty());
        let (b, _) = backup_now(&p, Some(&dir.path().join("bk"))).unwrap();
        assert!(std::path::Path::new(&b).exists());
    }
}
