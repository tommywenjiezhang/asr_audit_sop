//! One synchronization cycle with the shared database (blocking; run off the UI thread).
//!
//! 1. Validate the database identity, schema compatibility and maintenance mode.
//! 2. Bind (first contact) or verify the identity this computer's queue belongs to.
//! 3. Write check (device heartbeat). Read-only databases are still pulled, never pushed to.
//! 4. Push pending operations bound to this identity, oldest first, one short shared transaction each.
//!    Retries reuse the same op id; a lost acknowledgement returns the stored result.
//! 5. Pull: staff, then cases changed since the last checkpoint (re-reading an overlap because AutoNumber
//!    order is not commit order) plus a periodic full version reconciliation.
//!
//! The local SQLite lock is held only for short local steps, never while waiting on the network.

use crate::diag::{self, Severity};
use crate::error::AppError;
use crate::shared_apply as sa;
use crate::shared_store::{self as ss, DbIdentity, DeviceInfo, ErrKind, SResult, SharedConn, SharedError};
use crate::sync_queue as q;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

/// Feed rows re-read below the checkpoint on every pull.
pub const FEED_OVERLAP: i64 = 200;
/// Full version reconciliation interval.
pub const FULL_RECONCILE_MINUTES: i64 = 30;
const MAX_OPS_PER_CYCLE: usize = 500;
/// After this many failed attempts a non-transient error rejects the op (it stays visible for retry).
const MAX_ATTEMPTS: i64 = 5;

#[derive(Debug, Clone, Default, Serialize)]
pub struct CycleReport {
    pub identity: Option<DbIdentity>,
    /// "newly_bound" | "same"
    pub binding: String,
    pub writable: bool,
    pub pushed: usize,
    pub accepted: usize,
    pub conflicts: usize,
    pub rejected: usize,
    /// results that had already been stored by an earlier attempt (lost acknowledgements)
    pub replayed: usize,
    pub pulled: usize,
    pub staff: usize,
    pub full_reconcile: bool,
    pub held: i64,
    pub elapsed_ms: u64,
}

fn local_err(e: AppError) -> SharedError {
    SharedError::new(ErrKind::Other, format!("local database: {e}"))
}

