//! Synchronization logic that does not depend on the Tauri shell:
//! single-flight guard, incremental window computation, sync-state persistence,
//! bounded retries and processing of worker results.

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::outlook::{self, WorkerConfig};
use crate::settings::{FolderRef, Settings};
use crate::tickets::{self, IncomingMessage, IngestReport};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncStatus {
    pub running: bool,
    pub last_run_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub last_error_code: Option<String>,
    pub last_report: Option<IngestReport>,
    pub next_run_at: Option<String>,
    pub folder_display: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Default)]
pub struct SyncController {
    running: AtomicBool,
    pub wake: Notify,
    pub status: Mutex<SyncStatus>,
}

/// RAII guard that clears the running flag.
pub struct RunGuard<'a>(&'a AtomicBool);
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl SyncController {
    pub fn try_start(&self) -> Option<RunGuard<'_>> {
        self.running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| RunGuard(&self.running))
    }
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    pub fn snapshot(&self) -> SyncStatus {
        let mut s = self.status.lock().unwrap().clone();
        s.running = self.is_running();
        s
    }
}

pub fn folder_key(f: &FolderRef) -> String {
    format!("{}|{}", f.store_id, f.entry_id)
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderRequest {
    pub key: String,
    pub store_id: String,
    pub entry_id: String,
    pub path: String,
    pub include_subfolders: bool,
    pub since_utc: String,
    /// Scan mode "unread": only unread messages are returned (read state is never changed).
    pub unread_only: bool,
    /// Folders (and their subtrees) never scanned, e.g. the Mark Done completion folder.
    pub exclude_entry_ids: Vec<String>,
}

/// Window start for a folder: last received time minus the overlap, or the full
/// lookback for the first sync and for the periodic full rescan.
pub fn window_start(conn: &Connection, f: &FolderRef, s: &Settings, now: DateTime<Utc>) -> AppResult<(String, bool)> {
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT last_received_at, last_full_scan_at FROM sync_state WHERE folder_key=?1",
            [folder_key(f)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let lookback = now - ChronoDuration::days(s.initial_lookback_days as i64);
    let parse = |x: &Option<String>| x.as_deref().and_then(|v| DateTime::parse_from_rfc3339(v).ok()).map(|d| d.with_timezone(&Utc));
    let (since, full) = match row {
        None => (lookback, true),
        Some((last_recv, last_full)) => {
            let full_due = s.full_rescan_hours > 0
                && parse(&last_full).map(|t| now - t > ChronoDuration::hours(s.full_rescan_hours as i64)).unwrap_or(true);
            match parse(&last_recv) {
                Some(t) if !full_due => (t - ChronoDuration::minutes(s.overlap_minutes as i64), false),
                _ => (lookback, true),
            }
        }
    };
    Ok((since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), full))
}

pub fn record_state(conn: &Connection, f: &FolderRef, full: bool, max_recv: Option<&str>, imported: usize, err: Option<&str>) -> AppResult<()> {
    let ts = now();
    conn.execute(
        "INSERT INTO sync_state(folder_key,store_id,folder_entry_id,folder_path,last_run_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(folder_key) DO UPDATE SET folder_path=excluded.folder_path, last_run_at=excluded.last_run_at",
        params![folder_key(f), f.store_id, f.entry_id, f.path, ts],
    )?;
    match err {
        Some(e) => {
            conn.execute("UPDATE sync_state SET last_error=?2, last_error_at=?3 WHERE folder_key=?1", params![folder_key(f), e, ts])?;
        }
        None => {
            conn.execute(
                "UPDATE sync_state SET last_success_at=?2, last_error=NULL,
                   last_received_at=CASE WHEN ?3 IS NOT NULL AND (last_received_at IS NULL OR ?3 > last_received_at) THEN ?3 ELSE last_received_at END,
                   last_full_scan_at=CASE WHEN ?4 THEN ?2 ELSE last_full_scan_at END,
                   messages_imported=messages_imported+?5 WHERE folder_key=?1",
                params![folder_key(f), ts, max_recv, full, imported as i64],
            )?;
        }
    }
    Ok(())
}

/// Build the per-folder requests for one sync run.
pub fn build_requests(conn: &Connection, s: &Settings, now: DateTime<Utc>) -> AppResult<(Vec<FolderRequest>, Vec<bool>)> {
    let monitored = s.monitored();
    if monitored.is_empty() {
        return Err(AppError::coded("NOT_CONFIGURED", "No Outlook folder is selected. Choose a folder in Settings > Monitored Folders."));
    }
    let mut reqs = Vec::new();
    let mut fulls = Vec::new();
    for (f, subs) in &monitored {
        let (since, full) = window_start(conn, f, s, now)?;
        fulls.push(full);
        reqs.push(FolderRequest {
            key: folder_key(f),
            store_id: f.store_id.clone(),
            entry_id: f.entry_id.clone(),
            path: f.path.clone(),
            include_subfolders: *subs,
            since_utc: since,
            unread_only: s.scan_mode == "unread",
            exclude_entry_ids: s.archive_folder.iter().map(|a| a.entry_id.clone()).collect(),
        });
    }
    Ok((reqs, fulls))
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FolderResult {
    pub key: String,
    pub ok: bool,
    pub error_code: Option<String>,
    pub message: Option<String>,
    pub recovered: bool,
    pub resolved_entry_id: Option<String>,
    pub resolved_path: Option<String>,
    pub truncated: bool,
    pub messages: Vec<IncomingMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncOutcome {
    pub report: IngestReport,
    pub warnings: Vec<String>,
    pub folders: usize,
}

/// Apply worker results: ingest messages, persist sync state, handle recovered folders.
/// Returns the merged report, user-facing warnings and updated settings if a folder moved.
pub fn apply_results(conn: &Connection, settings: &Settings, fulls: &[bool], results: &[FolderResult]) -> AppResult<(IngestReport, Vec<String>, Option<Settings>)> {
    let mut warnings = Vec::new();
    let mut total = IngestReport::default();
    let mut new_settings = settings.clone();
    let mut changed = false;
    for (i, (f, _)) in settings.monitored().iter().enumerate() {
        let Some(r) = results.iter().find(|r| r.key == folder_key(f)) else { continue };
        if !r.ok {
            let msg = format!("{}: {}", f.path, r.message.clone().unwrap_or_else(|| "folder unavailable".into()));
            record_state(conn, f, false, None, 0, Some(&msg))?;
            warnings.push(msg);
            continue;
        }
        if r.recovered {
            if let Some(new_id) = &r.resolved_entry_id {
                let new_path = r.resolved_path.clone().unwrap_or_else(|| f.path.clone());
                let msg = format!("Folder \"{}\" was moved or renamed; it was found again at \"{}\".", f.path, new_path);
                tracing::warn!("{msg}");
                warnings.push(msg);
                let nf = FolderRef { store_id: f.store_id.clone(), entry_id: new_id.clone(), path: new_path };
                if new_settings.folder.as_ref() == Some(f) {
                    new_settings.folder = Some(nf);
                } else if let Some(s) = new_settings.selected_subfolders.iter_mut().find(|s| *s == f) {
                    *s = nf;
                }
                changed = true;
            }
        }
        if r.truncated {
            warnings.push(format!("{}: more messages than the per-sync limit; the rest will be imported on the next check.", f.path));
        }
        let rep = tickets::ingest(conn, &r.messages, settings)?;
        record_state(conn, f, fulls.get(i).copied().unwrap_or(false) && !r.truncated, rep.max_received_at.as_deref(), rep.new_messages, None)?;
        total.seen += rep.seen;
        total.new_messages += rep.new_messages;
        total.already_imported += rep.already_imported;
        total.moved += rep.moved;
        total.created_tickets += rep.created_tickets;
        total.updated_tickets += rep.updated_tickets;
        total.skipped_internal += rep.skipped_internal;
        total.duplicates_flagged += rep.duplicates_flagged;
    }
    Ok((total, warnings, if changed { Some(new_settings) } else { None }))
}

/// Call the worker with bounded retries for transient failures.
pub async fn call_with_retry(cfg: &WorkerConfig, op: &str, params: serde_json::Value, timeout: Duration, attempts: u32) -> AppResult<serde_json::Value> {
    let mut last: Option<AppError> = None;
    for attempt in 0..attempts.max(1) {
        if attempt > 0 {
            let wait = Duration::from_secs(5 * 3u64.pow(attempt - 1));
            tracing::info!(op, attempt, wait_s = wait.as_secs(), "retrying outlook operation");
            tokio::time::sleep(wait).await;
        }
        match outlook::call(cfg, op, params.clone(), timeout).await.and_then(outlook::into_result) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let transient = outlook::is_transient(e.code());
                last = Some(e);
                if !transient {
                    break;
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| AppError::msg("sync failed")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::tickets::tests::{conn, msg};

    #[test]
    fn single_flight_guard() {
        let c = SyncController::default();
        let g = c.try_start().expect("first start");
        assert!(c.try_start().is_none(), "overlapping run must be rejected");
        drop(g);
        assert!(c.try_start().is_some());
    }

    #[test]
    fn window_uses_lookback_then_watermark_with_overlap() {
        let conn = open_in_memory().unwrap();
        let mut s = Settings::default();
        s.initial_lookback_days = 7;
        s.overlap_minutes = 30;
        s.full_rescan_hours = 24;
        let f = FolderRef { store_id: "S".into(), entry_id: "E".into(), path: "Inbox/Wenjie".into() };
        let now_ = DateTime::parse_from_rfc3339("2026-11-16T12:00:00Z").unwrap().with_timezone(&Utc);
        let (since, full) = window_start(&conn, &f, &s, now_).unwrap();
        assert_eq!(since, "2026-11-09T12:00:00Z");
        assert!(full);
        record_state(&conn, &f, true, Some("2026-11-16T11:00:00Z"), 3, None).unwrap();
        conn.execute("UPDATE sync_state SET last_full_scan_at='2026-11-16T11:30:00Z'", []).unwrap();
        let (since, full) = window_start(&conn, &f, &s, now_).unwrap();
        assert_eq!(since, "2026-11-16T10:30:00Z");
        assert!(!full);
        let later = now_ + ChronoDuration::hours(30);
        let (_, full) = window_start(&conn, &f, &s, later).unwrap();
        assert!(full, "periodic full rescan");
        record_state(&conn, &f, false, None, 0, Some("boom")).unwrap();
        let w: String = conn.query_row("SELECT last_received_at FROM sync_state", [], |r| r.get(0)).unwrap();
        assert_eq!(w, "2026-11-16T11:00:00Z", "errors never move the watermark");
    }

    #[test]
    fn not_configured_and_subfolder_modes() {
        let c = open_in_memory().unwrap();
        let mut s = Settings::default();
        assert_eq!(build_requests(&c, &s, Utc::now()).unwrap_err().code(), "NOT_CONFIGURED");
        s.folder = Some(FolderRef { store_id: "S".into(), entry_id: "W".into(), path: "Inbox/Wenjie".into() });
        s.subfolder_mode = "all".into();
        let (r, _) = build_requests(&c, &s, Utc::now()).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].include_subfolders);
        s.subfolder_mode = "selected".into();
        s.selected_subfolders = vec![FolderRef { store_id: "S".into(), entry_id: "SUB".into(), path: "Inbox/Wenjie/Sub".into() }];
        let (r, _) = build_requests(&c, &s, Utc::now()).unwrap();
        assert_eq!(r.len(), 2);
        assert!(!r[0].include_subfolders);
        assert!(!r[0].unread_only);
        s.scan_mode = "unread".into();
        let (r, _) = build_requests(&c, &s, Utc::now()).unwrap();
        assert!(r.iter().all(|x| x.unread_only));
        assert!(r[0].exclude_entry_ids.is_empty());
        s.archive_folder = Some(FolderRef { store_id: "S".into(), entry_id: "DONE".into(), path: "Inbox/Wenjie/Done".into() });
        s.selected_with_children = vec!["SUB".into()];
        let (r, _) = build_requests(&c, &s, Utc::now()).unwrap();
        assert!(r[1].include_subfolders, "selected subfolder with its children");
        assert!(r.iter().all(|x| x.exclude_entry_ids == vec!["DONE".to_string()]), "the completion folder is never scanned");
    }

    #[test]
    fn apply_results_handles_errors_recovery_and_repeat_checks() {
        let c = conn();
        let mut s = Settings::default();
        let f = FolderRef { store_id: "S1".into(), entry_id: "OLD".into(), path: "Inbox/Wenjie".into() };
        s.folder = Some(f.clone());
        let res = vec![FolderResult {
            key: folder_key(&f), ok: true, recovered: true, resolved_entry_id: Some("NEW".into()),
            resolved_path: Some("Inbox/Wenjie2".into()),
            messages: vec![msg(1, "C1", "Black screen", "webinar black screen", "a@b.org")], ..Default::default()
        }];
        let (rep, warns, ns) = apply_results(&c, &s, &[true], &res).unwrap();
        assert_eq!(rep.created_tickets, 1);
        assert!(warns.iter().any(|w| w.contains("moved or renamed")));
        assert_eq!(ns.unwrap().folder.unwrap().entry_id, "NEW");
        // the same results again (repeat check) create nothing new
        let (rep, _, _) = apply_results(&c, &s, &[false], &res).unwrap();
        assert_eq!(rep.created_tickets, 0);
        assert_eq!(rep.already_imported, 1);
        let err = vec![FolderResult { key: folder_key(&f), ok: false, message: Some("Folder not found".into()), ..Default::default() }];
        let (_, warns, _) = apply_results(&c, &s, &[false], &err).unwrap();
        assert!(warns[0].contains("Folder not found"));
    }
}
