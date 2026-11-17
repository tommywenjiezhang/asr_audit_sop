//! SQLite connection, pragmas and versioned migrations (PRAGMA user_version).

use crate::error::AppResult;
use rusqlite::Connection;
use std::path::Path;

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

const MIGRATIONS: &[&str] = &[
    // v1 - initial schema
    r#"
    CREATE TABLE app_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE TABLE issue_definitions (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        description TEXT,
        sop_section TEXT,
        sop_page TEXT,
        fields_json TEXT NOT NULL,
        keywords_json TEXT NOT NULL,
        internal_steps TEXT,
        default_template_id TEXT,
        sort_order INTEGER NOT NULL DEFAULT 100,
        enabled INTEGER NOT NULL DEFAULT 1,
        catalog_version INTEGER NOT NULL
    );

    CREATE TABLE email_templates (
        id TEXT PRIMARY KEY,
        issue_type TEXT NOT NULL REFERENCES issue_definitions(id),
        name TEXT NOT NULL,
        description TEXT,
        sop_section TEXT,
        sop_page TEXT,
        sop_status TEXT NOT NULL,
        version INTEGER NOT NULL DEFAULT 1,
        system_version INTEGER,
        priority INTEGER NOT NULL DEFAULT 100,
        required_fields TEXT NOT NULL DEFAULT '[]',
        optional_fields TEXT NOT NULL DEFAULT '[]',
        rules_json TEXT NOT NULL DEFAULT '[]',
        asserts_json TEXT NOT NULL DEFAULT '[]',
        subject_template TEXT NOT NULL,
        body_html TEXT NOT NULL,
        default_subject TEXT,
        default_body_html TEXT,
        is_system INTEGER NOT NULL DEFAULT 0,
        is_customized INTEGER NOT NULL DEFAULT 0,
        system_update_available INTEGER NOT NULL DEFAULT 0,
        enabled INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        last_modified TEXT NOT NULL
    );
    CREATE INDEX idx_templates_issue ON email_templates(issue_type, enabled, priority);

    CREATE TABLE email_template_versions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        template_id TEXT NOT NULL REFERENCES email_templates(id) ON DELETE CASCADE,
        version INTEGER NOT NULL,
        subject_template TEXT NOT NULL,
        body_html TEXT NOT NULL,
        note TEXT,
        saved_at TEXT NOT NULL
    );
    CREATE INDEX idx_template_versions ON email_template_versions(template_id, version);

    CREATE TABLE tickets (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        status TEXT NOT NULL DEFAULT 'New',
        priority TEXT NOT NULL DEFAULT 'Normal',
        issue_type TEXT NOT NULL DEFAULT 'other' REFERENCES issue_definitions(id),
        issue_type_source TEXT NOT NULL DEFAULT 'auto',
        classification_confidence REAL,
        school_njiis_id TEXT,
        school_name TEXT,
        school_verification TEXT NOT NULL DEFAULT 'Unverified School',
        sender_name TEXT,
        sender_email TEXT,
        original_subject TEXT,
        assigned_staff TEXT,
        conversation_id TEXT,
        possible_duplicate_of INTEGER REFERENCES tickets(id) ON DELETE SET NULL,
        internal_notes TEXT,
        resolution_notes TEXT,
        next_followup_date TEXT,
        source TEXT NOT NULL DEFAULT 'outlook',
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        last_email_received_at TEXT,
        last_response_at TEXT
    );
    CREATE INDEX idx_tickets_status ON tickets(status, updated_at DESC);
    CREATE INDEX idx_tickets_updated ON tickets(updated_at DESC);
    CREATE INDEX idx_tickets_conversation ON tickets(conversation_id);
    CREATE INDEX idx_tickets_sender ON tickets(sender_email, issue_type);
    CREATE INDEX idx_tickets_school ON tickets(school_njiis_id);

    CREATE TABLE ticket_fields (
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        field_key TEXT NOT NULL,
        extracted_value TEXT,
        extraction_confidence TEXT,
        manual_value TEXT,
        verified INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (ticket_id, field_key)
    );

    CREATE TABLE email_messages (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        store_id TEXT,
        entry_id TEXT,
        internet_message_id TEXT,
        conversation_id TEXT,
        folder_entry_id TEXT,
        folder_path TEXT,
        subject TEXT,
        sender_name TEXT,
        sender_email TEXT,
        received_at TEXT,
        body_text TEXT,
        body_truncated INTEGER NOT NULL DEFAULT 0,
        has_attachments INTEGER NOT NULL DEFAULT 0,
        is_internal INTEGER NOT NULL DEFAULT 0,
        metadata_json TEXT,
        imported_at TEXT NOT NULL
    );
    CREATE UNIQUE INDEX idx_messages_imid ON email_messages(internet_message_id) WHERE internet_message_id IS NOT NULL AND internet_message_id <> '';
    CREATE INDEX idx_messages_entry ON email_messages(entry_id);
    CREATE INDEX idx_messages_conversation ON email_messages(conversation_id);

    CREATE TABLE ticket_email_links (
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        message_id INTEGER NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
        link_type TEXT NOT NULL,
        linked_at TEXT NOT NULL,
        PRIMARY KEY (ticket_id, message_id)
    );
    CREATE INDEX idx_links_message ON ticket_email_links(message_id);

    CREATE TABLE ticket_history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        at TEXT NOT NULL,
        actor TEXT NOT NULL,
        action TEXT NOT NULL,
        field TEXT,
        old_value TEXT,
        new_value TEXT
    );
    CREATE INDEX idx_history_ticket ON ticket_history(ticket_id, id);

    CREATE TABLE email_drafts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        ticket_id INTEGER NOT NULL UNIQUE REFERENCES tickets(id) ON DELETE CASCADE,
        template_id TEXT,
        subject TEXT NOT NULL,
        body_html TEXT NOT NULL,
        is_user_edited INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL DEFAULT 'Draft',
        outlook_entry_id TEXT,
        outlook_created_at TEXT,
        version INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );

    CREATE TABLE email_draft_versions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        draft_id INTEGER NOT NULL REFERENCES email_drafts(id) ON DELETE CASCADE,
        version INTEGER NOT NULL,
        template_id TEXT,
        subject TEXT NOT NULL,
        body_html TEXT NOT NULL,
        reason TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_draft_versions ON email_draft_versions(draft_id, version);

    CREATE TABLE sync_state (
        folder_key TEXT PRIMARY KEY,
        store_id TEXT,
        folder_entry_id TEXT,
        folder_path TEXT,
        last_success_at TEXT,
        last_received_at TEXT,
        last_full_scan_at TEXT,
        last_run_at TEXT,
        last_error TEXT,
        last_error_at TEXT,
        messages_imported INTEGER NOT NULL DEFAULT 0
    );
    "#,
    // v2 - two statuses (In Progress / Done), multiple issues and schools per ticket,
    //      Copilot prompt, Outlook completion progress, full-text search
    r#"
    INSERT INTO ticket_history(ticket_id,at,actor,action,field,old_value,new_value)
        SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ','now'), 'system', 'Status migrated to two-status workflow', 'status', status,
               CASE WHEN status IN ('Resolved','Closed') THEN 'Done' ELSE 'In Progress' END
        FROM tickets WHERE status NOT IN ('In Progress','Done');

    ALTER TABLE tickets ADD COLUMN needs_review INTEGER NOT NULL DEFAULT 0;
    UPDATE tickets SET needs_review = 1 WHERE status = 'Needs Review';

    ALTER TABLE tickets ADD COLUMN completed_at TEXT;
    UPDATE tickets SET completed_at = updated_at WHERE status IN ('Resolved','Closed');
    UPDATE tickets SET status = CASE WHEN status IN ('Resolved','Closed') THEN 'Done' ELSE 'In Progress' END;

    ALTER TABLE tickets ADD COLUMN first_received_at TEXT;
    UPDATE tickets SET first_received_at = COALESCE(
        (SELECT MIN(m.received_at) FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
          WHERE l.ticket_id=tickets.id AND COALESCE(m.received_at,'')<>''),
        created_at);
    ALTER TABLE tickets ADD COLUMN staff_actions TEXT;
    ALTER TABLE tickets ADD COLUMN last_prompt TEXT;
    ALTER TABLE tickets ADD COLUMN last_prompt_at TEXT;
    CREATE INDEX idx_tickets_received ON tickets(first_received_at DESC);
    CREATE INDEX idx_tickets_completed ON tickets(completed_at DESC);

    CREATE TABLE ticket_issues (
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        issue_type TEXT NOT NULL,
        source TEXT NOT NULL DEFAULT 'auto',
        position INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (ticket_id, issue_type)
    );
    CREATE INDEX idx_ticket_issues_type ON ticket_issues(issue_type);
    INSERT INTO ticket_issues(ticket_id, issue_type, source, position)
        SELECT id, issue_type, issue_type_source, 0 FROM tickets;

    CREATE TABLE ticket_schools (
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        slot INTEGER NOT NULL,
        removed INTEGER NOT NULL DEFAULT 0,
        created_by TEXT NOT NULL DEFAULT 'system',
        created_at TEXT NOT NULL,
        PRIMARY KEY (ticket_id, slot)
    );
    INSERT INTO ticket_schools(ticket_id, slot, created_at) SELECT id, 1, created_at FROM tickets;

    CREATE TABLE message_completion (
        message_id INTEGER PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        flagged_at TEXT,
        moved_at TEXT,
        dest_store_id TEXT,
        dest_folder_entry_id TEXT,
        dest_path TEXT,
        last_error TEXT,
        last_error_at TEXT,
        attempts INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_completion_ticket ON message_completion(ticket_id);

    CREATE VIRTUAL TABLE ticket_search USING fts5(content, tokenize = 'unicode61 remove_diacritics 2');
    "#,
    // v3 - staff assignment, ownership from Outlook folders, current/history scope membership.
    //      Existing statuses (In Progress / Done) are kept; Unassigned and Assigned are added.
    r#"
    CREATE TABLE staff (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        display_name TEXT NOT NULL,
        email TEXT NOT NULL,
        mailbox_name TEXT,
        store_id TEXT NOT NULL,
        folder_entry_id TEXT NOT NULL,
        folder_path TEXT NOT NULL,
        active INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );

    ALTER TABLE tickets ADD COLUMN assignee_staff_id INTEGER REFERENCES staff(id);
    ALTER TABLE tickets ADD COLUMN assigned_at TEXT;
    ALTER TABLE tickets ADD COLUMN ownership_conflict INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE tickets ADD COLUMN ticket_ref TEXT;
    -- 'current' | 'history' | 'unverified' (independent of the workflow status)
    ALTER TABLE tickets ADD COLUMN scope_state TEXT NOT NULL DEFAULT 'unverified';
    ALTER TABLE tickets ADD COLUMN scope_reason TEXT;
    ALTER TABLE tickets ADD COLUMN scope_changed_at TEXT;
    CREATE INDEX idx_tickets_scope ON tickets(scope_state, status);
    CREATE INDEX idx_tickets_assignee ON tickets(assignee_staff_id, status);

    -- 'in' | 'out' | 'unknown' membership of the selected scope, plus the last confirmed location
    ALTER TABLE email_messages ADD COLUMN scope_state TEXT NOT NULL DEFAULT 'unknown';
    ALTER TABLE email_messages ADD COLUMN scope_key TEXT;
    ALTER TABLE email_messages ADD COLUMN location_status TEXT;
    ALTER TABLE email_messages ADD COLUMN location_checked_at TEXT;
    ALTER TABLE email_messages ADD COLUMN out_reason TEXT;
    ALTER TABLE email_messages ADD COLUMN folder_ancestors TEXT;
    ALTER TABLE email_messages ADD COLUMN owner_staff_id INTEGER;

    CREATE TABLE assignment_ops (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        from_staff_id INTEGER,
        to_staff_id INTEGER NOT NULL,
        operator TEXT NOT NULL,
        note TEXT,
        created_at TEXT NOT NULL,
        -- 'moving' | 'failed' | 'done' | 'superseded'
        state TEXT NOT NULL,
        dest_store_id TEXT NOT NULL,
        dest_folder_entry_id TEXT NOT NULL,
        dest_path TEXT NOT NULL,
        prev_folders TEXT,
        finished_at TEXT,
        -- 'none' | 'pending' | 'sending' | 'submitted' | 'failed' | 'uncertain' | 'skipped'
        notify_state TEXT NOT NULL DEFAULT 'none',
        notify_id TEXT,
        notify_recipient TEXT,
        notify_account TEXT,
        notify_attempts INTEGER NOT NULL DEFAULT 0,
        notify_at TEXT,
        notify_error TEXT,
        notify_entry_id TEXT,
        notify_checked_at TEXT
    );
    CREATE INDEX idx_assignment_ticket ON assignment_ops(ticket_id, id);

    CREATE TABLE assignment_moves (
        op_id INTEGER NOT NULL REFERENCES assignment_ops(id) ON DELETE CASCADE,
        message_id INTEGER NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
        moved_at TEXT,
        new_folder_path TEXT,
        last_error TEXT,
        last_error_at TEXT,
        attempts INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (op_id, message_id)
    );
    "#,
];