fn lock(local: &Mutex<Connection>) -> std::sync::MutexGuard<'_, Connection> {
    local.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn run_cycle(local: &Mutex<Connection>, shared: &mut dyn SharedConn, dev: &DeviceInfo, mailbox_name: &str) -> SResult<CycleReport> {
    let started = std::time::Instant::now();
    let mut rep = CycleReport::default();
    // 1. identity / compatibility
    let id = ss::identity(shared)?;
    match ss::compatibility(&id) {
        "ok" => {}
        c => return Err(SharedError::new(ErrKind::Incompatible, format!("schema compatibility: {c}"))),
    }
    // 2. binding
    let binding = q::check_binding(&lock(local), &id.database_id, &id.display_name).map_err(local_err)?;
    match &binding {
        q::Binding::Mismatch { bound } => {
            return Err(SharedError::new(ErrKind::IdentityChanged, format!("bound to {bound}, found {}", id.database_id)));
        }
        q::Binding::NewlyBound => {
            rep.binding = "newly_bound".into();
            diag::record(&lock(local), Severity::Info, "shared-db", "DB_BOUND", &format!("Connected to shared database \"{}\" for the first time; earlier local changes are held for review.", id.display_name), "", &id.database_id);
        }
        q::Binding::Same => rep.binding = "same".into(),
    }
    rep.identity = Some(id.clone());
    // 3. write check
    rep.writable = if id.maintenance {
        false
    } else {
        match ss::heartbeat(shared, dev) {
            Ok(()) => true,
            Err(e) if matches!(e.kind, ErrKind::ReadOnly | ErrKind::PermissionDenied) => false,
            Err(e) => return Err(e),
        }
    };
    // 4. push
    let mut refresh: BTreeSet<String> = BTreeSet::new();
    if rep.writable {
        let _ = sa::ensure_mailbox(shared, mailbox_name, &dev.windows_account);
        let mut done_rows: BTreeSet<i64> = BTreeSet::new();
        'push: while rep.pushed < MAX_OPS_PER_CYCLE {
            let batch = q::next_batch(&lock(local), &id.database_id, 50).map_err(local_err)?;
            let batch: Vec<_> = batch.into_iter().filter(|(r, _)| !done_rows.contains(r)).collect();
            if batch.is_empty() {
                break;
            }
            for (row, op) in batch {
                done_rows.insert(row);
                rep.pushed += 1;
                match sa::apply(shared, &op) {
                    Ok(res) => {
                        if res.duplicate {
                            rep.replayed += 1;
                        }
                        match res.result.as_str() {
                            "accepted" => rep.accepted += 1,
                            "conflict" => rep.conflicts += 1,
                            _ => rep.rejected += 1,
                        }
                        let conn = lock(local);
                        q::record_result(&conn, row, &res).map_err(local_err)?;
                        if res.result != "accepted" {
                            diag::record(&conn, if res.result == "conflict" { Severity::Warning } else { Severity::Error }, "sync", &format!("OP_{}", res.result.to_uppercase()),
                                &format!("{} {}: {}", op.op_type, res.result, if res.message.is_empty() { res.conflicts.first().map(|c| c.reason.clone()).unwrap_or_default() } else { res.message.clone() }), "", &op.op_id);
                        }
                        if let Some(t) = res.ticket_id {
                            refresh.insert(t);
                        }
                    }
                    Err(e) => {
                        let conn = lock(local);
                        q::record_attempt(&conn, row, Some(&format!("{} {}", e.code(), e.message))).map_err(local_err)?;
                        diag::record(&conn, if e.kind.transient() { Severity::Warning } else { Severity::Error }, "sync", e.code(), &format!("Sending {} failed: {}", op.op_type, e.kind.plain()), &format!("{} {}", e.provider_detail(), e.message), &op.op_id);
                        if e.kind.transient() || matches!(e.kind, ErrKind::Maintenance | ErrKind::ReadOnly | ErrKind::PermissionDenied | ErrKind::Unavailable) {
                            // stop: the op keeps its id and is retried on the next cycle
                            drop(conn);
                            return Err(e);
                        }
                        let attempts: i64 = conn.query_row("SELECT attempts FROM sync_ops WHERE id=?1", [row], |r| r.get(0)).unwrap_or(0);
                        if attempts >= MAX_ATTEMPTS {
                            let _ = conn.execute("UPDATE sync_ops SET state='rejected', settled_at=?2 WHERE id=?1", rusqlite::params![row, crate::db::now()]);
                            rep.rejected += 1;
                        }
                        continue 'push;
                    }
                }
            }
        }
    }
    // 5. pull
    let staff = sa::staff_all(shared)?;
    rep.staff = q::apply_staff(&lock(local), &staff).map_err(local_err)?;
    let seq_key = format!("shared_feed_seq:{}", id.database_id);
    let full_key = format!("shared_full_at:{}", id.database_id);
    let (since, last_full, local_versions) = {
        let conn = lock(local);
        let since: i64 = crate::db::get_setting(&conn, &seq_key).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
        let last_full = crate::db::get_setting(&conn, &full_key).ok().flatten();
        let mut st = conn.prepare("SELECT shared_id, shared_version FROM tickets WHERE shared_id IS NOT NULL AND merged_into IS NULL AND shared_state='shared'").map_err(|e| local_err(e.into()))?;
        let map: HashMap<String, i64> = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| local_err(e.into()))?
            .filter_map(|r| r.ok())
            .collect();
        (since, last_full, map)
    };
    let full_due = last_full
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) > chrono::Duration::minutes(FULL_RECONCILE_MINUTES))
        .unwrap_or(true);
    let top = sa::max_feed_seq(shared)?;
    let mut wanted: BTreeSet<String> = refresh;
    if full_due {
        rep.full_reconcile = true;
        for (tid, ver) in sa::ticket_versions(shared)? {
            if local_versions.get(&tid).map(|v| *v < ver).unwrap_or(true) {
                wanted.insert(tid);
            }
        }
    } else {
        for row in sa::feed_since(shared, (since - FEED_OVERLAP).max(0), 5000)? {
            if let Some(t) = row.ticket_id {
                if local_versions.get(&t).map(|v| *v < row.ticket_version).unwrap_or(true) {
                    wanted.insert(t);
                }
            }
        }
    }
    let ids: Vec<String> = wanted.into_iter().collect();
    for b in sa::fetch_bundles(shared, &ids)? {
        let conn = lock(local);
        match q::apply_bundle(&conn, &b) {
            Ok(_) => rep.pulled += 1,
            Err(e) => diag::record(&conn, Severity::Error, "sync", "PULL_APPLY_FAILED", "A shared case could not be stored locally", &e.to_string(), &b.core.ticket_id),
        }
    }
    {
        let conn = lock(local);
        let now = crate::db::now();
        let _ = crate::db::set_setting(&conn, &seq_key, &top.max(since).to_string());
        if full_due {
            let _ = crate::db::set_setting(&conn, &full_key, &now);
        }
        let _ = crate::db::set_setting(&conn, "shared_last_success_at", &now);
        rep.held = q::summary(&conn).map(|s| s.held).unwrap_or(0);
    }
    rep.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(rep)
}

// ------------------------------------------------------------------ status interface (used by the UI,
// the scheduler and Diagnostics)

