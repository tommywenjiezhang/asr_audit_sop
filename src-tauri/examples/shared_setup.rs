//! Administrator tool for the one-time shared database setup (no Tauri; uses the same core code as the app).
//!
//!   cargo run --no-default-features --example shared_setup -- inspect <db.accdb>
//!   cargo run --no-default-features --example shared_setup -- init    <db.accdb> <display name> <test|production>
//!   cargo run --no-default-features --example shared_setup -- sync    <support_desk.db> <db.accdb> <mailbox name>
//!   cargo run --no-default-features --example shared_setup -- verify  <support_desk.db> <db.accdb>
//!
//! `init` only accepts an existing EMPTY database (verified backup first). `sync` backs up the local
//! database, connects (binds this computer to the shared database identity), queues every local-only case
//! with the normal restartable import, and runs synchronization cycles until nothing is pending.
//! Run it only while the desktop app is closed on this computer.

use asr_support_desk_lib::{db, shared_store as ss, shared_sync as shs, sync_queue as q};
use std::path::PathBuf;
use std::sync::Mutex;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = match a.first().map(|s| s.as_str()) {
        Some("inspect") if a.len() == 2 => inspect(&a[1]),
        Some("init") if a.len() == 4 => init(&a[1], &a[2], &a[3]),
        Some("sync") if a.len() == 4 => sync(&a[1], &a[2], &a[3]),
        Some("verify") if a.len() == 3 => verify(&a[1], &a[2]),
        _ => Err("usage: inspect <accdb> | init <accdb> <name> <env> | sync <local.db> <accdb> <mailbox> | verify <local.db> <accdb>".into()),
    };
    if let Err(e) = r {
        eprintln!("FAILED: {e}");
        std::process::exit(1);
    }
}

type R = Result<(), String>;

fn inspect(path: &str) -> R {
    let p = PathBuf::from(path);
    let mut c = ss::open(&p, true).map_err(|e| format!("{} {}", e.code(), e.message))?;
    let tables = c.user_tables().map_err(|e| e.message)?;
    println!("user tables/queries: {}", tables.len());
    for t in &tables {
        println!("  {t}");
    }
    match ss::identity(c.as_mut()) {
        Ok(id) => println!("ASR shared database: {:?}", id),
        Err(e) => println!("not an ASR shared database: {} {}", e.code(), e.message),
    }
    Ok(())
}

fn init(path: &str, name: &str, env: &str) -> R {
    if !["test", "production"].contains(&env) {
        return Err("environment must be test or production".into());
    }
    let who = format!("shared_setup ({})", q::windows_account());
    let rep = ss::initialize_existing(&PathBuf::from(path), name, env, &who).map_err(|e| format!("{} {}", e.code(), e.message))?;
    println!("initialized database_id={} name=\"{}\" env={} schema v{}", rep.identity.database_id, rep.identity.display_name, rep.identity.environment, rep.identity.schema_version);
    println!("verified backup: {} (sha256 {})", rep.backup_path, rep.backup_sha256);
    println!("tables ({}): {}", rep.tables_created.len(), rep.tables_created.join(", "));
    Ok(())
}

fn local_counts(conn: &rusqlite::Connection) -> Vec<(&'static str, i64)> {
    let g = |s: &str| conn.query_row(s, [], |r| r.get::<_, i64>(0)).unwrap_or(-1);
    vec![
        ("tickets (not merged)", g("SELECT COUNT(*) FROM tickets WHERE merged_into IS NULL")),
        ("  local_only", g("SELECT COUNT(*) FROM tickets WHERE shared_state='local_only' AND merged_into IS NULL")),
        ("  shared", g("SELECT COUNT(*) FROM tickets WHERE shared_state='shared' AND merged_into IS NULL")),
        ("  merged into another case", g("SELECT COUNT(*) FROM tickets WHERE merged_into IS NOT NULL")),
        ("email links", g("SELECT COUNT(*) FROM ticket_email_links")),
        ("notes", g("SELECT COUNT(*) FROM ticket_notes")),
        ("ownership history", g("SELECT COUNT(*) FROM ownership_history")),
        ("completion history", g("SELECT COUNT(*) FROM completion_history")),
        ("staff", g("SELECT COUNT(*) FROM staff")),
        ("sync_ops pending", g("SELECT COUNT(*) FROM sync_ops WHERE state='pending'")),
        ("sync_ops held", g("SELECT COUNT(*) FROM sync_ops WHERE state='held'")),
        ("sync_ops accepted", g("SELECT COUNT(*) FROM sync_ops WHERE state='accepted'")),
        ("sync_ops conflict", g("SELECT COUNT(*) FROM sync_ops WHERE state='conflict'")),
        ("sync_ops rejected", g("SELECT COUNT(*) FROM sync_ops WHERE state='rejected'")),
        ("sync_conflicts open", g("SELECT COUNT(*) FROM sync_conflicts WHERE resolved_at IS NULL")),
    ]
}

