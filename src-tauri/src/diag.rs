//! Developer diagnostics: a bounded, redacted event log kept in the local SQLite database.
//!
//! - Redaction happens BEFORE anything is written: email addresses, phone numbers, long digit runs
//!   (school/NJIIS identifiers), connection-string secrets and quoted subjects are masked.
//! - Recording never fails the caller: errors are swallowed (and traced), so a logging problem can never
//!   stop a ticket from being saved or a synchronization from running.
//! - Diagnostics are separate from case activity history (ticket_history / ActivityLog).

use regex::Regex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::LazyLock;

/// Maximum number of events kept (oldest removed first).
pub const MAX_EVENTS: i64 = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+'\-]+@[A-Za-z0-9\-]+(\.[A-Za-z0-9\-]+)+").unwrap());
static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(?\b\d{3}\)?[\s.\-]\d{3}[\s.\-]\d{4}\b").unwrap());
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{4,}\b").unwrap());
static SECRET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(pwd|password|uid|user id|secret|token|apikey|api_key)\s*=\s*[^;\s]+").unwrap());
static DRIVER_CS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)driver=\{[^}]*\};[^\n]*").unwrap());
static SUBJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)(subject\s*[:=]\s*)("[^"]*"|[^\n;]+)"#).unwrap());
/// C:\Users\<name>\... profile paths (the Windows user name is personal)
static PROFILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)([a-z]:\\users\\)[^\\/\s]+").unwrap());
/// DOMAIN\user accounts written as "account=..." / "user=..."
static ACCOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(account|user|username|windows_account)\s*[=:]\s*[^\s;,]+").unwrap());

/// Mask personal and secret values. Applied to every summary and detail before writing or copying.
pub fn redact(s: &str) -> String {
    let s = DRIVER_CS.replace_all(s, "[connection string removed]");
    let s = SECRET.replace_all(&s, "$1=[removed]");
    let s = SUBJECT.replace_all(&s, "${1}[subject removed]");
    let s = PROFILE.replace_all(&s, "${1}[user]");
    let s = ACCOUNT.replace_all(&s, "$1=[removed]");
    let s = EMAIL.replace_all(&s, "[email]");
    let s = PHONE.replace_all(&s, "[phone]");
    // long digit runs (NJIIS / school IDs, ZIP+4, account numbers); timestamps and short codes survive
    let s = DIGITS.replace_all(&s, |c: &regex::Captures| {
        let d = &c[0];
        // keep 4-digit years and negative provider codes like -1811 readable
        if d.len() == 4 && (d.starts_with("19") || d.starts_with("20")) { d.to_string() } else if d.len() <= 4 { d.to_string() } else { "[number]".to_string() }
    });
    let mut out = s.to_string();
    if out.chars().count() > 2000 {
        out = out.chars().take(2000).collect::<String>() + "…";
    }
    out
}

