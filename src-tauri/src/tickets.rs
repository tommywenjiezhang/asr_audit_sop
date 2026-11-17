//! Tickets: persistence, field values (extracted vs. staff-entered), audit history,
//! and ingestion of Outlook messages (dedup, conversation linking, duplicate flagging).

use crate::analyzer;
use crate::catalog::catalog;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::render::{put, Ctx};
use crate::settings::Settings;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const STATUSES: &[&str] = &["New", "Needs Review", "In Progress", "Waiting for User", "Waiting for Internal Action", "Resolved", "Closed"];
pub const PRIORITIES: &[&str] = &["Low", "Normal", "High", "Urgent"];
pub const SYSTEM: &str = "system";

pub fn display_id(id: i64) -> String {
    format!("T-{id:06}")
}

#[derive(Debug, Clone, Serialize)]
pub struct TicketSummary {
    pub id: i64,
    pub display_id: String,
    pub status: String,
    pub priority: String,
    pub issue_type: String,
    pub school_name: Option<String>,
    pub school_njiis_id: Option<String>,
    pub school_verification: String,
    pub sender_name: Option<String>,
    pub sender_email: Option<String>,
    pub original_subject: Option<String>,
    pub assigned_staff: Option<String>,
    pub updated_at: String,
    pub last_email_received_at: Option<String>,
    pub next_followup_date: Option<String>,
    pub possible_duplicate_of: Option<i64>,
    pub has_draft: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ticket {
    pub id: i64,
    pub display_id: String,
    pub status: String,
    pub priority: String,
    pub issue_type: String,
    pub issue_type_source: String,
    pub classification_confidence: Option<f64>,
    pub school_njiis_id: Option<String>,
    pub school_name: Option<String>,
    pub school_verification: String,
    pub sender_name: Option<String>,
    pub sender_email: Option<String>,
    pub original_subject: Option<String>,
    pub assigned_staff: Option<String>,
    pub conversation_id: Option<String>,
    pub possible_duplicate_of: Option<i64>,
    pub internal_notes: Option<String>,
    pub resolution_notes: Option<String>,
    pub next_followup_date: Option<String>,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_email_received_at: Option<String>,
    pub last_response_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TicketField {
    pub key: String,
    pub extracted_value: Option<String>,
    pub extraction_confidence: Option<String>,
    pub manual_value: Option<String>,
    pub verified: bool,
    pub effective: String,
    /// "Not Provided" | "Extracted - Not Verified" | "Entered" | "Verified"
    pub state: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageRow {
    pub id: i64,
    pub subject: Option<String>,
    pub sender_name: Option<String>,
    pub sender_email: Option<String>,
    pub received_at: Option<String>,
    pub body_text: Option<String>,
    pub body_truncated: bool,
    pub folder_path: Option<String>,
    pub link_type: String,
    pub is_internal: bool,
    pub has_attachments: bool,
    pub entry_id: Option<String>,
    pub store_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryRow {
    pub id: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub field: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TicketDetail {
    pub ticket: Ticket,
    pub fields: Vec<TicketField>,
    pub messages: Vec<MessageRow>,
    pub history: Vec<HistoryRow>,
}

const TICKET_COLS: &str = "id,status,priority,issue_type,issue_type_source,classification_confidence,school_njiis_id,school_name,school_verification,sender_name,sender_email,original_subject,assigned_staff,conversation_id,possible_duplicate_of,internal_notes,resolution_notes,next_followup_date,source,created_at,updated_at,last_email_received_at,last_response_at";

fn map_ticket(r: &rusqlite::Row) -> rusqlite::Result<Ticket> {
    let id: i64 = r.get(0)?;
    Ok(Ticket {
        id,
        display_id: display_id(id),
        status: r.get(1)?,
        priority: r.get(2)?,
        issue_type: r.get(3)?,
        issue_type_source: r.get(4)?,
        classification_confidence: r.get(5)?,
        school_njiis_id: r.get(6)?,
        school_name: r.get(7)?,
        school_verification: r.get(8)?,
        sender_name: r.get(9)?,
        sender_email: r.get(10)?,
        original_subject: r.get(11)?,
        assigned_staff: r.get(12)?,
        conversation_id: r.get(13)?,
        possible_duplicate_of: r.get(14)?,
        internal_notes: r.get(15)?,
        resolution_notes: r.get(16)?,
        next_followup_date: r.get(17)?,
        source: r.get(18)?,
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
        last_email_received_at: r.get(21)?,
        last_response_at: r.get(22)?,
    })
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Ticket> {
    conn.query_row(&format!("SELECT {TICKET_COLS} FROM tickets WHERE id=?1"), [id], map_ticket)
        .optional()?
        .ok_or_else(|| AppError::coded("NOT_FOUND", format!("Ticket {} not found", display_id(id))))
}

pub fn history(conn: &Connection, ticket_id: i64, actor: &str, action: &str, field: Option<&str>, old: Option<&str>, new: Option<&str>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO ticket_history(ticket_id,at,actor,action,field,old_value,new_value) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![ticket_id, now(), actor, action, field, old, new],
    )?;
    Ok(())
}

fn touch(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("UPDATE tickets SET updated_at=?2 WHERE id=?1", params![id, now()])?;
    Ok(())
}

// ---------------------------------------------------------------- listing

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    /// "all" | "new" | "review" | "progress" | "waiting" | "done" | "open"
    pub group: String,
    pub search: String,
    pub issue_type: String,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize)]
pub struct ListResult {
    pub rows: Vec<TicketSummary>,
    pub total: i64,
    pub counts: Vec<(String, i64)>,
}

fn group_statuses(group: &str) -> Option<Vec<&'static str>> {
    Some(match group {
        "new" => vec!["New"],
        "review" => vec!["Needs Review"],
        "progress" => vec!["In Progress"],
        "waiting" => vec!["Waiting for User", "Waiting for Internal Action"],
        "done" => vec!["Resolved", "Closed"],
        "open" => vec!["New", "Needs Review", "In Progress", "Waiting for User", "Waiting for Internal Action"],
        _ => return None,
    })
}

pub fn list(conn: &Connection, q: &ListQuery) -> AppResult<ListResult> {
    let mut where_: Vec<String> = vec!["1=1".into()];
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(sts) = group_statuses(&q.group) {
        let ph: Vec<String> = sts.iter().map(|s| {
            args.push(Box::new(s.to_string()));
            format!("?{}", args.len())
        }).collect();
        where_.push(format!("t.status IN ({})", ph.join(",")));
    }
    if !q.issue_type.is_empty() {
        args.push(Box::new(q.issue_type.clone()));
        where_.push(format!("t.issue_type = ?{}", args.len()));
    }
    let search = q.search.trim();
    if !search.is_empty() {
        let like = format!("%{}%", search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        args.push(Box::new(like));
        let n = args.len();
        let id_clause = search
            .trim_start_matches(['T', 't', '-'])
            .trim_start_matches('0')
            .parse::<i64>()
            .ok()
            .map(|id| format!(" OR t.id = {id}"))
            .unwrap_or_default();
        where_.push(format!(
            "(t.school_name LIKE ?{n} ESCAPE '\\' OR t.school_njiis_id LIKE ?{n} ESCAPE '\\' OR t.sender_name LIKE ?{n} ESCAPE '\\'
              OR t.sender_email LIKE ?{n} ESCAPE '\\' OR t.original_subject LIKE ?{n} ESCAPE '\\'{id_clause})"
        ));
    }
    let w = where_.join(" AND ");
    let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM tickets t WHERE {w}"), params_ref.as_slice(), |r| r.get(0))?;
    let limit = if q.limit <= 0 { 200 } else { q.limit.min(1000) };
    let sql = format!(
        "SELECT t.id,t.status,t.priority,t.issue_type,t.school_name,t.school_njiis_id,t.school_verification,t.sender_name,t.sender_email,
                t.original_subject,t.assigned_staff,t.updated_at,t.last_email_received_at,t.next_followup_date,t.possible_duplicate_of,
                EXISTS(SELECT 1 FROM email_drafts d WHERE d.ticket_id=t.id)
         FROM tickets t WHERE {w} ORDER BY t.updated_at DESC, t.id DESC LIMIT {limit} OFFSET {}",
        q.offset.max(0)
    );
    let mut st = conn.prepare(&sql)?;
    let rows = st
        .query_map(params_ref.as_slice(), |r| {
            let id: i64 = r.get(0)?;
            Ok(TicketSummary {
                id,
                display_id: display_id(id),
                status: r.get(1)?,
                priority: r.get(2)?,
                issue_type: r.get(3)?,
                school_name: r.get(4)?,
                school_njiis_id: r.get(5)?,
                school_verification: r.get(6)?,
                sender_name: r.get(7)?,
                sender_email: r.get(8)?,
                original_subject: r.get(9)?,
                assigned_staff: r.get(10)?,
                updated_at: r.get(11)?,
                last_email_received_at: r.get(12)?,
                next_followup_date: r.get(13)?,
                possible_duplicate_of: r.get(14)?,
                has_draft: r.get::<_, i64>(15)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut st = conn.prepare("SELECT status, COUNT(*) FROM tickets GROUP BY status")?;
    let counts = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ListResult { rows, total, counts })
}

// ---------------------------------------------------------------- fields

fn field_state(extracted: &Option<String>, manual: &Option<String>, verified: bool) -> (String, String, bool) {
    let (eff, trusted) = match manual {
        Some(m) => (m.clone(), true),
        None => (extracted.clone().unwrap_or_default(), verified),
    };
    let blank = eff.trim().is_empty() || eff.trim() == "[]";
    let state = if blank {
        "Not Provided"
    } else if verified {
        "Verified"
    } else if manual.is_some() {
        "Entered"
    } else {
        "Extracted - Not Verified"
    };
    (eff, state.to_string(), trusted)
}

pub fn fields(conn: &Connection, ticket_id: i64) -> AppResult<Vec<TicketField>> {
    let mut st = conn.prepare(
        "SELECT field_key,extracted_value,extraction_confidence,manual_value,verified,updated_at FROM ticket_fields WHERE ticket_id=?1 ORDER BY field_key",
    )?;
    let v = st
        .query_map([ticket_id], |r| {
            let extracted: Option<String> = r.get(1)?;
            let manual: Option<String> = r.get(3)?;
            let verified = r.get::<_, i64>(4)? != 0;
            let (effective, state, _) = field_state(&extracted, &manual, verified);
            Ok(TicketField {
                key: r.get(0)?,
                extracted_value: extracted,
                extraction_confidence: r.get(2)?,
                manual_value: manual,
                verified,
                effective,
                state,
                updated_at: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

fn effective_field(conn: &Connection, ticket_id: i64, key: &str) -> AppResult<(String, bool)> {
    let row: Option<(Option<String>, Option<String>, i64)> = conn
        .query_row(
            "SELECT extracted_value, manual_value, verified FROM ticket_fields WHERE ticket_id=?1 AND field_key=?2",
            params![ticket_id, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        Some((e, m, v)) => {
            let (eff, _, trusted) = field_state(&e, &m, v != 0);
            (eff, trusted)
        }
        None => (String::new(), false),
    })
}

/// Keep the denormalized school columns on `tickets` in sync with the field values.
fn refresh_school_columns(conn: &Connection, ticket_id: i64) -> AppResult<()> {
    let (id, id_trusted) = effective_field(conn, ticket_id, "school_njiis_id")?;
    let (name, _) = effective_field(conn, ticket_id, "school_name")?;
    let verification = if id.trim().is_empty() {
        "Unverified School"
    } else if id_trusted {
        "Verified"
    } else {
        "Extracted - Not Verified"
    };
    conn.execute(
        "UPDATE tickets SET school_njiis_id=NULLIF(?2,''), school_name=NULLIF(?3,''), school_verification=?4 WHERE id=?1",
        params![ticket_id, id.trim(), name.trim(), verification],
    )?;
    Ok(())
}

/// Store an extracted value. Never touches staff-entered values or verification.
/// With `only_if_empty`, an existing extracted value is kept.
pub fn set_extracted(conn: &Connection, ticket_id: i64, key: &str, value: &str, confidence: &str, only_if_empty: bool) -> AppResult<bool> {
    let existing: Option<Option<String>> = conn
        .query_row(
            "SELECT extracted_value FROM ticket_fields WHERE ticket_id=?1 AND field_key=?2",
            params![ticket_id, key],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        None => {
            conn.execute(
                "INSERT INTO ticket_fields(ticket_id,field_key,extracted_value,extraction_confidence,updated_at) VALUES(?1,?2,?3,?4,?5)",
                params![ticket_id, key, value, confidence, now()],
            )?;
            Ok(true)
        }
        Some(Some(e)) if only_if_empty && !e.trim().is_empty() => Ok(false),
        Some(_) => {
            conn.execute(
                "UPDATE ticket_fields SET extracted_value=?3, extraction_confidence=?4, updated_at=?5 WHERE ticket_id=?1 AND field_key=?2",
                params![ticket_id, key, value, confidence, now()],
            )?;
            Ok(true)
        }
    }
}

/// Staff edit of a field. `value=None` reverts to the extracted value.
pub fn set_field(conn: &Connection, ticket_id: i64, key: &str, value: Option<String>, verified: bool, actor: &str) -> AppResult<TicketField> {
    if key.is_empty() || key.len() > 64 || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(AppError::coded("INVALID", "Invalid field key"));
    }
    if let Some(v) = &value {
        if v.len() > 20_000 {
            return Err(AppError::coded("INVALID", "Value too long"));
        }
    }
    let tx = conn.unchecked_transaction()?;
    get(&tx, ticket_id)?;
    let (old, _) = effective_field(&tx, ticket_id, key)?;
    let value = value.map(|v| v.trim().to_string());
    tx.execute(
        "INSERT INTO ticket_fields(ticket_id,field_key,manual_value,verified,updated_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(ticket_id,field_key) DO UPDATE SET manual_value=excluded.manual_value, verified=excluded.verified, updated_at=excluded.updated_at",
        params![ticket_id, key, value, verified as i64, now()],
    )?;
    let (new, _) = effective_field(&tx, ticket_id, key)?;
    let action = if verified { "Field verified" } else if value.is_none() { "Field reverted to extracted value" } else { "Field updated" };
    if old != new || verified {
        history(&tx, ticket_id, actor, action, Some(key), Some(&old), Some(&new))?;
    }
    if key == "school_njiis_id" || key == "school_name" {
        refresh_school_columns(&tx, ticket_id)?;
    }
    touch(&tx, ticket_id)?;
    tx.commit()?;
    fields(conn, ticket_id)?
        .into_iter()
        .find(|f| f.key == key)
        .ok_or_else(|| AppError::msg("field not saved"))
}

// ---------------------------------------------------------------- ticket edits

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TicketPatch {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub issue_type: Option<String>,
    pub assigned_staff: Option<String>,
    pub internal_notes: Option<String>,
    pub resolution_notes: Option<String>,
    pub next_followup_date: Option<String>,
    pub sender_name: Option<String>,
    pub sender_email: Option<String>,
    pub clear_duplicate_flag: Option<bool>,
}

pub fn update(conn: &Connection, id: i64, p: &TicketPatch, actor: &str) -> AppResult<Ticket> {
    let tx = conn.unchecked_transaction()?;
    let cur = get(&tx, id)?;
    let mut changed = false;
    macro_rules! upd {
        ($field:ident, $col:literal, $validate:expr) => {
            if let Some(v) = &p.$field {
                let v = v.trim().to_string();
                let validate: &dyn Fn(&str) -> bool = &$validate;
                if !validate(&v) {
                    return Err(AppError::coded("INVALID", format!("Invalid value for {}", $col)));
                }
                let old = cur.$field.clone();
                if old.as_deref().unwrap_or("") != v {
                    tx.execute(&format!("UPDATE tickets SET {}=NULLIF(?2,'') WHERE id=?1", $col), params![id, v])?;
                    history(&tx, id, actor, "Ticket updated", Some($col), old.as_deref(), Some(&v))?;
                    changed = true;
                }
            }
        };
    }
    if let Some(s) = &p.status {
        if !STATUSES.contains(&s.as_str()) {
            return Err(AppError::coded("INVALID", "Unknown status"));
        }
        if *s != cur.status {
            tx.execute("UPDATE tickets SET status=?2 WHERE id=?1", params![id, s])?;
            history(&tx, id, actor, "Status changed", Some("status"), Some(&cur.status), Some(s))?;
            changed = true;
        }
    }
    if let Some(s) = &p.priority {
        if !PRIORITIES.contains(&s.as_str()) {
            return Err(AppError::coded("INVALID", "Unknown priority"));
        }
        if *s != cur.priority {
            tx.execute("UPDATE tickets SET priority=?2 WHERE id=?1", params![id, s])?;
            history(&tx, id, actor, "Priority changed", Some("priority"), Some(&cur.priority), Some(s))?;
            changed = true;
        }
    }
    if let Some(it) = &p.issue_type {
        if catalog().issue(it).is_none() {
            return Err(AppError::coded("INVALID", "Unknown issue type"));
        }
        if *it != cur.issue_type || cur.issue_type_source != "manual" {
            tx.execute("UPDATE tickets SET issue_type=?2, issue_type_source='manual' WHERE id=?1", params![id, it])?;
            history(&tx, id, actor, "Issue type set", Some("issue_type"), Some(&cur.issue_type), Some(it))?;
            changed = true;
        }
    }
    upd!(assigned_staff, "assigned_staff", |v: &str| v.len() <= 200);
    upd!(internal_notes, "internal_notes", |v: &str| v.len() <= 50_000);
    upd!(resolution_notes, "resolution_notes", |v: &str| v.len() <= 50_000);
    upd!(next_followup_date, "next_followup_date", |v: &str| v.is_empty() || chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok());
    upd!(sender_name, "sender_name", |v: &str| v.len() <= 300);
    upd!(sender_email, "sender_email", |v: &str| v.is_empty() || (v.contains('@') && v.len() <= 300));
    if p.clear_duplicate_flag == Some(true) && cur.possible_duplicate_of.is_some() {
        tx.execute("UPDATE tickets SET possible_duplicate_of=NULL WHERE id=?1", [id])?;
        history(&tx, id, actor, "Duplicate flag cleared", None, cur.possible_duplicate_of.map(display_id).as_deref(), None)?;
        changed = true;
    }
    if changed {
        touch(&tx, id)?;
    }
    tx.commit()?;
    get(conn, id)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NewTicket {
    pub sender_name: String,
    pub sender_email: String,
    pub subject: String,
    pub body: String,
    pub issue_type: String,
    pub school_njiis_id: String,
    pub school_name: String,
}

pub fn create_manual(conn: &Connection, n: &NewTicket, settings: &Settings, actor: &str) -> AppResult<i64> {
    let cat = catalog();
    let tx = conn.unchecked_transaction()?;
    let ts = now();
    let analysis = analyzer::analyze(cat, &n.subject, &n.body, &n.sender_name, &n.sender_email);
    let (issue, source) = if !n.issue_type.is_empty() && cat.issue(&n.issue_type).is_some() {
        (n.issue_type.clone(), "manual")
    } else {
        (analysis.issue_type.clone(), "auto")
    };
    tx.execute(
        "INSERT INTO tickets(status,priority,issue_type,issue_type_source,classification_confidence,sender_name,sender_email,original_subject,assigned_staff,source,created_at,updated_at)
         VALUES('New','Normal',?1,?2,?3,NULLIF(?4,''),NULLIF(?5,''),NULLIF(?6,''),NULLIF(?7,''),'manual',?8,?8)",
        params![issue, source, analysis.confidence, n.sender_name.trim(), n.sender_email.trim(), n.subject.trim(), settings.default_assigned_staff, ts],
    )?;
    let id = tx.last_insert_rowid();
    for e in &analysis.fields {
        set_extracted(&tx, id, &e.key, &e.value, &e.confidence, false)?;
    }
    history(&tx, id, actor, "Ticket created manually", None, None, Some(&issue))?;
    tx.commit()?;
    if !n.school_njiis_id.trim().is_empty() {
        set_field(conn, id, "school_njiis_id", Some(n.school_njiis_id.clone()), false, actor)?;
    }
    if !n.school_name.trim().is_empty() {
        set_field(conn, id, "school_name", Some(n.school_name.clone()), false, actor)?;
    }
    refresh_school_columns(conn, id)?;
    Ok(id)
}

pub fn messages(conn: &Connection, ticket_id: i64) -> AppResult<Vec<MessageRow>> {
    let mut st = conn.prepare(
        "SELECT m.id,m.subject,m.sender_name,m.sender_email,m.received_at,m.body_text,m.body_truncated,m.folder_path,l.link_type,m.is_internal,m.has_attachments,m.entry_id,m.store_id
         FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id WHERE l.ticket_id=?1 ORDER BY m.received_at DESC, m.id DESC",
    )?;
    let v = st
        .query_map([ticket_id], |r| {
            Ok(MessageRow {
                id: r.get(0)?,
                subject: r.get(1)?,
                sender_name: r.get(2)?,
                sender_email: r.get(3)?,
                received_at: r.get(4)?,
                body_text: r.get(5)?,
                body_truncated: r.get::<_, i64>(6)? != 0,
                folder_path: r.get(7)?,
                link_type: r.get(8)?,
                is_internal: r.get::<_, i64>(9)? != 0,
                has_attachments: r.get::<_, i64>(10)? != 0,
                entry_id: r.get(11)?,
                store_id: r.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn history_rows(conn: &Connection, ticket_id: i64) -> AppResult<Vec<HistoryRow>> {
    let mut st = conn.prepare("SELECT id,at,actor,action,field,old_value,new_value FROM ticket_history WHERE ticket_id=?1 ORDER BY id DESC LIMIT 500")?;
    let v = st
        .query_map([ticket_id], |r| {
            Ok(HistoryRow { id: r.get(0)?, at: r.get(1)?, actor: r.get(2)?, action: r.get(3)?, field: r.get(4)?, old_value: r.get(5)?, new_value: r.get(6)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn detail(conn: &Connection, id: i64) -> AppResult<TicketDetail> {
    Ok(TicketDetail { ticket: get(conn, id)?, fields: fields(conn, id)?, messages: messages(conn, id)?, history: history_rows(conn, id)? })
}

/// Placeholder context for a ticket.
pub fn build_ctx(conn: &Connection, id: i64, settings: &Settings) -> AppResult<Ctx> {
    let t = get(conn, id)?;
    let mut ctx = Ctx::new();
    for f in fields(conn, id)? {
        let trusted = f.manual_value.is_some() || f.verified;
        put(&mut ctx, &f.key, f.effective.clone(), trusted);
    }
    put(&mut ctx, "ticket_id", t.display_id.clone(), true);
    put(&mut ctx, "sender_name", t.sender_name.clone().unwrap_or_default(), true);
    put(&mut ctx, "sender_email", t.sender_email.clone().unwrap_or_default(), true);
    put(&mut ctx, "original_subject", t.original_subject.clone().unwrap_or_default(), true);
    put(&mut ctx, "staff_name", settings.staff_name.clone(), true);
    put(&mut ctx, "asr_deadline", settings.asr_deadline.clone(), true);
    Ok(ctx)
}

// ---------------------------------------------------------------- ingestion

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct IncomingMessage {
    pub store_id: String,
    pub entry_id: String,
    pub internet_message_id: String,
    pub conversation_id: String,
    pub folder_entry_id: String,
    pub folder_path: String,
    pub subject: String,
    pub sender_name: String,
    pub sender_email: String,
    /// RFC3339 UTC
    pub received_at: String,
    pub body: String,
    pub body_truncated: bool,
    pub has_attachments: bool,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IngestReport {
    pub seen: usize,
    pub new_messages: usize,
    pub already_imported: usize,
    pub moved: usize,
    pub created_tickets: usize,
    pub updated_tickets: usize,
    pub skipped_internal: usize,
    pub duplicates_flagged: usize,
    pub max_received_at: Option<String>,
}

fn find_message(conn: &Connection, m: &IncomingMessage) -> AppResult<Option<(i64, Option<String>, Option<String>)>> {
    if !m.internet_message_id.is_empty() {
        if let Some(r) = conn
            .query_row(
                "SELECT id, entry_id, folder_path FROM email_messages WHERE internet_message_id=?1",
                [&m.internet_message_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
        {
            return Ok(Some(r));
        }
    }
    if !m.entry_id.is_empty() {
        return Ok(conn
            .query_row(
                "SELECT id, entry_id, folder_path FROM email_messages WHERE entry_id=?1 AND COALESCE(store_id,'')=?2",
                params![m.entry_id, m.store_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?);
    }
    Ok(None)
}

pub fn ingest(conn: &Connection, msgs: &[IncomingMessage], settings: &Settings) -> AppResult<IngestReport> {
    let cat = catalog();
    let mut rep = IngestReport::default();
    let tx = conn.unchecked_transaction()?;
    for m in msgs {
        rep.seen += 1;
        if rep.max_received_at.as_deref().map(|x| m.received_at.as_str() > x).unwrap_or(true) && !m.received_at.is_empty() {
            rep.max_received_at = Some(m.received_at.clone());
        }
        // 1. dedup by Internet Message ID / EntryID; detect folder moves
        if let Some((mid, old_entry, old_path)) = find_message(&tx, m)? {
            rep.already_imported += 1;
            let moved = old_entry.as_deref() != Some(m.entry_id.as_str()) || old_path.as_deref() != Some(m.folder_path.as_str());
            if moved && !m.entry_id.is_empty() {
                tx.execute(
                    "UPDATE email_messages SET entry_id=?2, store_id=?3, folder_entry_id=?4, folder_path=?5 WHERE id=?1",
                    params![mid, m.entry_id, m.store_id, m.folder_entry_id, m.folder_path],
                )?;
                let mut st = tx.prepare("SELECT ticket_id FROM ticket_email_links WHERE message_id=?1")?;
                let tids: Vec<i64> = st.query_map([mid], |r| r.get(0))?.collect::<Result<_, _>>()?;
                for tid in tids {
                    history(&tx, tid, SYSTEM, "Email moved in Outlook", Some("folder"), old_path.as_deref(), Some(&m.folder_path))?;
                }
                rep.moved += 1;
            }
            continue;
        }
        let internal = settings.is_internal_sender(&m.sender_email);
        let preview_meta = serde_json::to_string(&m.metadata).unwrap_or_else(|_| "{}".into());
        tx.execute(
            "INSERT INTO email_messages(store_id,entry_id,internet_message_id,conversation_id,folder_entry_id,folder_path,subject,sender_name,sender_email,received_at,body_text,body_truncated,has_attachments,is_internal,metadata_json,imported_at)
             VALUES(?1,?2,NULLIF(?3,''),NULLIF(?4,''),?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![m.store_id, m.entry_id, m.internet_message_id, m.conversation_id, m.folder_entry_id, m.folder_path, m.subject,
                m.sender_name, m.sender_email.to_lowercase(), m.received_at, m.body, m.body_truncated as i64, m.has_attachments as i64,
                internal as i64, preview_meta, now()],
        )?;
        let mid = tx.last_insert_rowid();
        rep.new_messages += 1;

        // 2. existing ticket for this conversation?
        let existing: Option<(i64, String)> = if m.conversation_id.is_empty() {
            None
        } else {
            tx.query_row(
                "SELECT id, status FROM tickets WHERE conversation_id=?1 ORDER BY id DESC LIMIT 1",
                [&m.conversation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };

        if internal {
            if let Some((tid, _)) = existing {
                tx.execute("INSERT OR IGNORE INTO ticket_email_links(ticket_id,message_id,link_type,linked_at) VALUES(?1,?2,'internal',?3)", params![tid, mid, now()])?;
                history(&tx, tid, SYSTEM, "Staff email linked", None, None, Some(&m.subject))?;
            }
            rep.skipped_internal += 1;
            continue;
        }

        let analysis = analyzer::analyze(cat, &m.subject, &m.body, &m.sender_name, &m.sender_email);

        if let Some((tid, status)) = existing {
            tx.execute("INSERT OR IGNORE INTO ticket_email_links(ticket_id,message_id,link_type,linked_at) VALUES(?1,?2,'reply',?3)", params![tid, mid, now()])?;
            tx.execute(
                "UPDATE tickets SET last_email_received_at=MAX(COALESCE(last_email_received_at,''),?2), updated_at=?3 WHERE id=?1",
                params![tid, m.received_at, now()],
            )?;
            history(&tx, tid, SYSTEM, "New email in conversation", None, None, Some(&m.subject))?;
            // only fill values that were never extracted; staff-entered values are untouched
            for e in &analysis.fields {
                if e.key != "issue_description" {
                    set_extracted(&tx, tid, &e.key, &e.value, &e.confidence, true)?;
                }
            }
            refresh_school_columns(&tx, tid)?;
            if ["Waiting for User", "Resolved", "Closed"].contains(&status.as_str()) {
                tx.execute("UPDATE tickets SET status='Needs Review' WHERE id=?1", [tid])?;
                history(&tx, tid, SYSTEM, "Status changed (new email received)", Some("status"), Some(&status), Some("Needs Review"))?;
            }
            rep.updated_tickets += 1;
            continue;
        }

        // 3. new ticket
        let ts = now();
        tx.execute(
            "INSERT INTO tickets(status,priority,issue_type,issue_type_source,classification_confidence,sender_name,sender_email,original_subject,assigned_staff,conversation_id,source,created_at,updated_at,last_email_received_at)
             VALUES('New','Normal',?1,'auto',?2,NULLIF(?3,''),NULLIF(?4,''),NULLIF(?5,''),NULLIF(?6,''),NULLIF(?7,''),'outlook',?8,?8,?9)",
            params![analysis.issue_type, analysis.confidence, m.sender_name, m.sender_email.to_lowercase(), m.subject,
                settings.default_assigned_staff, m.conversation_id, ts, m.received_at],
        )?;
        let tid = tx.last_insert_rowid();
        tx.execute("INSERT INTO ticket_email_links(ticket_id,message_id,link_type,linked_at) VALUES(?1,?2,'original',?3)", params![tid, mid, ts])?;
        for e in &analysis.fields {
            set_extracted(&tx, tid, &e.key, &e.value, &e.confidence, false)?;
        }
        refresh_school_columns(&tx, tid)?;
        history(&tx, tid, SYSTEM, "Ticket created from Outlook email", Some("issue_type"), None, Some(&analysis.issue_type))?;

        // 4. probable duplicate (flag only - never merge)
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(settings.duplicate_window_days as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let school_id = analysis.get("school_njiis_id").unwrap_or("").to_string();
        let dup: Option<i64> = tx
            .query_row(
                "SELECT id FROM tickets WHERE id<>?1 AND status<>'Closed' AND created_at>=?2 AND issue_type=?3
                   AND (COALESCE(conversation_id,'') <> ?4 OR ?4 = '')
                   AND ((sender_email IS NOT NULL AND sender_email=?5) OR (?6<>'' AND school_njiis_id=?6))
                 ORDER BY id DESC LIMIT 1",
                params![tid, cutoff, analysis.issue_type, m.conversation_id, m.sender_email.to_lowercase(), school_id],
                |r| r.get(0),
            )
            .optional()?;
        let needs_review = dup.is_some() || analysis.issue_type == "other" || analysis.confidence < 0.35;
        if let Some(d) = dup {
            tx.execute("UPDATE tickets SET possible_duplicate_of=?2 WHERE id=?1", params![tid, d])?;
            history(&tx, tid, SYSTEM, "Possible duplicate flagged for review", None, None, Some(&display_id(d)))?;
            rep.duplicates_flagged += 1;
        }
        if needs_review {
            tx.execute("UPDATE tickets SET status='Needs Review' WHERE id=?1", [tid])?;
        }
        rep.created_tickets += 1;
    }
    tx.commit()?;
    Ok(rep)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::db::open_in_memory;

    pub fn conn() -> Connection {
        let c = open_in_memory().unwrap();
        crate::templates_store::seed_embedded(&c).unwrap();
        c
    }

    pub fn msg(n: u32, conv: &str, subject: &str, body: &str, sender: &str) -> IncomingMessage {
        IncomingMessage {
            store_id: "S1".into(),
            entry_id: format!("E{n}"),
            internet_message_id: format!("<m{n}@x>"),
            conversation_id: conv.into(),
            folder_entry_id: "F1".into(),
            folder_path: "Inbox/Wenjie".into(),
            subject: subject.into(),
            sender_name: "Doe, Jane".into(),
            sender_email: sender.into(),
            received_at: format!("2026-11-16T10:{:02}:00Z", n % 60),
            body: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn ingest_creates_ticket_and_dedups_repeated_sync() {
        let c = conn();
        let s = Settings::default();
        let m = msg(1, "C1", "School Subtype Error", "Our school ID is 123456. It says the school is missing the subtype.", "jane@school.org");
        let r1 = ingest(&c, &[m.clone()], &s).unwrap();
        assert_eq!(r1.created_tickets, 1);
        let r2 = ingest(&c, &[m.clone(), m.clone()], &s).unwrap();
        assert_eq!(r2.created_tickets, 0);
        assert_eq!(r2.already_imported, 2);
        let count: i64 = c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
        let t = get(&c, 1).unwrap();
        assert_eq!(t.issue_type, "school_subtype_missing");
        assert_eq!(t.school_njiis_id.as_deref(), Some("123456"));
        assert_eq!(t.school_verification, "Extracted - Not Verified");
    }

    #[test]
    fn moved_email_updates_location_and_keeps_history() {
        let c = conn();
        let s = Settings::default();
        let m = msg(1, "C1", "Help", "Need the user guide please", "jane@school.org");
        ingest(&c, &[m.clone()], &s).unwrap();
        let mut moved = m.clone();
        moved.entry_id = "E1-moved".into();
        moved.folder_path = "Inbox/Wenjie/School Subtype Needed".into();
        let r = ingest(&c, &[moved], &s).unwrap();
        assert_eq!(r.moved, 1);
        assert_eq!(r.created_tickets, 0);
        let h = history_rows(&c, 1).unwrap();
        assert!(h.iter().any(|x| x.action == "Email moved in Outlook"));
    }

    #[test]
    fn reply_links_to_conversation_and_preserves_manual_values() {
        let c = conn();
        let s = Settings::default();
        ingest(&c, &[msg(1, "C1", "Subtype", "Our school is missing the subtype.", "jane@school.org")], &s).unwrap();
        set_field(&c, 1, "school_njiis_id", Some("777777".into()), true, "staff").unwrap();
        set_field(&c, 1, "school_type", Some("Public".into()), false, "staff").unwrap();
        update(&c, 1, &TicketPatch { status: Some("Waiting for User".into()), ..Default::default() }, "staff").unwrap();
        let r = ingest(&c, &[msg(2, "C1", "RE: Subtype", "We are a charter school. School ID: 123456", "jane@school.org")], &s).unwrap();
        assert_eq!(r.updated_tickets, 1);
        assert_eq!(r.created_tickets, 0);
        let t = get(&c, 1).unwrap();
        assert_eq!(t.status, "Needs Review");
        assert_eq!(t.school_njiis_id.as_deref(), Some("777777"), "verified value must not be overwritten");
        assert_eq!(t.school_verification, "Verified");
        let f = fields(&c, 1).unwrap();
        let st = f.iter().find(|x| x.key == "school_type").unwrap();
        assert_eq!(st.effective, "Public");
        assert_eq!(st.extracted_value.as_deref(), Some("Charter"));
        assert_eq!(messages(&c, 1).unwrap().len(), 2);
    }

    #[test]
    fn duplicates_flagged_not_merged_and_internal_ignored() {
        let c = conn();
        let mut s = Settings::default();
        s.internal_senders = vec!["@doh.nj.gov".into()];
        ingest(&c, &[msg(1, "C1", "Black screen", "The webinar shows a black screen.", "jane@school.org")], &s).unwrap();
        let r = ingest(&c, &[msg(2, "C2", "Webinar black screen again", "Still a black screen on the webinar.", "jane@school.org")], &s).unwrap();
        assert_eq!(r.duplicates_flagged, 1);
        let t2 = get(&c, 2).unwrap();
        assert_eq!(t2.possible_duplicate_of, Some(1));
        assert_eq!(t2.status, "Needs Review");
        let r = ingest(&c, &[msg(3, "C9", "Internal", "note", "staff@doh.nj.gov")], &s).unwrap();
        assert_eq!(r.skipped_internal, 1);
        assert_eq!(r.created_tickets, 0);
    }

    #[test]
    fn unverified_school_when_no_id() {
        let c = conn();
        ingest(&c, &[msg(1, "C1", "Question", "How do I fill out the form? user guide?", "a@b.org")], &Settings::default()).unwrap();
        let t = get(&c, 1).unwrap();
        assert_eq!(t.school_verification, "Unverified School");
    }

    #[test]
    fn manual_ticket_list_search_and_update_history() {
        let c = conn();
        let s = Settings::default();
        let id = create_manual(&c, &NewTicket { sender_name: "Ann".into(), sender_email: "ann@x.org".into(), subject: "Phone call".into(),
            body: "".into(), issue_type: "user_guide".into(), school_name: "Maple School".into(), ..Default::default() }, &s, "staff").unwrap();
        let l = list(&c, &ListQuery { group: "open".into(), search: "maple".into(), ..Default::default() }).unwrap();
        assert_eq!(l.total, 1);
        let l = list(&c, &ListQuery { search: format!("T-{id:06}"), ..Default::default() }).unwrap();
        assert_eq!(l.total, 1);
        assert!(update(&c, id, &TicketPatch { status: Some("Bogus".into()), ..Default::default() }, "staff").is_err());
        update(&c, id, &TicketPatch { status: Some("In Progress".into()), next_followup_date: Some("2026-11-20".into()), ..Default::default() }, "staff").unwrap();
        let h = history_rows(&c, id).unwrap();
        assert!(h.iter().any(|x| x.action == "Status changed"));
        // revert field to extracted
        set_field(&c, id, "school_name", None, false, "staff").unwrap();
        assert_eq!(get(&c, id).unwrap().school_name, None);
    }
}