/// Connection and queue state of the shared database, as shown in Settings › Shared Database.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SharedStatus {
    /// "shared" | "standalone"
    pub mode: String,
    pub path: String,
    /// "standalone" | "not_checked" | "connected" | "read_only" | "unavailable" | "driver_missing" | "locked" |
    /// "no_permission" | "incompatible" | "maintenance" | "identity_changed" | "error"
    pub state: String,
    pub reachable: Option<bool>,
    pub readable: Option<bool>,
    pub writable: Option<bool>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub environment: Option<String>,
    pub schema_version: Option<i64>,
    /// "ok" | "upgrade_required" | "app_too_old"
    pub compatibility: Option<String>,
    pub app_schema_version: i64,
    pub bound_database_id: Option<String>,
    pub bound_database_name: Option<String>,
    pub driver: Option<String>,
    pub error_code: Option<String>,
    /// plain-language explanation
    pub error_message: Option<String>,
    /// SQLSTATE / native code (no message text: that is in Diagnostics, redacted)
    pub provider_detail: Option<String>,
    pub last_success_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub next_attempt_at: Option<String>,
    pub queue: q::QueueSummary,
    pub running: bool,
    pub last_report: Option<CycleReport>,
    pub consecutive_failures: u32,
}

/// Single-flight guard + latest status + wake signal for the background synchronization.
#[derive(Default)]
pub struct SharedController {
    running: std::sync::atomic::AtomicBool,
    pub wake: tokio::sync::Notify,
    pub status: Mutex<SharedStatus>,
    failures: std::sync::atomic::AtomicU32,
}

pub struct SharedGuard<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for SharedGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl SharedController {
    pub fn try_start(&self) -> Option<SharedGuard<'_>> {
        use std::sync::atomic::Ordering;
        self.running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| SharedGuard(&self.running))
    }
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn snapshot(&self) -> SharedStatus {
        let mut s = self.status.lock().unwrap_or_else(|p| p.into_inner()).clone();
        s.running = self.is_running();
        s.consecutive_failures = self.failures.load(std::sync::atomic::Ordering::SeqCst);
        s
    }
    pub fn record_outcome(&self, ok: bool) -> u32 {
        use std::sync::atomic::Ordering;
        if ok {
            self.failures.store(0, Ordering::SeqCst);
            0
        } else {
            self.failures.fetch_add(1, Ordering::SeqCst) + 1
        }
    }
    /// Seconds until the next automatic attempt: the configured interval when healthy, otherwise a bounded
    /// exponential backoff (30 s, 1, 2, 4, 8 min ... capped at 10 min and never longer than the interval).
    pub fn delay_secs(&self, interval_minutes: u32) -> u64 {
        let base = interval_minutes.max(1) as u64 * 60;
        match self.failures.load(std::sync::atomic::Ordering::SeqCst) {
            0 => base,
            f => (30u64 << (f - 1).min(5)).min(600).min(base.max(30)),
        }
    }
}

pub fn state_for(kind: ErrKind) -> &'static str {
    match kind {
        ErrKind::Unavailable | ErrKind::FileNotFound | ErrKind::Interrupted => "unavailable",
        ErrKind::DriverMissing | ErrKind::ArchMismatch => "driver_missing",
        ErrKind::Locked | ErrKind::Constraint => "locked",
        ErrKind::PermissionDenied => "no_permission",
        ErrKind::ReadOnly => "read_only",
        ErrKind::Incompatible | ErrKind::Corrupt => "incompatible",
        ErrKind::Maintenance => "maintenance",
        ErrKind::IdentityChanged => "identity_changed",
        ErrKind::Other => "error",
    }
}

/// Check the database and (optionally) run a synchronization cycle. Blocking.
pub fn connect_and_sync(local: &Mutex<Connection>, path: &std::path::Path, dev: &DeviceInfo, mailbox: &str, do_sync: bool) -> (ss::ProbeReport, Option<SResult<CycleReport>>) {
    let probe = ss::probe(path, dev);
    if !do_sync || !probe.readable || probe.compatibility != "ok" {
        return (probe, None);
    }
    if probe.error.as_ref().map(|e| e.kind == ErrKind::Maintenance).unwrap_or(false) {
        return (probe, None);
    }
    let cycle = ss::open(path, !probe.writable).and_then(|mut c| run_cycle(local, c.as_mut(), dev, mailbox));
    (probe, Some(cycle))
}