/// Record an event. Never returns an error.
pub fn record(conn: &Connection, severity: Severity, component: &str, code: &str, summary: &str, detail: &str, correlation: &str) {
    let r = (|| -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO diag_events(at, severity, component, code, summary, detail, correlation) VALUES(?1,?2,?3,NULLIF(?4,''),?5,NULLIF(?6,''),NULLIF(?7,''))",
            params![crate::db::now(), severity.as_str(), component, code, redact(summary), redact(detail), redact(correlation)],
        )?;
        let id = conn.last_insert_rowid();
        if id % 100 == 0 {
            conn.execute("DELETE FROM diag_events WHERE id <= ?1", [id - MAX_EVENTS])?;
        }
        Ok(())
    })();
    if let Err(e) = r {
        tracing::warn!(error = %e, component, code, "diagnostic event could not be stored");
    }
    match severity {
        Severity::Error => tracing::error!(component, code, "{}", redact(summary)),
        Severity::Warning => tracing::warn!(component, code, "{}", redact(summary)),
        Severity::Info => tracing::info!(component, code, "{}", redact(summary)),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagEvent {
    pub id: i64,
    pub at: String,
    pub severity: String,
    pub component: String,
    pub code: Option<String>,
    pub summary: String,
    pub detail: Option<String>,
    pub correlation: Option<String>,
}

pub fn recent(conn: &Connection, min_severity: &str, limit: i64) -> rusqlite::Result<Vec<DiagEvent>> {
    let sev = match min_severity {
        "error" => "('error')",
        "warning" => "('warning','error')",
        _ => "('info','warning','error')",
    };
    let mut st = conn.prepare(&format!(
        "SELECT id, at, severity, component, code, summary, detail, correlation FROM diag_events WHERE severity IN {sev} ORDER BY id DESC LIMIT ?1"
    ))?;
    let v = st
        .query_map([limit.clamp(1, MAX_EVENTS)], |r| {
            Ok(DiagEvent { id: r.get(0)?, at: r.get(1)?, severity: r.get(2)?, component: r.get(3)?, code: r.get(4)?, summary: r.get(5)?, detail: r.get(6)?, correlation: r.get(7)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// Header of a copied log: enough context for a developer, nothing personal.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiagHeader {
    pub app_version: String,
    pub local_schema: i64,
    pub shared_mode: String,
    pub shared_state: String,
    pub shared_database: Option<String>,
    pub shared_environment: Option<String>,
    pub shared_schema: Option<i64>,
    pub last_success_at: Option<String>,
    pub pending: i64,
    pub held: i64,
    pub conflicts: i64,
    pub failed: i64,
    pub driver: Option<String>,
    pub device: String,
}

pub fn header(conn: &Connection, app_version: &str, shared: Option<&crate::shared_sync::SharedStatus>) -> DiagHeader {
    let q = crate::sync_queue::summary(conn).unwrap_or_default();
    DiagHeader {
        app_version: app_version.into(),
        local_schema: crate::db::schema_version(conn).unwrap_or(0),
        shared_mode: crate::sync_queue::mode(conn),
        shared_state: shared.map(|s| s.state.clone()).unwrap_or_else(|| "unknown".into()),
        shared_database: shared.and_then(|s| s.database_name.clone()).or_else(|| crate::db::get_setting(conn, "shared_db_name").ok().flatten()),
        shared_environment: shared.and_then(|s| s.environment.clone()),
        shared_schema: shared.and_then(|s| s.schema_version),
        last_success_at: crate::db::get_setting(conn, "shared_last_success_at").ok().flatten(),
        pending: q.pending,
        held: q.held,
        conflicts: q.conflicts,
        failed: q.rejected,
        driver: shared.and_then(|s| s.driver.clone()),
        // only the first part of the random device id: enough to tell computers apart in a pilot
        device: crate::sync_queue::device_id(conn).map(|d| d.chars().take(8).collect()).unwrap_or_default(),
    }
}

/// Newest error event (for "Go to latest error").
pub fn latest_error(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT MAX(id) FROM diag_events WHERE severity='error'", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten()
}

/// Plain text for the clipboard. Redaction is applied again here (defense in depth), so a value that
/// slipped past redaction at write time is still masked in what leaves the computer.
pub fn copy_text(conn: &Connection, h: &DiagHeader, ids: Option<&[i64]>, min_severity: &str, limit: i64) -> String {
    let mut events = recent(conn, min_severity, limit).unwrap_or_default();
    if let Some(sel) = ids {
        events.retain(|e| sel.contains(&e.id));
    }
    events.reverse(); // oldest first reads naturally
    let mut out = String::new();
    out.push_str("ASR/Audit Support Desk - Diagnostics\n");
    out.push_str(&format!("Generated: {}\n", crate::db::now()));
    out.push_str(&format!("App version: {} | local schema v{} | device {}\n", h.app_version, h.local_schema, h.device));
    out.push_str(&format!(
        "Shared database: mode {} | state {} | database {} ({}) schema {} | driver {}\n",
        h.shared_mode,
        h.shared_state,
        h.shared_database.clone().unwrap_or_else(|| "-".into()),
        h.shared_environment.clone().unwrap_or_else(|| "-".into()),
        h.shared_schema.map(|v| format!("v{v}")).unwrap_or_else(|| "-".into()),
        h.driver.clone().unwrap_or_else(|| "-".into())
    ));
    out.push_str(&format!("Last successful sync: {} | pending {} | held {} | conflicts {} | failed {}\n",
        h.last_success_at.clone().unwrap_or_else(|| "never".into()), h.pending, h.held, h.conflicts, h.failed));
    out.push_str(&format!("Events: {} ({}){}\n", events.len(), if min_severity.is_empty() { "all" } else { min_severity }, if ids.is_some() { ", selected" } else { "" }));
    out.push_str("----\n");
    for e in &events {
        out.push_str(&format!("{} [{}] {} {} - {}", e.at, e.severity.to_uppercase(), e.component, e.code.clone().unwrap_or_default(), redact(&e.summary)));
        if let Some(d) = e.detail.as_deref().filter(|d| !d.is_empty()) {
            out.push_str(&format!(" | {}", redact(d)));
        }
        if let Some(c) = e.correlation.as_deref().filter(|c| !c.is_empty()) {
            out.push_str(&format!(" | ref {}", redact(c)));
        }
        out.push('\n');
    }
    out.push_str("----\nPersonal data (email addresses, phone numbers, school and NJIIS IDs, subjects, Windows user names, connection strings) is removed. Email text and Copilot prompts are never logged.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_masks_personal_values_and_secrets() {
        let raw = "Driver={Microsoft Access Driver (*.mdb, *.accdb)};DBQ=\\\\srv\\x.accdb;PWD=hunter2 failed for jane.doe@school.org phone 609-555-0100 NJIIS 0012345 Subject: Help with ASR for Maple";
        let r = redact(raw);
        for bad in ["hunter2", "jane.doe@school.org", "609-555-0100", "0012345", "Help with ASR", "Driver={"] {
            assert!(!r.contains(bad), "{bad} leaked: {r}");
        }
        assert!(redact("SQLSTATE HY000 / native -1811 in 2026").contains("-1811"));
        assert!(redact("SQLSTATE HY000 / native -1811 in 2026").contains("2026"));
    }

    #[test]
    fn record_is_bounded_and_never_fails() {
        let c = crate::db::open_in_memory().unwrap();
        record(&c, Severity::Error, "sync", "DB_LOCKED", "locked by a@b.org", "", "op-1");
        let v = recent(&c, "info", 10).unwrap();
        assert_eq!(v.len(), 1);
        assert!(!v[0].summary.contains("a@b.org"));
        // a broken table must not panic or error the caller
        c.execute_batch("DROP TABLE diag_events").unwrap();
        record(&c, Severity::Info, "sync", "", "still fine", "", "");
    }

    #[test]
    fn copied_log_is_plain_text_and_holds_no_sensitive_values() {
        let c = crate::db::open_in_memory().unwrap();
        // representative sensitive values passing through the real recording paths
        record(&c, Severity::Error, "shared-db", "DB_PERMISSION_DENIED", "Access denied for nurse.jane@school.org on \\\\srv\\share\\x.accdb", "native -1807 user=NJ\\jdoe C:\\Users\\jdoe\\AppData\\x", "");
        record(&c, Severity::Warning, "outlook-scan", "SCAN_PARTIAL", "Subject: Help with NJIIS 0012345 for Maple Leaf", "call 609-555-0100 x12", "op-1");
        record(&c, Severity::Info, "sync", "SYNC_OK", "Synchronized: sent 2", "Driver={Microsoft Access Driver (*.mdb, *.accdb)};DBQ=x;PWD=secret", "");
        let h = header(&c, "1.1.0", None);
        let text = copy_text(&c, &h, None, "info", 100);
        for bad in ["nurse.jane@school.org", "jdoe", "0012345", "Maple Leaf", "609-555-0100", "PWD=secret", "secret", "Driver={"] {
            assert!(!text.contains(bad), "{bad} leaked:\n{text}");
        }
        for want in ["DB_PERMISSION_DENIED", "native -1807", "SCAN_PARTIAL", "App version: 1.1.0", "[ERROR] shared-db", "srv"] {
            assert!(text.contains(want), "{want} missing:\n{text}");
        }
        assert!(text.lines().count() >= 9);
        // selection and latest error
        let ids: Vec<i64> = recent(&c, "error", 10).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(latest_error(&c), ids.first().copied());
        let sel = copy_text(&c, &h, Some(&ids), "info", 100);
        assert!(sel.contains("DB_PERMISSION_DENIED") && !sel.contains("SCAN_PARTIAL"));
        // bounded: old rows are trimmed
        for i in 0..(MAX_EVENTS + 250) {
            record(&c, Severity::Info, "t", "", &format!("e{i}"), "", "");
        }
        let n: i64 = c.query_row("SELECT COUNT(*) FROM diag_events", [], |r| r.get(0)).unwrap();
        assert!(n <= MAX_EVENTS + 100, "{n}");
    }

    /// Reproduce real failure conditions on this Windows machine and show what staff would copy:
    ///   set ASR_ACCESS_TEST_DIR=C:\local\test\folder && cargo test --no-default-features -- --ignored diag_reproduce --nocapture
    #[test]
    #[ignore]
    fn diag_reproduce_failures_with_the_real_driver() {
        use crate::shared_store::{self as ss, ErrKind};
        use crate::shared_sync as shs;
        let Ok(dir) = std::env::var("ASR_ACCESS_TEST_DIR") else { return };
        let base = std::path::PathBuf::from(dir).join(format!("diag-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let c = crate::db::open_in_memory().unwrap();
        let dev = ss::DeviceInfo { device_id: "dev-diag".into(), device_name: "PC".into(), windows_account: "x".into(), app_version: "test".into() };
        let mut seen = vec![];
        let mut check = |label: &str, path: &std::path::Path| {
            let p = ss::probe(path, &dev);
            let st = shs::compose_status(&c, "shared", &path.to_string_lossy(), Some(&p), None);
            record(&c, Severity::Error, "shared-db", st.error_code.as_deref().unwrap_or("OK"), &format!("{label}: {}", st.error_message.clone().unwrap_or_default()), st.provider_detail.as_deref().unwrap_or(""), "");
            println!("{label:28} -> state {:14} code {:28} reachable {:?} readable {:?} writable {:?} {}", st.state, st.error_code.clone().unwrap_or_default(), st.reachable, st.readable, st.writable, st.provider_detail.clone().unwrap_or_default());
            seen.push((label.to_string(), p.error.map(|e| e.kind)));
        };
        // 1. unreachable share (host does not exist)
        check("unreachable share", std::path::Path::new(r"\\asr-test-host-does-not-exist.invalid\share\asr.accdb"));
        // 2. folder exists, file missing
        check("missing file", &base.join("missing.accdb"));
        // 3. denied folder: a real NTFS deny ACE for the current user on a test folder
        let denied = base.join("denied");
        std::fs::create_dir_all(&denied).unwrap();
        let db = denied.join("asr.accdb");
        ss::create_database(&db, "denied test", "test", "diag test").unwrap();
        let user = std::env::var("USERNAME").unwrap();
        let ok = std::process::Command::new("icacls").arg(&denied).args(["/deny", &format!("{user}:(OI)(CI)(R,W)")]).output().unwrap();
        assert!(ok.status.success(), "icacls deny failed");
        check("permission denied", &db);
        let _ = std::process::Command::new("icacls").arg(&denied).args(["/remove:d", &user]).output();
        // 4. maintenance mode = failed sync until it ends
        let m = base.join("maint.accdb");
        ss::create_database(&m, "maintenance test", "test", "diag test").unwrap();
        {
            let mut cc = ss::open(&m, false).unwrap();
            ss::set_meta(cc.as_mut(), "maintenance", "1").unwrap();
            ss::set_meta(cc.as_mut(), "maintenance_message", "Upgrade by admin").unwrap();
        }
        check("maintenance", &m);
        // 5. missing driver: the real ODBC manager with a driver that is not installed (IM002)
        let e = crate::shared_store::SharedError { kind: ErrKind::DriverMissing, sqlstate: "IM002".into(), native: 0, message: "[Microsoft][ODBC Driver Manager] Data source name not found and no default driver specified".into() };
        record(&c, Severity::Error, "shared-db", e.code(), &format!("missing driver: {}", e.kind.plain()), &e.provider_detail(), "");
        seen.push(("missing driver".into(), Some(ErrKind::DriverMissing)));
        let text = copy_text(&c, &header(&c, "test", None), None, "info", 100);
        println!("\n===== copied log =====\n{text}");
        let kinds: Vec<_> = seen.iter().map(|s| s.1).collect();
        assert_eq!(kinds[0], Some(ErrKind::Unavailable));
        assert_eq!(kinds[1], Some(ErrKind::FileNotFound));
        assert!(matches!(kinds[2], Some(ErrKind::PermissionDenied) | Some(ErrKind::Unavailable)), "{:?}", kinds[2]);
        assert_eq!(kinds[3], Some(ErrKind::Maintenance));
        for code in ["SHARE_UNAVAILABLE", "DB_FILE_NOT_FOUND", "DB_MAINTENANCE", "ACCESS_DRIVER_MISSING"] {
            assert!(text.contains(code), "{code}");
        }
        assert!(!text.contains(&user), "the Windows user name is not in the copied log");
    }

    #[test]
    fn logging_failure_never_blocks_saving_or_synchronizing() {
        let dir = tempfile::tempdir().unwrap();
        let shared = crate::shared_store::tests::new_shared(dir.path());
        let a = crate::shared_sync::tests::Client::new("A");
        a.conn().execute_batch("DROP TABLE diag_events").unwrap();
        a.connect(&shared);
        a.ingest(crate::tickets::tests::msg(1, "C1", "Help", "body", "x@y.org"));
        let t = a.ticket_by_imid("<m1@x>");
        crate::notes::add(&a.conn(), t, "saved although diagnostics are broken", "A").unwrap();
        let r = a.sync(&shared).unwrap();
        assert!(r.accepted >= 2);
        assert_eq!(crate::shared_sync::tests::shared_count(&shared, "SELECT COUNT(*) FROM TicketNotes"), 1);
    }
}
