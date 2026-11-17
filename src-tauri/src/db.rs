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
];

pub fn open(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> AppResult<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
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
}