/// Build the status shown to staff from a check/cycle result and the local queue.
pub fn compose_status(conn: &Connection, mode: &str, path: &str, probe: Option<&ss::ProbeReport>, cycle: Option<&SResult<CycleReport>>) -> SharedStatus {
    let mut s = SharedStatus {
        mode: mode.into(),
        path: path.into(),
        state: if mode == q::MODE_STANDALONE { "standalone".into() } else { "not_checked".into() },
        app_schema_version: ss::SHARED_SCHEMA_VERSION,
        bound_database_id: q::bound_db(conn),
        bound_database_name: crate::db::get_setting(conn, "shared_db_name").ok().flatten(),
        last_success_at: crate::db::get_setting(conn, "shared_last_success_at").ok().flatten(),
        queue: q::summary(conn).unwrap_or_default(),
        ..Default::default()
    };
    if let Some(p) = probe {
        s.last_attempt_at = Some(crate::db::now());
        s.reachable = Some(p.reachable);
        s.readable = Some(p.readable);
        s.writable = Some(p.writable);
        s.driver = p.driver.map(|d| format!("{d:?}"));
        if let Some(id) = &p.identity {
            s.database_id = Some(id.database_id.clone());
            s.database_name = Some(id.display_name.clone());
            s.environment = Some(id.environment.clone());
            s.schema_version = Some(id.schema_version);
            s.compatibility = Some(p.compatibility.clone());
        }
        let mut err = p.error.clone();
        if err.is_none() {
            if let (Some(id), Some(b)) = (&p.identity, &s.bound_database_id) {
                if &id.database_id != b {
                    err = Some(SharedError::new(ErrKind::IdentityChanged, ""));
                }
            }
        }
        if let Some(Err(e)) = cycle {
            err = Some(e.clone());
        }
        if let Some(Ok(r)) = cycle {
            s.last_report = Some(r.clone());
            s.last_success_at = crate::db::get_setting(conn, "shared_last_success_at").ok().flatten();
            s.bound_database_id = q::bound_db(conn);
            s.bound_database_name = crate::db::get_setting(conn, "shared_db_name").ok().flatten();
            s.queue = q::summary(conn).unwrap_or_default();
        }
        match err {
            Some(e) => {
                s.state = state_for(e.kind).into();
                s.error_code = Some(e.code().into());
                s.error_message = Some(match e.kind {
                    ErrKind::Incompatible | ErrKind::Maintenance if !e.message.is_empty() => format!("{} {}", e.kind.plain(), crate::diag::redact(&e.message)),
                    _ => e.kind.plain().to_string(),
                });
                let d = e.provider_detail();
                s.provider_detail = if d.is_empty() { None } else { Some(d) };
            }
            None if mode == q::MODE_STANDALONE => s.state = "standalone".into(),
            None => s.state = "connected".into(),
        }
    }
    s
}

