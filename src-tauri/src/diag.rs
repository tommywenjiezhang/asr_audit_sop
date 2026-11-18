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

/// Mask personal and secret values. Applied to every summary and detail before writing or copying.
pub fn redact(s: &str) -> String {
    let s = DRIVER_CS.replace_all(s, "[connection string removed]");
    let s = SECRET.replace_all(&s, "$1=[removed]");
    let s = SUBJECT.replace_all(&s, "${1}[subject removed]");
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
}