fn print_counts(title: &str, v: &[(&str, i64)]) {
    println!("{title}");
    for (k, n) in v {
        println!("  {k:<28} {n}");
    }
}

fn sync(local: &str, path: &str, mailbox: &str) -> R {
    let lp = PathBuf::from(local);
    let sp = PathBuf::from(path);
    let conn = db::open(&lp).map_err(|e| e.to_string())?;
    // same startup step as the app: the issue-type catalog (pulled cases reference it)
    asr_support_desk_lib::templates_store::seed_embedded(&conn).map_err(|e| e.to_string())?;
    // consistent local backup before anything is queued
    let bdir = lp.parent().unwrap().join("backups");
    std::fs::create_dir_all(&bdir).map_err(|e| e.to_string())?;
    let bk = bdir.join(format!("pre-shared-import-{}.db", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    asr_support_desk_lib::export::backup(&conn, &bk).map_err(|e| e.to_string())?;
    println!("local backup: {}", bk.display());
    if q::mode(&conn) != q::MODE_SHARED {
        return Err("local database is in Standalone mode; nothing done".into());
    }
    print_counts("local before:", &local_counts(&conn));
    let held_before: i64 = conn.query_row("SELECT COUNT(*) FROM sync_ops WHERE state='held'", [], |r| r.get(0)).unwrap();
    let dev = q::device_info(&conn, APP_VERSION).map_err(|e| e.to_string())?;
    let actor = q::actor(&conn, &std::env::var("USERNAME").unwrap_or_default()).name;
    let local = Mutex::new(conn);

    // 1. connect: probe + first cycle binds this computer to the database identity
    let (probe, cycle) = shs::connect_and_sync(&local, &sp, &dev, mailbox, true);
    println!("probe: reachable={} readable={} writable={} compat={} err={:?}", probe.reachable, probe.readable, probe.writable, probe.compatibility, probe.error.as_ref().map(|e| e.code()));
    match cycle {
        Some(Ok(r)) => println!("cycle 0: {r:?}"),
        Some(Err(e)) => return Err(format!("first cycle failed: {} {}", e.code(), e.message)),
        None => return Err("not connected (see probe)".into()),
    }
    if held_before > 0 {
        println!("NOTE: {held_before} operation(s) were held before this run; they are left held for review in Settings.");
    }
    // 2. queue the import (restartable batches)
    let mut queued = 0;
    loop {
        let n = q::queue_import(&local.lock().unwrap(), &actor, 500).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        queued += n;
    }
    println!("queued {queued} local case(s) for import (actor \"{actor}\")");
    // 3. push/pull until nothing is pending (each cycle: one short Access transaction per operation)
    let mut last_pending = i64::MAX;
    for i in 1..=200 {
        let mut c = ss::open(&sp, false).map_err(|e| format!("{} {}", e.code(), e.message))?;
        let r = shs::run_cycle(&local, c.as_mut(), &dev, mailbox).map_err(|e| format!("cycle {i}: {} {}", e.code(), e.message))?;
        let s = q::summary(&local.lock().unwrap()).map_err(|e| e.to_string())?;
        println!("cycle {i}: pushed {} accepted {} conflicts {} rejected {} replayed {} pulled {} | pending {} ({} ms)", r.pushed, r.accepted, r.conflicts, r.rejected, r.replayed, r.pulled, s.pending, r.elapsed_ms);
        if s.pending == 0 && r.pushed == 0 {
            break;
        }
        if s.pending >= last_pending && r.pushed == 0 {
            return Err(format!("no progress; {} pending; last error {:?}", s.pending, s.last_error));
        }
        last_pending = s.pending;
    }
    let conn = local.into_inner().unwrap();
    crate_diag(&conn, queued);
    print_counts("local after:", &local_counts(&conn));
    Ok(())
}

fn crate_diag(conn: &rusqlite::Connection, queued: usize) {
    asr_support_desk_lib::diag::record(conn, asr_support_desk_lib::diag::Severity::Info, "migration", "IMPORT_QUEUED", &format!("{queued} local case(s) imported into the shared database (setup tool)"), "", "");
}

fn verify(local: &str, path: &str) -> R {
    let conn = rusqlite::Connection::open_with_flags(local, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    print_counts("local:", &local_counts(&conn));
    let mut c = ss::open(&PathBuf::from(path), true).map_err(|e| format!("{} {}", e.code(), e.message))?;
    let id = ss::identity(c.as_mut()).map_err(|e| e.message)?;
    println!("shared: {} ({}) id {} schema v{}", id.display_name, id.environment, id.database_id, id.schema_version);
    let mut n = |sql: &str| c.query(sql, &[]).ok().and_then(|r| r.first().and_then(|x| x[0].clone())).unwrap_or_else(|| "?".into());
    for (k, sql) in [
        ("Tickets", "SELECT COUNT(*) FROM Tickets"),
        ("  not merged", "SELECT COUNT(*) FROM Tickets WHERE merged_into IS NULL"),
        ("Emails", "SELECT COUNT(*) FROM Emails"),
        ("  distinct match_key", "SELECT COUNT(*) FROM (SELECT DISTINCT match_key FROM Emails WHERE match_key IS NOT NULL)"),
        ("  with match_key", "SELECT COUNT(*) FROM Emails WHERE match_key IS NOT NULL"),
        ("TicketEmails", "SELECT COUNT(*) FROM TicketEmails"),
        ("TicketNotes", "SELECT COUNT(*) FROM TicketNotes"),
        ("TicketFields", "SELECT COUNT(*) FROM TicketFields"),
        ("TicketIssueTypes", "SELECT COUNT(*) FROM TicketIssueTypes"),
        ("TicketSchools", "SELECT COUNT(*) FROM TicketSchools"),
        ("OwnershipHistory", "SELECT COUNT(*) FROM OwnershipHistory"),
        ("CompletionHistory", "SELECT COUNT(*) FROM CompletionHistory"),
        ("ActivityLog", "SELECT COUNT(*) FROM ActivityLog"),
        ("ProcessedOperations", "SELECT COUNT(*) FROM ProcessedOperations"),
        ("  distinct op_id", "SELECT COUNT(*) FROM (SELECT DISTINCT op_id FROM ProcessedOperations)"),
        ("SyncConflicts", "SELECT COUNT(*) FROM SyncConflicts"),
        ("TicketRelations", "SELECT COUNT(*) FROM TicketRelations"),
        ("Staff", "SELECT COUNT(*) FROM Staff"),
        ("Devices", "SELECT COUNT(*) FROM Devices"),
        ("Mailboxes", "SELECT COUNT(*) FROM Mailboxes"),
        ("ChangeFeed", "SELECT COUNT(*) FROM ChangeFeed"),
    ] {
        println!("  {k:<22} {}", n(sql));
    }
    // per-status comparison
    let shared_status = c.query("SELECT ticket_status, COUNT(*) FROM Tickets WHERE merged_into IS NULL GROUP BY ticket_status", &[]).map_err(|e| e.message)?;
    println!("shared by status: {:?}", shared_status.iter().map(|r| (ss::col_s(r, 0), ss::col_s(r, 1))).collect::<Vec<_>>());
    let mut st = conn.prepare("SELECT status, COUNT(*) FROM tickets WHERE merged_into IS NULL GROUP BY status").unwrap();
    let local_status: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    println!("local  by status: {local_status:?}");
    // every local shared ticket exists in Access with the same version
    let mut st = conn.prepare("SELECT shared_id, shared_version FROM tickets WHERE shared_state='shared' AND merged_into IS NULL").unwrap();
    let local_ids: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    let remote: std::collections::HashMap<String, i64> = c
        .query("SELECT ticket_id, row_version FROM Tickets", &[])
        .map_err(|e| e.message)?
        .iter()
        .map(|r| (ss::col_s(r, 0), ss::col_i(r, 1)))
        .collect();
    let missing = local_ids.iter().filter(|(id, _)| !remote.contains_key(id)).count();
    let version_diff = local_ids.iter().filter(|(id, v)| remote.get(id).map(|rv| rv != v).unwrap_or(false)).count();
    println!("local shared cases: {}; missing in Access: {missing}; version differs: {version_diff}", local_ids.len());
    Ok(())
}