/// Open (and migrate) the database file. Before applying migrations to an existing database,
/// a consistent copy is written to `<db dir>/backups/pre-migration-v<N>-<timestamp>.db`.
pub fn open(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    let current = schema_version(&conn)? as usize;
    if current > 0 && current < MIGRATIONS.len() {
        let dir = path.parent().unwrap_or(Path::new(".")).join("backups");
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(format!("pre-migration-v{current}-{}.db", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        crate::export::backup(&conn, &dest)
            .map_err(|e| crate::error::AppError::coded("BACKUP_FAILED", format!("Database not upgraded: the safety backup failed ({e}).")))?;
        tracing::info!(path = %dest.display(), from = current, "database backed up before migration");
    }
    migrate(&conn)?;
    after_migrate(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> AppResult<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    after_migrate(&conn)?;
    Ok(conn)
}

/// Work that needs Rust code after the SQL migrations (e.g. building the search index).
pub fn after_migrate(conn: &Connection) -> AppResult<()> {
    let indexed: i64 = conn.query_row("SELECT COUNT(*) FROM ticket_search", [], |r| r.get(0))?;
    let tickets: i64 = conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0))?;
    if indexed != tickets {
        crate::search::rebuild_all(conn)?;
    }
    // v3 follow-up (repeatable, safe to resume): stable ticket references, then a first scope pass.
    // Every ticket with Outlook emails stays "Awaiting Verification" until a reconciliation confirms its scope.
    let missing_refs: Vec<i64> = {
        let mut st = conn.prepare("SELECT id FROM tickets WHERE ticket_ref IS NULL")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    if !missing_refs.is_empty() {
        let tx = conn.unchecked_transaction()?;
        for id in &missing_refs {
            crate::tickets::ensure_ticket_ref(&tx, *id)?;
        }
        tx.commit()?;
    }
    if get_setting(conn, "v3_scope_init")?.is_none() {
        let tx = conn.unchecked_transaction()?;
        let ids: Vec<i64> = {
            let mut st = tx.prepare("SELECT id FROM tickets")?;
            let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            v
        };
        for id in ids {
            crate::scope::recompute_ticket(&tx, id, "system")?;
        }
        set_setting(&tx, "v3_scope_init", &now())?;
        tx.commit()?;
    }
    Ok(())
}

fn configure(conn: &Connection) -> AppResult<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // WAL for file DBs; in-memory DBs report "memory" which is fine.
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY;")?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

pub fn migrate(conn: &Connection) -> AppResult<()> {
    let current = schema_version(conn)? as usize;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        tx.commit()?;
        tracing::info!(version = i + 1, "applied database migration");
    }
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row("SELECT value FROM app_settings WHERE key=?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO app_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrations_apply_and_are_idempotent() {
        let c = open_in_memory().unwrap();
        assert_eq!(schema_version(&c).unwrap() as usize, MIGRATIONS.len());
        migrate(&c).unwrap();
        let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 1);
    }

    /// Manual check against a COPY of a real database (never the live file):
    ///   set ASR_DB_COPY=<copy path> && cargo test --no-default-features -- --ignored migrate_real_copy --nocapture
    #[test]
    #[ignore]
    fn migrate_real_copy() {
        let Ok(p) = std::env::var("ASR_DB_COPY") else { return };
        let path = std::path::PathBuf::from(p);
        let count = |c: &Connection, sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        let tables = ["tickets", "ticket_fields", "email_templates", "email_messages", "ticket_email_links", "ticket_history", "email_drafts"];
        let before: Vec<i64> = {
            let c = Connection::open(&path).unwrap();
            tables.iter().map(|t| count(&c, &format!("SELECT COUNT(*) FROM {t}"))).collect()
        };
        let not_target = {
            let c = Connection::open(&path).unwrap();
            count(&c, "SELECT COUNT(*) FROM tickets WHERE status NOT IN ('In Progress','Done')")
        };
        let c = open(&path).unwrap();
        let after: Vec<i64> = tables.iter().map(|t| count(&c, &format!("SELECT COUNT(*) FROM {t}"))).collect();
        for (i, t) in tables.iter().enumerate() {
            let expected = if *t == "ticket_history" { before[i] + not_target } else { before[i] };
            println!("{t}: before {} after {}", before[i], after[i]);
            assert_eq!(after[i], expected, "{t}");
        }
        let mut st = c.prepare("SELECT status, COUNT(*), SUM(needs_review) FROM tickets GROUP BY status").unwrap();
        let rows: Vec<(String, i64, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect();
        println!("statuses after: {rows:?}");
        assert!(rows.iter().all(|(s, _, _)| s == "In Progress" || s == "Done"));
        println!("ticket_issues {} ticket_schools {} search docs {}",
            count(&c, "SELECT COUNT(*) FROM ticket_issues"), count(&c, "SELECT COUNT(*) FROM ticket_schools"), count(&c, "SELECT COUNT(*) FROM ticket_search"));
        assert_eq!(count(&c, "SELECT COUNT(*) FROM ticket_search"), after[0]);
        let integrity: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(integrity, "ok");
        let s = crate::settings::Settings::default();
        let first: i64 = c.query_row("SELECT MIN(id) FROM tickets", [], |r| r.get(0)).unwrap();
        let p = crate::prompt::build_prompt(&c, first, &s).unwrap();
        println!("prompt for first ticket: {} characters, {} lines", p.chars().count(), p.lines().count());
        if let Ok(id) = std::env::var("ASR_PRINT_PROMPT") {
            println!("=====\n{}\n=====", crate::prompt::build_prompt(&c, id.parse().unwrap_or(first), &s).unwrap());
        }
        let backups = std::fs::read_dir(path.parent().unwrap().join("backups")).unwrap().count();
        println!("backups written: {backups}");
        assert!(backups >= 1);
    }

    #[test]
    fn v2_database_migrates_to_v3_keeping_statuses_and_awaiting_verification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("support_desk.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.execute_batch(MIGRATIONS[1]).unwrap();
            c.execute_batch("PRAGMA user_version=2").unwrap();
            c.execute("INSERT INTO issue_definitions(id,name,fields_json,keywords_json,catalog_version) VALUES('other','Other','[]','[]',1)", []).unwrap();
            for (i, s) in ["In Progress", "Done"].iter().enumerate() {
                c.execute("INSERT INTO tickets(status,issue_type,created_at,updated_at,completed_at) VALUES(?1,'other','2026-11-0'||?2||'T10:00:00Z','x',?3)",
                    rusqlite::params![s, i + 1, if *s == "Done" { Some("2026-11-02T12:00:00Z") } else { None }]).unwrap();
            }
            c.execute("INSERT INTO email_messages(entry_id,internet_message_id,subject,received_at,body_text,imported_at) VALUES('E1','<a@x>','s','2026-10-30T12:00:00Z','b','x')", []).unwrap();
            c.execute("INSERT INTO ticket_email_links(ticket_id,message_id,link_type,linked_at) VALUES(1,1,'original','x')", []).unwrap();
            c.execute("INSERT INTO message_completion(message_id,ticket_id,flagged_at,moved_at) VALUES(1,1,'x','x')", []).unwrap();
            c.execute("INSERT INTO ticket_history(ticket_id,at,actor,action) VALUES(2,'x','staff','Marked Done')", []).unwrap();
        }
        let c = open(&path).unwrap();
        assert_eq!(schema_version(&c).unwrap() as usize, MIGRATIONS.len());
        let st: Vec<(String, String, Option<String>)> = {
            let mut q = c.prepare("SELECT status, scope_state, ticket_ref FROM tickets ORDER BY id").unwrap();
            let v = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect();
            v
        };
        assert_eq!((st[0].0.as_str(), st[1].0.as_str()), ("In Progress", "Done"), "existing statuses preserved");
        assert_eq!(st[0].1, "unverified", "email tickets await verification - not claimed current");
        assert_eq!(st[1].1, "history", "a Done ticket without Outlook emails is history");
        assert!(st.iter().all(|t| t.2.as_deref().unwrap_or("").starts_with("ASR-")), "stable references created");
        let kept: (i64, i64) = c.query_row("SELECT (SELECT COUNT(*) FROM message_completion), (SELECT COUNT(*) FROM ticket_history WHERE action='Marked Done')", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(kept, (1, 1), "completion and history records kept");
        assert!(std::fs::read_dir(dir.path().join("backups")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("pre-migration-v2-")));
        drop(c);
        let c = open(&path).unwrap(); // repeatable / resumable
        let refs: i64 = c.query_row("SELECT COUNT(DISTINCT ticket_ref) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(refs, 2);
    }

    #[test]
    fn v1_database_migrates_to_two_statuses_without_data_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("support_desk.db");
        let old = ["New", "Needs Review", "In Progress", "Waiting for User", "Waiting for Internal Action", "Resolved", "Closed"];
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.execute_batch("PRAGMA user_version=1").unwrap();
            c.execute("INSERT INTO issue_definitions(id,name,fields_json,keywords_json,catalog_version) VALUES('other','Other','[]','[]',1)", []).unwrap();
            for (i, s) in old.iter().enumerate() {
                let ts = format!("2026-11-0{}T10:00:00Z", i + 1);
                c.execute("INSERT INTO tickets(status,issue_type,original_subject,created_at,updated_at) VALUES(?1,'other',?2,?3,?3)", rusqlite::params![s, format!("subject {i}"), ts]).unwrap();
            }
            c.execute("INSERT INTO ticket_fields(ticket_id,field_key,extracted_value,manual_value,verified,updated_at) VALUES(1,'school_njiis_id','111','0012345',1,'x')", []).unwrap();
            c.execute("INSERT INTO email_messages(entry_id,subject,received_at,body_text,imported_at) VALUES('E1','s','2026-10-30T12:00:00Z','Original body text','x')", []).unwrap();
            c.execute("INSERT INTO ticket_email_links(ticket_id,message_id,link_type,linked_at) VALUES(1,1,'original','x')", []).unwrap();
            c.execute("INSERT INTO ticket_history(ticket_id,at,actor,action) VALUES(1,'x','staff','Earlier action')", []).unwrap();
        }
        let c = open(&path).unwrap();
        assert_eq!(schema_version(&c).unwrap() as usize, MIGRATIONS.len());
        let status = |id: i64| c.query_row("SELECT status FROM tickets WHERE id=?1", [id], |r| r.get::<_, String>(0)).unwrap();
        let mapped: Vec<String> = (1..=7).map(status).collect();
        assert_eq!(mapped, ["In Progress", "In Progress", "In Progress", "In Progress", "In Progress", "Done", "Done"]);
        let nr: i64 = c.query_row("SELECT needs_review FROM tickets WHERE id=2", [], |r| r.get(0)).unwrap();
        assert_eq!(nr, 1, "Needs Review survives as a badge");
        let done_at: String = c.query_row("SELECT completed_at FROM tickets WHERE id=6", [], |r| r.get(0)).unwrap();
        assert_eq!(done_at, "2026-11-06T10:00:00Z");
        let fr1: String = c.query_row("SELECT first_received_at FROM tickets WHERE id=1", [], |r| r.get(0)).unwrap();
        let fr2: String = c.query_row("SELECT first_received_at FROM tickets WHERE id=2", [], |r| r.get(0)).unwrap();
        assert_eq!((fr1.as_str(), fr2.as_str()), ("2026-10-30T12:00:00Z", "2026-11-02T10:00:00Z"));
        let migrated: i64 = c.query_row("SELECT COUNT(*) FROM ticket_history WHERE action='Status migrated to two-status workflow'", [], |r| r.get(0)).unwrap();
        assert_eq!(migrated, 6, "every changed status is recorded (In Progress was unchanged)");
        let old_val: String = c.query_row("SELECT old_value FROM ticket_history WHERE ticket_id=4 AND field='status'", [], |r| r.get(0)).unwrap();
        assert_eq!(old_val, "Waiting for User");
        let earlier: i64 = c.query_row("SELECT COUNT(*) FROM ticket_history WHERE action='Earlier action'", [], |r| r.get(0)).unwrap();
        assert_eq!(earlier, 1);
        let field: (String, String, i64) = c.query_row("SELECT extracted_value, manual_value, verified FROM ticket_fields WHERE ticket_id=1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!(field, ("111".into(), "0012345".into(), 1));
        let (issues, schools): (i64, i64) = c.query_row("SELECT (SELECT COUNT(*) FROM ticket_issues), (SELECT COUNT(*) FROM ticket_schools)", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((issues, schools), (7, 7));
        // search index built for existing tickets
        let hits = crate::tickets::list(&c, &crate::tickets::ListQuery { search: "0012345".into(), ..Default::default() }).unwrap();
        assert_eq!(hits.total, 1);
        let hits = crate::tickets::list(&c, &crate::tickets::ListQuery { search: "original body".into(), ..Default::default() }).unwrap();
        assert_eq!(hits.total, 1);
        // safety backup written before migrating, holding the v1 data
        let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups")).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0].file_name().unwrap().to_string_lossy().starts_with("pre-migration-v1-"));
        let b = Connection::open(&backups[0]).unwrap();
        assert_eq!(schema_version(&b).unwrap(), 1);
        let st: String = b.query_row("SELECT status FROM tickets WHERE id=4", [], |r| r.get(0)).unwrap();
        assert_eq!(st, "Waiting for User");
        drop(c);
        // opening again: nothing to migrate, no new backup, no repeated history
        let c = open(&path).unwrap();
        assert_eq!(std::fs::read_dir(dir.path().join("backups")).unwrap().count(), 1);
        let migrated2: i64 = c.query_row("SELECT COUNT(*) FROM ticket_history WHERE action='Status migrated to two-status workflow'", [], |r| r.get(0)).unwrap();
        assert_eq!(migrated2, 6);
    }
}