/// Connected dry run for the import preview: how many local-only cases already exist in the shared
/// database (same Internet Message-ID in the same logical mailbox).
pub fn import_dry_run(shared: &mut dyn SharedConn, imids: &[(i64, String)]) -> SResult<i64> {
    let mailbox = sa::mailbox_key(shared)?;
    let mut keys: Vec<(i64, String)> = imids.iter().map(|(t, m)| (*t, sa::message_match_key(&mailbox, m))).collect();
    keys.sort();
    let mut found: BTreeSet<i64> = BTreeSet::new();
    for chunk in keys.chunks(40) {
        let ph = vec!["?"; chunk.len()].join(", ");
        let p: Vec<ss::Val> = chunk.iter().map(|(_, k)| k.clone().into()).collect();
        let hits: BTreeSet<String> = shared.query(&format!("SELECT match_key FROM Emails WHERE match_key IN ({ph})"), &p)?.iter().map(|r| ss::col_s(r, 0)).collect();
        for (t, k) in chunk {
            if hits.contains(k) {
                found.insert(*t);
            }
        }
    }
    Ok(found.len() as i64)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::shared_store::tests::{device, new_shared};
    use crate::tickets::tests::msg;
    use crate::tickets::{self, ingest};

    /// One simulated workstation: its own local SQLite database and device identity.
    pub struct Client {
        pub local: Mutex<Connection>,
        pub dev: DeviceInfo,
        pub name: String,
    }

    impl Client {
        pub fn new(name: &str) -> Client {
            let c = crate::db::open_in_memory().unwrap();
            crate::templates_store::seed_embedded(&c).unwrap();
            crate::db::set_setting(&c, "device_id", &format!("dev-{name}")).unwrap();
            Client { local: Mutex::new(c), dev: device(name), name: name.into() }
        }
        pub fn on_file(name: &str, path: &std::path::Path) -> Client {
            let c = crate::db::open(path).unwrap();
            crate::templates_store::seed_embedded(&c).unwrap();
            crate::db::set_setting(&c, "device_id", &format!("dev-{name}")).unwrap();
            Client { local: Mutex::new(c), dev: device(name), name: name.into() }
        }
        pub fn sync(&self, shared: &std::path::Path) -> SResult<CycleReport> {
            let mut c = ss::open(shared, false)?;
            run_cycle(&self.local, c.as_mut(), &self.dev, "ASR Test Mailbox")
        }
        /// first sync binds; this also releases anything held before binding (explicit review step)
        pub fn connect(&self, shared: &std::path::Path) -> CycleReport {
            let r = self.sync(shared).unwrap();
            let n = q::release_held(&self.conn()).unwrap();
            if n > 0 {
                return self.sync(shared).unwrap();
            }
            r
        }
        pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
            lock(&self.local)
        }
        pub fn ingest(&self, m: crate::tickets::IncomingMessage) {
            ingest(&self.conn(), &[m], &Settings::default()).unwrap();
        }
        pub fn ticket_by_imid(&self, imid: &str) -> i64 {
            self.conn()
                .query_row(
                    "SELECT l.ticket_id FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id JOIN tickets t ON t.id=l.ticket_id
                     WHERE m.internet_message_id=?1 AND t.merged_into IS NULL",
                    [imid],
                    |r| r.get(0),
                )
                .unwrap()
        }
        pub fn shared_id(&self, tid: i64) -> String {
            self.conn().query_row("SELECT shared_id FROM tickets WHERE id=?1", [tid], |r| r.get(0)).unwrap()
        }
        pub fn count(&self, sql: &str) -> i64 {
            self.conn().query_row(sql, [], |r| r.get(0)).unwrap()
        }
    }

    pub fn shared_count(p: &std::path::Path, sql: &str) -> i64 {
        let mut c = ss::open(p, false).unwrap();
        ss::col_i(&c.query(sql, &[]).unwrap()[0], 0)
    }

    #[test]
    fn works_offline_and_queue_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("a.db");
        {
            let a = Client::on_file("A", &db);
            // the configured share is unavailable: local work continues
            assert_eq!(a.sync(&dir.path().join("missing").join("x.sqlite")).unwrap_err().kind, ErrKind::FileNotFound);
            a.ingest(msg(1, "C1", "Help", "user guide please", "a@b.org"));
            let t = a.ticket_by_imid("<m1@x>");
            tickets::set_field(&a.conn(), t, "school_name", Some("Maple".into()), false, "Aaron").unwrap();
            assert_eq!(a.count("SELECT COUNT(*) FROM sync_ops WHERE state='held'"), 2, "creation + edit held: no identity confirmed yet");
        }
        // restart
        let a = Client::on_file("A", &db);
        let shared = new_shared(dir.path());
        let r = a.sync(&shared).unwrap();
        assert_eq!(r.binding, "newly_bound");
        assert_eq!(r.pushed, 0, "held changes are not sent before review");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets"), 0);
        assert_eq!(q::release_held(&a.conn()).unwrap(), 2);
        let r = a.sync(&shared).unwrap();
        assert_eq!((r.pushed, r.accepted), (2, 2));
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets"), 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketFields WHERE field_key='school_name' AND manual_value='Maple'"), 1);
        // later edits are queued and sent
        let t = a.ticket_by_imid("<m1@x>");
        tickets::set_field(&a.conn(), t, "school_njiis_id", Some("0012345".into()), true, "Aaron").unwrap();
        assert_eq!(a.count("SELECT COUNT(*) FROM sync_ops WHERE state='pending'"), 1);
        a.sync(&shared).unwrap();
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketFields WHERE field_key='school_njiis_id' AND manual_value='0012345' AND is_verified=1"), 1, "leading zeros kept as text");
        assert_eq!(q::ticket_sync_state(&a.conn(), t).unwrap(), "synced");
    }

    #[test]
    fn retries_after_lost_acknowledgement_create_one_shared_change() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("A");
        a.connect(&shared);
        a.ingest(msg(1, "C1", "Help", "body", "a@b.org"));
        let t = a.ticket_by_imid("<m1@x>");
        a.sync(&shared).unwrap();
        tickets::set_field(&a.conn(), t, "school_name", Some("Oak".into()), false, "A").unwrap();
        let db = q::bound_db(&a.conn()).unwrap();
        let (row, op) = q::next_batch(&a.conn(), &db, 10).unwrap().remove(0);
        // committed in the shared database, but the acknowledgement never reached this computer
        let mut c = ss::open(&shared, false).unwrap();
        let first = sa::apply(c.as_mut(), &op).unwrap();
        assert!(!first.duplicate);
        drop(c);
        let versions_before = shared_count(&shared, "SELECT row_version FROM Tickets");
        // the retry reuses the op id: the stored result is returned, nothing is applied twice
        let r = a.sync(&shared).unwrap();
        assert_eq!(r.replayed, 1);
        assert_eq!(shared_count(&shared, "SELECT row_version FROM Tickets"), versions_before);
        assert_eq!(shared_count(&shared, &format!("SELECT COUNT(*) FROM ActivityLog WHERE op_id='{}'", op.op_id)), 1);
        assert_eq!(shared_count(&shared, &format!("SELECT COUNT(*) FROM ProcessedOperations WHERE op_id='{}'", op.op_id)), 1);
        assert_eq!(a.count(&format!("SELECT COUNT(*) FROM sync_ops WHERE id={row} AND state='accepted'")), 1);
        // an op that never reached the database is simply applied once on retry
        tickets::set_field(&a.conn(), t, "school_name", Some("Elm".into()), false, "A").unwrap();
        a.sync(&shared).unwrap();
        a.sync(&shared).unwrap();
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM ActivityLog WHERE field_name='school_name'"), 2);
    }

    #[test]
    fn two_clients_importing_the_same_email_converge_and_keep_notes_and_edits() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("A");
        let b = Client::new("B");
        a.connect(&shared);
        b.connect(&shared);
        // both import the same Outlook email while offline, and both work on it
        a.ingest(msg(1, "C1", "Subtype", "School: Maple Leaf, NJIIS 0012345", "nurse@school.org"));
        b.ingest(msg(1, "C1", "Subtype", "School: Maple Leaf, NJIIS 0012345", "nurse@school.org"));
        let ta = a.ticket_by_imid("<m1@x>");
        let tb = b.ticket_by_imid("<m1@x>");
        crate::notes::add(&a.conn(), ta, "Called the nurse", "Aaron").unwrap();
        crate::notes::add(&b.conn(), tb, "Emailed the district", "Wenjie").unwrap();
        tickets::set_field(&a.conn(), ta, "sender_title", Some("School Nurse".into()), false, "Aaron").unwrap();
        tickets::set_field(&b.conn(), tb, "sender_phone", Some("609-555-0100".into()), false, "Wenjie").unwrap();
        a.sync(&shared).unwrap();
        let rb = b.sync(&shared).unwrap();
        assert!(rb.accepted >= 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets"), 1, "one shared case");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Emails"), 1, "one shared email (unique match key)");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketAliases WHERE alias_type='duplicate'"), 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketNotes"), 2, "both computers' notes kept");
        a.sync(&shared).unwrap();
        b.sync(&shared).unwrap();
        let sid_a = a.shared_id(a.ticket_by_imid("<m1@x>"));
        let sid_b = b.shared_id(b.ticket_by_imid("<m1@x>"));
        assert_eq!(sid_a, sid_b, "both computers point at the same shared case");
        for cl in [&a, &b] {
            let t = cl.ticket_by_imid("<m1@x>");
            let notes = crate::notes::list(&cl.conn(), t).unwrap();
            assert_eq!(notes.len(), 2, "{}: notes from both computers", cl.name);
            let f = tickets::fields(&cl.conn(), t).unwrap();
            assert_eq!(f.iter().find(|f| f.key == "sender_title").unwrap().manual_value.as_deref(), Some("School Nurse"));
            assert_eq!(f.iter().find(|f| f.key == "sender_phone").unwrap().manual_value.as_deref(), Some("609-555-0100"));
            assert_eq!(cl.count("SELECT COUNT(*) FROM tickets WHERE merged_into IS NULL"), 1);
        }
    }

    #[test]
    fn competing_edits_merge_by_field_and_conflicts_keep_both_values() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("A");
        let b = Client::new("B");
        a.connect(&shared);
        a.ingest(msg(1, "C1", "Help", "body", "x@y.org"));
        a.sync(&shared).unwrap();
        b.connect(&shared);
        let ta = a.ticket_by_imid("<m1@x>");
        let tb = b.ticket_by_imid("<m1@x>");
        // different fields: both kept
        tickets::set_field(&a.conn(), ta, "school_name", Some("Maple".into()), false, "A").unwrap();
        tickets::set_field(&b.conn(), tb, "school_zip", Some("08601".into()), false, "B").unwrap();
        // same field, different values: conflict
        tickets::set_field(&a.conn(), ta, "sender_title", Some("Nurse".into()), false, "A").unwrap();
        tickets::set_field(&b.conn(), tb, "sender_title", Some("Principal".into()), false, "B").unwrap();
        a.sync(&shared).unwrap();
        let r = b.sync(&shared).unwrap();
        assert_eq!(r.conflicts, 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketFields WHERE (field_key='school_name' AND manual_value='Maple') OR (field_key='school_zip' AND manual_value='08601')"), 2);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketFields WHERE field_key='sender_title' AND manual_value='Nurse'"), 1, "first accepted value is not overwritten");
        let cs = q::conflicts(&b.conn(), Some(tb), false).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!((cs[0].proposed_value.as_deref(), cs[0].shared_value.as_deref()), (Some("Principal"), Some("Nurse")));
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM SyncConflicts WHERE conflict_status='open' AND proposed_value='Principal'"), 1);
        assert_eq!(q::ticket_sync_state(&b.conn(), tb).unwrap(), "conflict");
        // B sees the confirmed shared value
        let v = tickets::fields(&b.conn(), tb).unwrap().into_iter().find(|f| f.key == "sender_title").unwrap();
        assert_eq!(v.manual_value.as_deref(), Some("Nurse"));
        // accepting the shared value stays "resolving" until the shared database accepts it
        q::accept_shared(&b.conn(), &cs[0].conflict_id, "B").unwrap();
        assert_eq!(q::conflicts(&b.conn(), Some(tb), false).unwrap()[0].status, "resolving");
        b.sync(&shared).unwrap();
        assert!(q::conflicts(&b.conn(), Some(tb), false).unwrap().is_empty());
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM SyncConflicts WHERE conflict_status='resolved'"), 1);
    }

    #[test]
    fn pull_never_overwrites_unsent_local_work() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("A");
        let b = Client::new("B");
        a.connect(&shared);
        a.ingest(msg(1, "C1", "Help", "body", "x@y.org"));
        a.sync(&shared).unwrap();
        b.connect(&shared);
        let tb = b.ticket_by_imid("<m1@x>");
        tickets::set_field(&b.conn(), tb, "school_name", Some("B-local".into()), false, "B").unwrap();
        // B is read-only for this pull: simulate by pulling without pushing (identity same, op pending)
        let ta = a.ticket_by_imid("<m1@x>");
        tickets::set_field(&a.conn(), ta, "school_name", Some("A-value".into()), false, "A").unwrap();
        a.sync(&shared).unwrap();
        let mut c = ss::open(&shared, false).unwrap();
        let bundles = sa::fetch_bundles(c.as_mut(), &[b.shared_id(tb)]).unwrap();
        q::apply_bundle(&b.conn(), &bundles[0]).unwrap();
        let v = tickets::fields(&b.conn(), tb).unwrap().into_iter().find(|f| f.key == "school_name").unwrap();
        assert_eq!(v.manual_value.as_deref(), Some("B-local"), "pending local edit kept until settled");
    }

    #[test]
    fn identity_change_isolates_the_queue_and_never_replays_into_another_database() {
        let dir = tempfile::tempdir().unwrap();
        let first = new_shared(dir.path());
        let other_dir = tempfile::tempdir().unwrap();
        let second = new_shared(other_dir.path());
        let a = Client::new("A");
        a.connect(&first);
        a.ingest(msg(1, "C1", "Help", "body", "x@y.org"));
        assert_eq!(a.count("SELECT COUNT(*) FROM sync_ops WHERE state='pending'"), 1);
        // the path now points at a different database
        let e = a.sync(&second).unwrap_err();
        assert_eq!(e.kind, ErrKind::IdentityChanged);
        assert_eq!(shared_count(&second, "SELECT COUNT(*) FROM Tickets"), 0, "nothing replayed into the other database");
        assert_eq!(shared_count(&second, "SELECT COUNT(*) FROM ProcessedOperations"), 0);
        // explicit switch: old operations are isolated, not sent
        let mut c = ss::open(&second, false).unwrap();
        let id2 = ss::identity(c.as_mut()).unwrap();
        assert_eq!(q::rebind(&a.conn(), &id2.database_id, &id2.display_name).unwrap(), 1);
        a.sync(&second).unwrap();
        assert_eq!(shared_count(&second, "SELECT COUNT(*) FROM Tickets"), 0);
        assert_eq!(a.count("SELECT COUNT(*) FROM sync_ops WHERE state='isolated'"), 1);
        // and the first database never received it either (it was offline-pending for it)
        assert_eq!(shared_count(&first, "SELECT COUNT(*) FROM Tickets"), 0);
    }

    #[test]
    fn restored_backup_does_not_replay_stale_operations() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::on_file("A", &dir.path().join("a.db"));
        a.connect(&shared);
        a.ingest(msg(1, "C1", "Help", "body", "x@y.org"));
        a.sync(&shared).unwrap();
        let t = a.ticket_by_imid("<m1@x>");
        tickets::set_field(&a.conn(), t, "school_name", Some("Old value".into()), false, "A").unwrap();
        let bk = dir.path().join("backup.db");
        crate::export::backup(&a.conn(), &bk).unwrap();
        // meanwhile the case moved on in the shared database
        a.sync(&shared).unwrap();
        tickets::set_field(&a.conn(), t, "school_name", Some("Newer value".into()), false, "A").unwrap();
        a.sync(&shared).unwrap();
        // restore the old backup: its pending op must be held, not replayed
        {
            let mut conn = a.conn();
            crate::export::restore(&mut conn, &bk, &dir.path().join("safety.db")).unwrap();
            assert_eq!(q::after_restore(&conn).unwrap(), 1);
        }
        let r = a.sync(&shared).unwrap();
        assert_eq!(r.pushed, 0);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketFields WHERE field_key='school_name' AND manual_value='Newer value'"), 1);
    }

    #[test]
    fn standalone_mode_keeps_records_local_until_explicit_import() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("A");
        q::set_mode(&a.conn(), q::MODE_STANDALONE).unwrap();
        a.ingest(msg(1, "C1", "Help", "body", "x@y.org"));
        a.ingest(msg(2, "C2", "Other", "", "y@z.org"));
        assert_eq!(a.count("SELECT COUNT(*) FROM sync_ops"), 0);
        // the database becoming reachable does not upload anything by itself
        q::set_mode(&a.conn(), q::MODE_SHARED).unwrap();
        a.connect(&shared);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets"), 0);
        let p = q::import_preview(&a.conn()).unwrap();
        assert_eq!((p.local_only, p.with_message_id), (2, 2));
        // restartable import in batches
        assert_eq!(q::queue_import(&a.conn(), "A", 1).unwrap(), 1);
        assert_eq!(q::queue_import(&a.conn(), "A", 10).unwrap(), 1);
        assert_eq!(q::queue_import(&a.conn(), "A", 10).unwrap(), 0);
        a.sync(&shared).unwrap();
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets WHERE origin='migration'"), 2);
    }

    #[test]
    fn emails_without_message_id_are_never_merged_automatically() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("A");
        let b = Client::new("B");
        a.connect(&shared);
        b.connect(&shared);
        let mut m = msg(1, "", "Help", "body", "x@y.org");
        m.internet_message_id = String::new();
        a.ingest(m.clone());
        b.ingest(m);
        a.sync(&shared).unwrap();
        b.sync(&shared).unwrap();
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM Tickets"), 2, "uncertain matches stay separate");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketRelations WHERE relation_type='possible_duplicate'"), 1, "routed to review");
    }

    #[test]
    fn read_only_database_is_pulled_but_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("A");
        a.connect(&shared);
        a.ingest(msg(1, "C1", "Help", "body", "x@y.org"));
        a.sync(&shared).unwrap();
        let b = Client::new("B");
        b.connect(&shared);
        b.ingest(msg(2, "C2", "Other", "body", "z@y.org"));
        let mut perm = std::fs::metadata(&shared).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&shared, perm.clone()).unwrap();
        let mut c = ss::open(&shared, true).unwrap();
        let r = run_cycle(&b.local, c.as_mut(), &b.dev, "ASR Test Mailbox");
        drop(c);
        perm.set_readonly(false);
        std::fs::set_permissions(&shared, perm).unwrap();
        let r = r.unwrap();
        assert!(!r.writable);
        assert_eq!(r.pushed, 0);
        assert_eq!(b.count("SELECT COUNT(*) FROM sync_ops WHERE state='pending'"), 1, "kept pending");
        assert_eq!(b.count("SELECT COUNT(*) FROM tickets WHERE shared_state='shared'"), 1, "pull still works");
    }

    /// Two simulated workstations against a REAL Access test database (never the production path):
    ///   set ASR_ACCESS_TEST_DIR=C:\local\folder && cargo test --no-default-features -- --ignored access_two_clients --nocapture
    #[test]
    #[ignore]
    fn access_two_clients_converge() {
        let Ok(dir) = std::env::var("ASR_ACCESS_TEST_DIR") else { return };
        let p = std::path::PathBuf::from(dir).join(format!("asr-sync-{}.accdb", uuid::Uuid::new_v4()));
        ss::create_database(&p, "Access sync test", "test", "cargo test").unwrap();
        let a = Client::new("A");
        let b = Client::new("B");
        a.connect(&p);
        b.connect(&p);
        a.ingest(msg(1, "C1", "Subtype for 3 schools", "NJIIS 0012345 ünïcødé ✓", "nurse@school.org"));
        b.ingest(msg(1, "C1", "Subtype for 3 schools", "NJIIS 0012345 ünïcødé ✓", "nurse@school.org"));
        let ta = a.ticket_by_imid("<m1@x>");
        crate::notes::add(&a.conn(), ta, &"Long note ".repeat(100), "Aaron").unwrap();
        let tb = b.ticket_by_imid("<m1@x>");
        crate::notes::add(&b.conn(), tb, "Second note", "Wenjie").unwrap();
        let ra = a.sync(&p).unwrap();
        let rb = b.sync(&p).unwrap();
        println!("A: {ra:?}\nB: {rb:?}");
        for cl in [&a, &b] {
            assert_eq!(cl.count("SELECT COUNT(*) FROM sync_ops WHERE state<>'accepted'"), 0, "{}: every op accepted", cl.name);
        }
        a.sync(&p).unwrap();
        b.sync(&p).unwrap();
        assert_eq!(shared_count(&p, "SELECT COUNT(*) FROM Tickets"), 1);
        assert_eq!(shared_count(&p, "SELECT COUNT(*) FROM TicketNotes"), 2);
        assert_eq!(a.shared_id(a.ticket_by_imid("<m1@x>")), b.shared_id(b.ticket_by_imid("<m1@x>")));
        let mut c = ss::open(&p, false).unwrap();
        let body = c.query("SELECT body_text FROM Emails", &[]).unwrap();
        assert!(ss::col_s(&body[0], 0).contains("ünïcødé ✓"), "unicode round trip");
        // lost acknowledgement against Access
        let ta = a.ticket_by_imid("<m1@x>");
        tickets::set_field(&a.conn(), ta, "school_name", Some("Maple".into()), false, "A").unwrap();
        let db = q::bound_db(&a.conn()).unwrap();
        let (_, op) = q::next_batch(&a.conn(), &db, 10).unwrap().remove(0);
        sa::apply(c.as_mut(), &op).unwrap();
        drop(c);
        let r = a.sync(&p).unwrap();
        assert_eq!(r.replayed, 1);
        assert_eq!(shared_count(&p, &format!("SELECT COUNT(*) FROM ActivityLog WHERE op_id='{}'", op.op_id)), 1);
        println!("Access file: {}", p.display());
    }
}
