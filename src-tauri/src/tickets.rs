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

pub const UNASSIGNED: &str = "Unassigned";
pub const ASSIGNED: &str = "Assigned";
pub const IN_PROGRESS: &str = "In Progress";
pub const DONE: &str = "Done";
pub const REVIEW: &str = "Review";
/// Workflow statuses (stored separately from the owner, from current/history placement and from the
/// shared-database sync state). Assigned = owner accepted (Claim / assignment), In Progress = Start Work,
/// Review = a Done case got a new substantive reply or staff flagged it, Done = staff clicked Mark Done
/// (no sent-reply verification). See `workflow` and `shared_workflow`.
pub const STATUSES: &[&str] = &[UNASSIGNED, ASSIGNED, IN_PROGRESS, REVIEW, DONE];
pub const PRIORITIES: &[&str] = &["Low", "Normal", "High", "Urgent"];
pub const SYSTEM: &str = "system";
/// SQL expression (alias `t`) for the "low confidence classification" badge.
const LOW_CONF_SQL: &str = "(t.issue_type_source='auto' AND (t.issue_type='other' OR COALESCE(t.classification_confidence,0) < 0.35))";

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
    pub first_received_at: Option<String>,
    pub completed_at: Option<String>,
    pub needs_review: bool,
    pub low_confidence: bool,
    pub issue_count: i64,
    pub assignee_staff_id: Option<i64>,
    pub assignee_name: Option<String>,
    pub assigned_at: Option<String>,
    pub ownership_conflict: bool,
    pub scope_state: String,
    pub scope_reason: Option<String>,
    pub ticket_ref: Option<String>,
    /// incoming emails of this ticket currently confirmed in the selected scope
    pub in_scope_count: i64,
    /// shared-database state: "local" | "pending" | "synced" | "conflict" | "failed"
    pub sync_state: String,
    pub review_reason: Option<String>,
    /// an incoming email arrived after the last staff action on an open case
    pub new_reply: bool,
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
    pub needs_review: bool,
    pub completed_at: Option<String>,
    pub first_received_at: Option<String>,
    pub staff_actions: Option<String>,
    pub last_prompt_at: Option<String>,
    pub low_confidence: bool,
    pub assignee_staff_id: Option<i64>,
    pub assigned_at: Option<String>,
    pub ownership_conflict: bool,
    /// Stable reference shared by all computers that import the same email (local T-numbers are not).
    pub ticket_ref: Option<String>,
    /// "current" | "history" | "unverified"
    pub scope_state: String,
    pub scope_reason: Option<String>,
    pub scope_changed_at: Option<String>,
    pub assignee_name: Option<String>,
    pub review_reason: Option<String>,
    pub shared_version: i64,
    /// "local_only" | "queued" | "shared"
    pub shared_state: String,
    pub shared_refreshed_at: Option<String>,
    pub sync_state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueRow {
    pub issue_type: String,
    pub source: String,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionRow {
    pub message_id: i64,
    pub flagged_at: Option<String>,
    pub moved_at: Option<String>,
    pub dest_path: Option<String>,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub attempts: i64,
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
    /// "in" | "out" | "unknown" - membership of the selected scope
    pub scope_state: String,
    pub out_reason: Option<String>,
    /// "found" | "not_found" | "error" | null (not checked yet)
    pub location_status: Option<String>,
    pub location_checked_at: Option<String>,
    pub owner_name: Option<String>,
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
    pub issues: Vec<IssueRow>,
    /// Active school group slots (1, 2, ...); values are fields keyed by `analyzer::school_key`.
    pub schools: Vec<i64>,
    pub completion: Vec<CompletionRow>,
    pub assignments: Vec<crate::assignment::AssignmentOp>,
    pub notes: Vec<crate::notes::Note>,
    pub ownership: Vec<OwnershipRow>,
    pub completions: Vec<CompletionDecision>,
    pub conflicts: Vec<crate::sync_queue::LocalConflict>,
    pub pending_ops: Vec<crate::sync_queue::OpRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OwnershipRow {
    pub staff_name: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub started_by: Option<String>,
    pub start_reason: Option<String>,
    pub end_reason: Option<String>,
    pub origin: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionDecision {
    pub completion_id: String,
    pub actor_name: Option<String>,
    pub decided_at: String,
    pub covered: i64,
    pub state: String,
    pub state_reason: Option<String>,
}

const TICKET_COLS: &str = "id,status,priority,issue_type,issue_type_source,classification_confidence,school_njiis_id,school_name,school_verification,sender_name,sender_email,original_subject,assigned_staff,conversation_id,possible_duplicate_of,internal_notes,resolution_notes,next_followup_date,source,created_at,updated_at,last_email_received_at,last_response_at,needs_review,completed_at,first_received_at,staff_actions,last_prompt_at,assignee_staff_id,assigned_at,ownership_conflict,ticket_ref,scope_state,scope_reason,scope_changed_at,(SELECT display_name FROM staff s WHERE s.id=tickets.assignee_staff_id),review_reason,shared_version,shared_state,shared_refreshed_at";

fn map_ticket(r: &rusqlite::Row) -> rusqlite::Result<Ticket> {
    let id: i64 = r.get(0)?;
    let issue_type: String = r.get(3)?;
    let source: String = r.get(4)?;
    let conf: Option<f64> = r.get(5)?;
    Ok(Ticket {
        low_confidence: source == "auto" && (issue_type == "other" || conf.unwrap_or(0.0) < 0.35),
        needs_review: r.get::<_, i64>(23)? != 0,
        completed_at: r.get(24)?,
        first_received_at: r.get(25)?,
        staff_actions: r.get(26)?,
        last_prompt_at: r.get(27)?,
        assignee_staff_id: r.get(28)?,
        assigned_at: r.get(29)?,
        ownership_conflict: r.get::<_, i64>(30)? != 0,
        ticket_ref: r.get(31)?,
        scope_state: r.get(32)?,
        scope_reason: r.get(33)?,
        scope_changed_at: r.get(34)?,
        assignee_name: r.get(35)?,
        review_reason: r.get(36)?,
        shared_version: r.get(37)?,
        shared_state: r.get(38)?,
        shared_refreshed_at: r.get(39)?,
        sync_state: String::new(),
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
    let mut t = conn
        .query_row(&format!("SELECT {TICKET_COLS} FROM tickets WHERE id=?1"), [id], map_ticket)
        .optional()?
        .ok_or_else(|| AppError::coded("NOT_FOUND", format!("Ticket {} not found", display_id(id))))?;
    t.sync_state = crate::sync_queue::ticket_sync_state(conn, id)?;
    Ok(t)
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
    /// "current" (Current Tickets) | "history" (Ticket History) | "unverified" (Awaiting location check) |
    /// "team" / "" (all cases, any folder)
    pub view: String,
    /// "" | "pending" | "conflict" | "failed" | "attention" (pending, conflict or failed)
    pub sync: String,
    /// "" (all) | "Unassigned" | "Assigned" | "In Progress" | "Done" | "open" (anything but Done)
    pub status: String,
    /// "" (anyone) | "none" (no assignee) | staff id
    pub assignee: String,
    /// History filter: reason the ticket left Current Tickets ("completed", "reassigned", "moved", "not_found", "unknown")
    pub reason: String,
    /// Plain-text filters on school (name / NJIIS ID / other IDs) and sender (name / email)
    pub school: String,
    pub sender: String,
    /// Only tickets received at least this many days ago (ticket age); 0 = off
    pub min_age_days: i64,
    pub assigned_from: String,
    pub assigned_to: String,
    /// Free text (FTS5 over ticket, school, sender, subject, bodies, notes) or a ticket ID.
    pub search: String,
    /// Matches any of the ticket's issue types.
    pub issue_type: String,
    /// UTC RFC3339 bounds, `from` inclusive and `to` exclusive. The UI sends local midnight of the
    /// start date and local midnight of the day AFTER the end date, so end dates are inclusive.
    pub received_from: String,
    pub received_to: String,
    pub completed_from: String,
    pub completed_to: String,
    /// "received" (default) | "updated" | "completed"
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize)]
pub struct ListResult {
    pub rows: Vec<TicketSummary>,
    pub total: i64,
    pub counts: Vec<(String, i64)>,
}

fn parse_ticket_id(s: &str) -> Option<i64> {
    let t = s.trim();
    let digits = t.strip_prefix(['T', 't']).map(|r| r.trim_start_matches('-')).unwrap_or(t);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) || digits.len() > 12 {
        return None;
    }
    digits.parse().ok()
}

pub fn list(conn: &Connection, q: &ListQuery) -> AppResult<ListResult> {
    // duplicates combined into another local ticket stay stored but are never listed
    let mut where_: Vec<String> = vec!["t.merged_into IS NULL".into()];
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let arg = |v: String, args: &mut Vec<Box<dyn rusqlite::ToSql>>| {
        args.push(Box::new(v));
        format!("?{}", args.len())
    };
    match q.view.as_str() {
        "" | "all" | "team" => {}
        v @ ("current" | "history" | "unverified") => where_.push(format!("t.scope_state = '{v}'")),
        _ => return Err(AppError::coded("INVALID", "Unknown view")),
    }
    if q.status == "open" {
        where_.push(format!("t.status <> '{DONE}'"));
    } else if !q.status.is_empty() {
        if !STATUSES.contains(&q.status.as_str()) {
            return Err(AppError::coded("INVALID", "Unknown status filter"));
        }
        let p = arg(q.status.clone(), &mut args);
        where_.push(format!("t.status = {p}"));
    }
    match q.assignee.as_str() {
        "" => {}
        "none" => where_.push("t.assignee_staff_id IS NULL".into()),
        id => {
            let id: i64 = id.parse().map_err(|_| AppError::coded("INVALID", "Unknown assignee filter"))?;
            where_.push(format!("t.assignee_staff_id = {id}"));
        }
    }
    match q.sync.as_str() {
        "" => {}
        v @ ("pending" | "conflict" | "failed") => where_.push(format!("({}) = '{v}'", crate::sync_queue::TICKET_SYNC_SQL)),
        "attention" => where_.push(format!("({}) IN ('pending','conflict','failed')", crate::sync_queue::TICKET_SYNC_SQL)),
        _ => return Err(AppError::coded("INVALID", "Unknown sync filter")),
    }
    if !q.reason.is_empty() {
        let p = arg(q.reason.clone(), &mut args);
        where_.push(format!("t.scope_reason = {p}"));
    }
    let like = |s: &str| format!("%{}%", s.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    if !q.school.trim().is_empty() {
        let p = arg(like(&q.school), &mut args);
        where_.push(format!(
            "(t.school_name LIKE {p} ESCAPE '\\' OR t.school_njiis_id LIKE {p} ESCAPE '\\' OR EXISTS(SELECT 1 FROM ticket_fields f WHERE f.ticket_id=t.id
               AND (f.field_key LIKE 'school%' OR f.field_key='facility_id') AND COALESCE(f.manual_value,f.extracted_value) LIKE {p} ESCAPE '\\'))"
        ));
    }
    if !q.sender.trim().is_empty() {
        let p = arg(like(&q.sender), &mut args);
        where_.push(format!("(t.sender_name LIKE {p} ESCAPE '\\' OR t.sender_email LIKE {p} ESCAPE '\\')"));
    }
    if q.min_age_days > 0 {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(q.min_age_days)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let p = arg(cutoff, &mut args);
        where_.push(format!("COALESCE(t.first_received_at, t.created_at) <= {p}"));
    }
    if !q.assigned_from.is_empty() {
        let p = arg(normalize_instant(&q.assigned_from)?, &mut args);
        where_.push(format!("t.assigned_at >= {p}"));
    }
    if !q.assigned_to.is_empty() {
        let p = arg(normalize_instant(&q.assigned_to)?, &mut args);
        where_.push(format!("t.assigned_at < {p}"));
    }
    if !q.issue_type.is_empty() {
        let p = arg(q.issue_type.clone(), &mut args);
        where_.push(format!("(t.issue_type = {p} OR EXISTS(SELECT 1 FROM ticket_issues i WHERE i.ticket_id=t.id AND i.issue_type={p}))"));
    }
    for (col, from, to) in [("COALESCE(t.first_received_at, t.created_at)", &q.received_from, &q.received_to), ("t.completed_at", &q.completed_from, &q.completed_to)] {
        if !from.is_empty() {
            let p = arg(normalize_instant(from)?, &mut args);
            where_.push(format!("{col} >= {p}"));
        }
        if !to.is_empty() {
            let p = arg(normalize_instant(to)?, &mut args);
            where_.push(format!("{col} < {p}"));
        }
    }
    let search = q.search.trim();
    if !search.is_empty() {
        let id_clause = parse_ticket_id(search).map(|id| format!("t.id = {id}"));
        let fts_clause = crate::search::match_expr(search).map(|m| {
            let p = arg(m, &mut args);
            format!("t.id IN (SELECT rowid FROM ticket_search WHERE ticket_search MATCH {p})")
        });
        let parts: Vec<String> = [id_clause, fts_clause].into_iter().flatten().collect();
        if parts.is_empty() {
            where_.push("0".into());
        } else {
            where_.push(format!("({})", parts.join(" OR ")));
        }
    }
    let w = where_.join(" AND ");
    let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM tickets t WHERE {w}"), params_ref.as_slice(), |r| r.get(0))?;
    let limit = if q.limit <= 0 { 200 } else { q.limit.min(1000) };
    let order = match q.sort.as_str() {
        "updated" => "t.updated_at DESC, t.id DESC",
        "completed" => "t.completed_at DESC NULLS LAST, t.id DESC",
        "assigned" => "t.assigned_at DESC NULLS LAST, t.id DESC",
        _ => "COALESCE(t.first_received_at, t.created_at) DESC, t.id DESC",
    };
    let sql = format!(
        "SELECT t.id,t.status,t.priority,t.issue_type,t.school_name,t.school_njiis_id,t.school_verification,t.sender_name,t.sender_email,
                t.original_subject,t.assigned_staff,t.updated_at,t.last_email_received_at,t.next_followup_date,t.possible_duplicate_of,
                COALESCE(t.first_received_at, t.created_at), t.completed_at, t.needs_review, {LOW_CONF_SQL},
                (SELECT COUNT(*) FROM ticket_issues i WHERE i.ticket_id=t.id),
                t.assignee_staff_id, (SELECT display_name FROM staff s WHERE s.id=t.assignee_staff_id), t.assigned_at, t.ownership_conflict,
                t.scope_state, t.scope_reason, t.ticket_ref,
                (SELECT COUNT(*) FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
                  WHERE l.ticket_id=t.id AND l.link_type IN ('original','reply') AND m.is_internal=0 AND m.scope_state='in'),
                ({sync}), t.review_reason,
                (t.status <> 'Done' AND COALESCE(t.last_email_received_at,'') > COALESCE((SELECT MAX(h.at) FROM ticket_history h WHERE h.ticket_id=t.id
                    AND h.actor NOT IN ('system','sync') AND h.action NOT LIKE 'Copilot%' AND h.action NOT LIKE 'Opened%'), t.created_at))
         FROM tickets t WHERE {w} ORDER BY {order} LIMIT {limit} OFFSET {}",
        q.offset.max(0),
        sync = crate::sync_queue::TICKET_SYNC_SQL
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
                first_received_at: r.get(15)?,
                completed_at: r.get(16)?,
                needs_review: r.get::<_, i64>(17)? != 0,
                low_confidence: r.get::<_, i64>(18)? != 0,
                issue_count: r.get(19)?,
                assignee_staff_id: r.get(20)?,
                assignee_name: r.get(21)?,
                assigned_at: r.get(22)?,
                ownership_conflict: r.get::<_, i64>(23)? != 0,
                scope_state: r.get(24)?,
                scope_reason: r.get(25)?,
                ticket_ref: r.get(26)?,
                in_scope_count: r.get(27)?,
                sync_state: r.get(28)?,
                review_reason: r.get(29)?,
                new_reply: r.get::<_, i64>(30)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut st = conn.prepare("SELECT scope_state, COUNT(*) FROM tickets WHERE merged_into IS NULL GROUP BY scope_state")?;
    let counts = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ListResult { rows, total, counts })
}

/// Accept RFC3339 instants (any offset) and store/compare them as UTC "YYYY-MM-DDTHH:MM:SSZ".
fn normalize_instant(s: &str) -> AppResult<String> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .map(|d| d.with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .map_err(|_| AppError::coded("INVALID", format!("Invalid date filter: {s}")))
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

pub fn refresh_school_columns_pub(conn: &Connection, ticket_id: i64) -> AppResult<()> {
    refresh_school_columns(conn, ticket_id)
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
    let before: Option<(Option<String>, i64)> = tx
        .query_row("SELECT manual_value, verified FROM ticket_fields WHERE ticket_id=?1 AND field_key=?2", params![ticket_id, key], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let mark = crate::sync_queue::last_history_row(&tx)?;
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
    let (b_manual, b_ver) = before.map(|(m, v)| (m, v != 0)).unwrap_or((None, false));
    if b_manual != value || b_ver != verified {
        let extracted: Option<String> = tx.query_row("SELECT extracted_value FROM ticket_fields WHERE ticket_id=?1 AND field_key=?2", params![ticket_id, key], |r| r.get(0))?;
        let op = crate::sync_queue::enqueue(&tx, actor, crate::sync_queue::NewOp {
            ticket_id: Some(ticket_id),
            entity_type: "field",
            entity_id: Some(key.to_string()),
            op_type: "field.set",
            original: serde_json::json!({"manual": b_manual, "has_manual": b_manual.is_some(), "verified": b_ver}),
            proposed: serde_json::json!({"manual": value, "has_manual": value.is_some(), "verified": verified, "extracted": extracted}),
        })?;
        crate::sync_queue::tag_history(&tx, ticket_id, &op, mark)?;
    }
    touch(&tx, ticket_id)?;
    crate::search::reindex(&tx, ticket_id)?;
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
    pub staff_actions: Option<String>,
    pub clear_duplicate_flag: Option<bool>,
    pub clear_needs_review: Option<bool>,
}

pub fn update(conn: &Connection, id: i64, p: &TicketPatch, actor: &str) -> AppResult<Ticket> {
    if let Some(s) = p.status.as_deref() {
        if !STATUSES.contains(&s) {
            return Err(AppError::coded("INVALID", "Unknown status"));
        }
        let cur = get(conn, id)?;
        if s != cur.status {
            match (s, cur.status.as_str()) {
                (DONE, _) => return Err(AppError::coded("INVALID", "Use Mark Done to complete a ticket.")),
                (ASSIGNED | UNASSIGNED, _) => return Err(AppError::coded("INVALID", "Use Claim / Take over (or Assign) to change the owner.")),
                (IN_PROGRESS, DONE) => {
                    crate::workflow::reopen(conn, id, actor, "Reopened by staff")?;
                }
                (IN_PROGRESS, UNASSIGNED) => return Err(AppError::coded("INVALID", "Claim or assign the ticket before starting work.")),
                (IN_PROGRESS, _) => {
                    crate::workflow::start_work(conn, id, actor)?;
                }
                (REVIEW, _) => {
                    crate::workflow::flag_review(conn, id, "Flagged for review", actor)?;
                }
                _ => {}
            }
        }
        if p.priority.is_none() && p.issue_type.is_none() && p.assigned_staff.is_none() && p.internal_notes.is_none() && p.resolution_notes.is_none()
            && p.next_followup_date.is_none() && p.sender_name.is_none() && p.sender_email.is_none() && p.staff_actions.is_none()
            && p.clear_duplicate_flag.is_none() && p.clear_needs_review.is_none()
        {
            return get(conn, id);
        }
        return update(conn, id, &TicketPatch { status: None, ..p.clone() }, actor);
    }
    // the old single notes box: a changed text becomes a new note (notes are separate records now)
    if let Some(n) = p.internal_notes.as_deref().map(str::trim).filter(|n| !n.is_empty() && n.len() <= 50_000) {
        let last: Option<String> = conn.query_row("SELECT body FROM ticket_notes WHERE ticket_id=?1 AND deleted=0 ORDER BY id DESC LIMIT 1", [id], |r| r.get(0)).optional()?;
        if last.as_deref() != Some(n) {
            crate::notes::add(conn, id, n, actor)?;
        }
    }
    let tx = conn.unchecked_transaction()?;
    let cur = get(&tx, id)?;
    let mark = crate::sync_queue::last_history_row(&tx)?;
    let issues_before: Vec<String> = issues(&tx, id)?.into_iter().map(|i| i.issue_type).collect();
    let mut shared_orig = serde_json::Map::new();
    let mut shared_prop = serde_json::Map::new();
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
                    if ["sender_name", "sender_email", "staff_actions"].contains(&$col) {
                        shared_orig.insert($col.into(), serde_json::json!(old));
                        shared_prop.insert($col.into(), serde_json::json!(if v.is_empty() { None } else { Some(v.clone()) }));
                    }
                    changed = true;
                }
            }
        };
    }
    if let Some(s) = &p.priority {
        if !PRIORITIES.contains(&s.as_str()) {
            return Err(AppError::coded("INVALID", "Unknown priority"));
        }
        if *s != cur.priority {
            tx.execute("UPDATE tickets SET priority=?2 WHERE id=?1", params![id, s])?;
            history(&tx, id, actor, "Priority changed", Some("priority"), Some(&cur.priority), Some(s))?;
            shared_orig.insert("priority".into(), serde_json::json!(cur.priority));
            shared_prop.insert("priority".into(), serde_json::json!(s));
            changed = true;
        }
    }
    if let Some(it) = &p.issue_type {
        if catalog().issue(it).is_none() {
            return Err(AppError::coded("INVALID", "Unknown issue type"));
        }
        if *it != cur.issue_type || cur.issue_type_source != "manual" {
            tx.execute("UPDATE tickets SET issue_type=?2, issue_type_source='manual' WHERE id=?1", params![id, it])?;
            tx.execute(
                "INSERT INTO ticket_issues(ticket_id,issue_type,source,position) VALUES(?1,?2,'manual',-1)
                 ON CONFLICT(ticket_id,issue_type) DO UPDATE SET source='manual', position=-1",
                params![id, it],
            )?;
            history(&tx, id, actor, "Issue type set", Some("issue_type"), Some(&cur.issue_type), Some(it))?;
            changed = true;
        }
    }
    upd!(assigned_staff, "assigned_staff", |v: &str| v.len() <= 200);
    upd!(resolution_notes, "resolution_notes", |v: &str| v.len() <= 50_000);
    upd!(next_followup_date, "next_followup_date", |v: &str| v.is_empty() || chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok());
    upd!(sender_name, "sender_name", |v: &str| v.len() <= 300);
    upd!(sender_email, "sender_email", |v: &str| v.is_empty() || (v.contains('@') && v.len() <= 300));
    upd!(staff_actions, "staff_actions", |v: &str| v.len() <= 50_000);
    if p.clear_duplicate_flag == Some(true) && cur.possible_duplicate_of.is_some() {
        tx.execute("UPDATE tickets SET possible_duplicate_of=NULL WHERE id=?1", [id])?;
        history(&tx, id, actor, "Duplicate flag cleared", None, cur.possible_duplicate_of.map(display_id).as_deref(), None)?;
        changed = true;
    }
    if p.clear_needs_review == Some(true) && cur.needs_review {
        tx.execute("UPDATE tickets SET needs_review=0 WHERE id=?1", [id])?;
        history(&tx, id, actor, "Review flag cleared", None, None, None)?;
        changed = true;
    }
    if !shared_prop.is_empty() {
        let op = crate::sync_queue::enqueue(&tx, actor, crate::sync_queue::NewOp {
            ticket_id: Some(id), entity_type: "ticket", entity_id: None, op_type: "ticket.update",
            original: serde_json::Value::Object(shared_orig), proposed: serde_json::Value::Object(shared_prop),
        })?;
        crate::sync_queue::tag_history(&tx, id, &op, mark)?;
    }
    let issues_after: Vec<String> = issues(&tx, id)?.into_iter().map(|i| i.issue_type).collect();
    if issues_after != issues_before {
        crate::sync_queue::enqueue(&tx, actor, crate::sync_queue::NewOp {
            ticket_id: Some(id), entity_type: "ticket", entity_id: None, op_type: "issues.set",
            original: serde_json::json!({"list": issues_before}), proposed: serde_json::json!({"list": issues_after}),
        })?;
    }
    if changed {
        touch(&tx, id)?;
        crate::search::reindex(&tx, id)?;
    }
    tx.commit()?;
    get(conn, id)
}

/// Reopen a Done ticket (local status only - Outlook is not touched): back to In Progress when it has
/// an owner, otherwise Unassigned. Explicit Reopen is required before a Done ticket can be assigned.
pub fn reopen(conn: &Connection, id: i64, actor: &str, reason: &str) -> AppResult<()> {
    let (cur, assignee): (String, Option<i64>) = conn.query_row("SELECT status, assignee_staff_id FROM tickets WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if cur != DONE {
        return Ok(());
    }
    let new = if assignee.is_some() { IN_PROGRESS } else { UNASSIGNED };
    conn.execute("UPDATE tickets SET status=?2, completed_at=NULL, updated_at=?3 WHERE id=?1", params![id, new, now()])?;
    history(conn, id, actor, reason, Some("status"), Some(&cur), Some(new))?;
    Ok(())
}

/// Start Work: Assigned (or Review) -> In Progress, keeping the owner.
pub fn start_work(conn: &Connection, id: i64, actor: &str) -> AppResult<Ticket> {
    crate::workflow::start_work(conn, id, actor)
}

/// Stable reference for a ticket, the same on every computer that imports the same original email
/// (derived from its Internet Message-ID). Local T-numbers differ between computers.
pub fn ensure_ticket_ref(conn: &Connection, id: i64) -> AppResult<String> {
    if let Some(r) = conn.query_row("SELECT ticket_ref FROM tickets WHERE id=?1", [id], |r| r.get::<_, Option<String>>(0))? {
        return Ok(r);
    }
    let imid: Option<String> = conn
        .query_row(
            "SELECT m.internet_message_id FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
             WHERE l.ticket_id=?1 AND COALESCE(m.internet_message_id,'')<>'' ORDER BY (l.link_type='original') DESC, m.received_at, m.id LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let seed = match imid {
        Some(i) => i,
        None => conn.query_row("SELECT 'local|' || id || '|' || created_at || '|' || COALESCE(original_subject,'') FROM tickets WHERE id=?1", [id], |r| r.get(0))?,
    };
    let r = format!("ASR-{:08X}", (crate::scope::fnv64(&seed) >> 32) as u32);
    conn.execute("UPDATE tickets SET ticket_ref=?2 WHERE id=?1", params![id, r])?;
    Ok(r)
}

/// Set a ticket Done locally (low-level; staff use `workflow::mark_done`, which also records coverage).
pub fn set_done(conn: &Connection, id: i64, actor: &str, note: &str) -> AppResult<()> {
    let ts = now();
    let cur: String = conn.query_row("SELECT status FROM tickets WHERE id=?1", [id], |r| r.get(0))?;
    conn.execute("UPDATE tickets SET status=?2, completed_at=?3, updated_at=?3 WHERE id=?1", params![id, DONE, ts])?;
    history(conn, id, actor, note, Some("status"), Some(&cur), Some(DONE))?;
    Ok(())
}

// ---------------------------------------------------------------- issues and schools

pub fn issues(conn: &Connection, ticket_id: i64) -> AppResult<Vec<IssueRow>> {
    let mut st = conn.prepare("SELECT issue_type, source, position FROM ticket_issues WHERE ticket_id=?1 ORDER BY position, issue_type")?;
    let v = st
        .query_map([ticket_id], |r| Ok(IssueRow { issue_type: r.get(0)?, source: r.get(1)?, position: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// Staff sets the ticket's issue types (first = primary). Later scans never change a staff selection.
pub fn set_issues(conn: &Connection, ticket_id: i64, list: &[String], actor: &str) -> AppResult<Vec<IssueRow>> {
    let cat = catalog();
    let mut clean: Vec<String> = Vec::new();
    for i in list {
        if cat.issue(i).is_none() {
            return Err(AppError::coded("INVALID", format!("Unknown issue type {i}")));
        }
        if !clean.contains(i) {
            clean.push(i.clone());
        }
    }
    if clean.is_empty() {
        return Err(AppError::coded("INVALID", "Select at least one issue type."));
    }
    let tx = conn.unchecked_transaction()?;
    let before: Vec<String> = issues(&tx, ticket_id)?.into_iter().map(|i| i.issue_type).collect();
    tx.execute("DELETE FROM ticket_issues WHERE ticket_id=?1", [ticket_id])?;
    for (pos, i) in clean.iter().enumerate() {
        tx.execute("INSERT INTO ticket_issues(ticket_id,issue_type,source,position) VALUES(?1,?2,'manual',?3)", params![ticket_id, i, pos as i64])?;
    }
    tx.execute("UPDATE tickets SET issue_type=?2, issue_type_source='manual' WHERE id=?1", params![ticket_id, clean[0]])?;
    if before != clean {
        let mark = crate::sync_queue::last_history_row(&tx)?;
        history(&tx, ticket_id, actor, "Issue types set", Some("issue_type"), Some(&before.join(", ")), Some(&clean.join(", ")))?;
        let op = crate::sync_queue::enqueue(&tx, actor, crate::sync_queue::NewOp {
            ticket_id: Some(ticket_id), entity_type: "ticket", entity_id: None, op_type: "issues.set",
            original: serde_json::json!({"list": before}), proposed: serde_json::json!({"list": clean}),
        })?;
        crate::sync_queue::tag_history(&tx, ticket_id, &op, mark)?;
    }
    touch(&tx, ticket_id)?;
    crate::search::reindex(&tx, ticket_id)?;
    tx.commit()?;
    issues(conn, ticket_id)
}

pub fn school_slots(conn: &Connection, ticket_id: i64) -> AppResult<Vec<i64>> {
    let mut st = conn.prepare("SELECT slot FROM ticket_schools WHERE ticket_id=?1 AND removed=0 ORDER BY slot")?;
    let v: Vec<i64> = st.query_map([ticket_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(if v.is_empty() { vec![1] } else { v })
}

fn ensure_school_slot(conn: &Connection, ticket_id: i64, slot: i64, actor: &str) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO ticket_schools(ticket_id,slot,created_by,created_at) VALUES(?1,?2,?3,?4)",
        params![ticket_id, slot, actor, now()],
    )?;
    Ok(())
}

pub fn add_school(conn: &Connection, ticket_id: i64, actor: &str) -> AppResult<i64> {
    get(conn, ticket_id)?;
    let next: i64 = conn.query_row("SELECT COALESCE(MAX(slot),0)+1 FROM ticket_schools WHERE ticket_id=?1", [ticket_id], |r| r.get(0))?;
    let next = next.max(2);
    let tx = conn.unchecked_transaction()?;
    ensure_school_slot(&tx, ticket_id, 1, SYSTEM)?;
    ensure_school_slot(&tx, ticket_id, next, actor)?;
    let mark = crate::sync_queue::last_history_row(&tx)?;
    history(&tx, ticket_id, actor, "School added", None, None, Some(&format!("School {next}")))?;
    let op = crate::sync_queue::enqueue(&tx, actor, crate::sync_queue::NewOp {
        ticket_id: Some(ticket_id), entity_type: "school", entity_id: Some(next.to_string()), op_type: "school.add", original: serde_json::json!({}), proposed: serde_json::json!({}),
    })?;
    crate::sync_queue::tag_history(&tx, ticket_id, &op, mark)?;
    touch(&tx, ticket_id)?;
    tx.commit()?;
    Ok(next)
}

/// Hide a school group. Its field values stay stored (and in history); a removed group is not shown or prompted.
pub fn remove_school(conn: &Connection, ticket_id: i64, slot: i64, actor: &str) -> AppResult<()> {
    if slot <= 1 {
        return Err(AppError::coded("INVALID", "The first school group cannot be removed; clear its fields instead."));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE ticket_schools SET removed=1 WHERE ticket_id=?1 AND slot=?2", params![ticket_id, slot])?;
    let mark = crate::sync_queue::last_history_row(&tx)?;
    history(&tx, ticket_id, actor, "School removed", None, Some(&format!("School {slot}")), None)?;
    let op = crate::sync_queue::enqueue(&tx, actor, crate::sync_queue::NewOp {
        ticket_id: Some(ticket_id), entity_type: "school", entity_id: Some(slot.to_string()), op_type: "school.remove", original: serde_json::json!({}), proposed: serde_json::json!({}),
    })?;
    crate::sync_queue::tag_history(&tx, ticket_id, &op, mark)?;
    touch(&tx, ticket_id)?;
    crate::search::reindex(&tx, ticket_id)?;
    tx.commit()?;
    Ok(())
}

pub fn completion_rows(conn: &Connection, ticket_id: i64) -> AppResult<Vec<CompletionRow>> {
    let mut st = conn.prepare(
        "SELECT message_id, flagged_at, moved_at, dest_path, last_error, last_error_at, attempts FROM message_completion WHERE ticket_id=?1 ORDER BY message_id",
    )?;
    let v = st
        .query_map([ticket_id], |r| {
            Ok(CompletionRow {
                message_id: r.get(0)?,
                flagged_at: r.get(1)?,
                moved_at: r.get(2)?,
                dest_path: r.get(3)?,
                last_error: r.get(4)?,
                last_error_at: r.get(5)?,
                attempts: r.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// Remember the prompt text that was copied to the clipboard.
pub fn record_prompt(conn: &Connection, ticket_id: i64, text: &str, actor: &str) -> AppResult<String> {
    let ts = now();
    let n = conn.execute("UPDATE tickets SET last_prompt=?2, last_prompt_at=?3 WHERE id=?1", params![ticket_id, text, ts])?;
    if n == 0 {
        return Err(AppError::coded("NOT_FOUND", format!("Ticket {} not found", display_id(ticket_id))));
    }
    history(conn, ticket_id, actor, "Copilot prompt copied", None, None, Some(&format!("{} characters", text.chars().count())))?;
    Ok(ts)
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
        "INSERT INTO tickets(status,priority,issue_type,issue_type_source,classification_confidence,sender_name,sender_email,original_subject,assigned_staff,source,created_at,updated_at,first_received_at)
         VALUES(?9,'Normal',?1,?2,?3,NULLIF(?4,''),NULLIF(?5,''),NULLIF(?6,''),NULLIF(?7,''),'manual',?8,?8,?8)",
        params![issue, source, analysis.confidence, n.sender_name.trim(), n.sender_email.trim(), n.subject.trim(), settings.default_assigned_staff, ts, UNASSIGNED],
    )?;
    let id = tx.last_insert_rowid();
    let issue_list = if source == "manual" { vec![issue.clone()] } else { analysis.issues.clone() };
    for (pos, i) in issue_list.iter().enumerate() {
        tx.execute("INSERT OR IGNORE INTO ticket_issues(ticket_id,issue_type,source,position) VALUES(?1,?2,?3,?4)", params![id, i, source, pos as i64])?;
    }
    for slot in 1..=analysis.school_count.max(1) {
        ensure_school_slot(&tx, id, slot, SYSTEM)?;
    }
    for e in &analysis.fields {
        set_extracted(&tx, id, &e.key, &e.value, &e.confidence, false)?;
    }
    if !n.body.trim().is_empty() {
        // keep the pasted text as the ticket's original message so it is searchable and used in the prompt
        tx.execute(
            "INSERT INTO email_messages(subject,sender_name,sender_email,received_at,body_text,imported_at) VALUES(?1,?2,?3,?4,?5,?4)",
            params![n.subject.trim(), n.sender_name.trim(), n.sender_email.trim().to_lowercase(), ts, n.body],
        )?;
        let mid = tx.last_insert_rowid();
        tx.execute("INSERT INTO ticket_email_links(ticket_id,message_id,link_type,linked_at) VALUES(?1,?2,'manual',?3)", params![id, mid, ts])?;
    }
    history(&tx, id, actor, "Ticket created manually", None, None, Some(&issue))?;
    ensure_ticket_ref(&tx, id)?;
    crate::scope::recompute_ticket(&tx, id, actor)?;
    tx.commit()?;
    if !n.school_njiis_id.trim().is_empty() {
        set_field(conn, id, "school_njiis_id", Some(n.school_njiis_id.clone()), false, actor)?;
    }
    if !n.school_name.trim().is_empty() {
        set_field(conn, id, "school_name", Some(n.school_name.clone()), false, actor)?;
    }
    refresh_school_columns(conn, id)?;
    crate::search::reindex(conn, id)?;
    // queued for the shared database with everything entered above (one snapshot)
    let tx = conn.unchecked_transaction()?;
    crate::sync_queue::on_ticket_created(&tx, id, actor, "manual")?;
    tx.commit()?;
    Ok(id)
}

pub fn messages(conn: &Connection, ticket_id: i64) -> AppResult<Vec<MessageRow>> {
    let mut st = conn.prepare(
        "SELECT m.id,m.subject,m.sender_name,m.sender_email,m.received_at,m.body_text,m.body_truncated,m.folder_path,l.link_type,m.is_internal,m.has_attachments,m.entry_id,m.store_id,
                m.scope_state,m.out_reason,m.location_status,m.location_checked_at,(SELECT display_name FROM staff s WHERE s.id=m.owner_staff_id)
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
                scope_state: r.get(13)?,
                out_reason: r.get(14)?,
                location_status: r.get(15)?,
                location_checked_at: r.get(16)?,
                owner_name: r.get(17)?,
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
    Ok(TicketDetail {
        ticket: get(conn, id)?,
        fields: fields(conn, id)?,
        messages: messages(conn, id)?,
        history: history_rows(conn, id)?,
        issues: issues(conn, id)?,
        schools: school_slots(conn, id)?,
        completion: completion_rows(conn, id)?,
        assignments: crate::assignment::ops(conn, id)?,
        notes: crate::notes::list(conn, id)?,
        ownership: ownership_rows(conn, id)?,
        completions: completion_decisions(conn, id)?,
        conflicts: crate::sync_queue::conflicts(conn, Some(id), false)?,
        pending_ops: crate::sync_queue::ticket_ops(conn, id)?,
    })
}

pub fn ownership_rows(conn: &Connection, id: i64) -> AppResult<Vec<OwnershipRow>> {
    let mut st = conn.prepare(
        "SELECT COALESCE(staff_name, (SELECT display_name FROM staff s WHERE s.id=o.staff_id)), started_at, ended_at, started_by, start_reason, end_reason, origin
         FROM ownership_history o WHERE ticket_id=?1 ORDER BY started_at, id",
    )?;
    let v = st
        .query_map([id], |r| Ok(OwnershipRow { staff_name: r.get(0)?, started_at: r.get(1)?, ended_at: r.get(2)?, started_by: r.get(3)?, start_reason: r.get(4)?, end_reason: r.get(5)?, origin: r.get(6)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn completion_decisions(conn: &Connection, id: i64) -> AppResult<Vec<CompletionDecision>> {
    let mut st = conn.prepare("SELECT completion_id, actor_name, decided_at, json_array_length(covered_json), state, state_reason FROM completion_history WHERE ticket_id=?1 ORDER BY decided_at DESC, id DESC")?;
    let v = st
        .query_map([id], |r| Ok(CompletionDecision { completion_id: r.get(0)?, actor_name: r.get(1)?, decided_at: r.get(2)?, covered: r.get(3)?, state: r.get(4)?, state_reason: r.get(5)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
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
    /// EntryIDs of the folder's parents (parent first, up to the store root)
    pub folder_ancestors: Vec<String>,
    /// Set when the email is an assignment notification sent by this app (never imported)
    pub notification_id: String,
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
    pub skipped_notifications: usize,
    pub duplicates_flagged: usize,
    pub max_received_at: Option<String>,
}

/// Location fields of a scanned email: (ancestors JSON, inferred owner, scope membership).
fn scanned_location(m: &IncomingMessage, scope: &crate::scope::ScopeCfg, staff: &[crate::staff::Staff]) -> (String, Option<i64>, &'static str) {
    let mut chain = vec![m.folder_entry_id.clone()];
    chain.extend(m.folder_ancestors.iter().cloned());
    let owner = crate::staff::owner_for(staff, &chain);
    // found by a scan of the selected scope; with ancestors we can double-check (scans never read excluded folders)
    let st = if m.folder_ancestors.is_empty() || crate::scope::in_scope(scope, &chain) { "in" } else { "out" };
    (serde_json::to_string(&m.folder_ancestors).unwrap_or_else(|_| "[]".into()), owner, st)
}

fn find_message(conn: &Connection, m: &IncomingMessage) -> AppResult<Option<(i64, Option<String>, Option<String>)>> {
    if !m.internet_message_id.is_empty() {
        // Outlook reports "<id>"; copies received from the shared database may be stored in either form
        let bare = crate::shared_apply::normalize_message_id(&m.internet_message_id);
        if let Some(r) = conn
            .query_row(
                "SELECT id, entry_id, folder_path FROM email_messages WHERE internet_message_id IN (?1, ?2, ?3) ORDER BY id LIMIT 1",
                params![m.internet_message_id, bare, format!("<{bare}>")],
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
    let scope = crate::scope::cfg(settings);
    let staff = crate::staff::list(conn, true)?;
    let mut touched: std::collections::BTreeSet<i64> = Default::default();
    let tx = conn.unchecked_transaction()?;
    for m in msgs {
        rep.seen += 1;
        if rep.max_received_at.as_deref().map(|x| m.received_at.as_str() > x).unwrap_or(true) && !m.received_at.is_empty() {
            rep.max_received_at = Some(m.received_at.clone());
        }
        // 0. assignment notifications sent by this app never become tickets or ticket emails
        if !m.notification_id.is_empty() {
            rep.skipped_notifications += 1;
            continue;
        }
        let (ancestors, owner, scope_state) = scanned_location(m, &scope, &staff);
        // 1. dedup by Internet Message ID / EntryID; detect folder moves
        if let Some((mid, old_entry, old_path)) = find_message(&tx, m)? {
            rep.already_imported += 1;
            let moved = old_entry.as_deref() != Some(m.entry_id.as_str()) || old_path.as_deref() != Some(m.folder_path.as_str());
            if !m.entry_id.is_empty() {
                // seen by a scan: confirmed location (also when unchanged)
                tx.execute(
                    "UPDATE email_messages SET entry_id=?2, store_id=?3, folder_entry_id=?4, folder_path=?5, folder_ancestors=?6, owner_staff_id=?7,
                       scope_state=?8, scope_key=?9, location_status='found', location_checked_at=?10, out_reason=NULL WHERE id=?1",
                    params![mid, m.entry_id, m.store_id, m.folder_entry_id, m.folder_path, ancestors, owner, scope_state, scope.key, now()],
                )?;
            }
            let mut st = tx.prepare("SELECT ticket_id FROM ticket_email_links WHERE message_id=?1")?;
            let tids: Vec<i64> = st.query_map([mid], |r| r.get(0))?.collect::<Result<_, _>>()?;
            for tid in tids {
                if moved && !m.entry_id.is_empty() {
                    history(&tx, tid, SYSTEM, "Email moved in Outlook", Some("folder"), old_path.as_deref(), Some(&m.folder_path))?;
                }
                touched.insert(tid);
            }
            if moved && !m.entry_id.is_empty() {
                rep.moved += 1;
            }
            continue;
        }
        let internal = settings.is_internal_sender(&m.sender_email);
        let preview_meta = serde_json::to_string(&m.metadata).unwrap_or_else(|_| "{}".into());
        tx.execute(
            "INSERT INTO email_messages(store_id,entry_id,internet_message_id,conversation_id,folder_entry_id,folder_path,subject,sender_name,sender_email,received_at,body_text,body_truncated,has_attachments,is_internal,metadata_json,imported_at,
                                        folder_ancestors,owner_staff_id,scope_state,scope_key,location_status,location_checked_at)
             VALUES(?1,?2,NULLIF(?3,''),NULLIF(?4,''),?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,'found',?16)",
            params![m.store_id, m.entry_id, m.internet_message_id, m.conversation_id, m.folder_entry_id, m.folder_path, m.subject,
                m.sender_name, m.sender_email.to_lowercase(), m.received_at, m.body, m.body_truncated as i64, m.has_attachments as i64,
                internal as i64, preview_meta, now(), ancestors, owner, scope_state, scope.key],
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
                crate::sync_queue::on_email_linked(&tx, tid, mid, "internal", SYSTEM)?;
                crate::search::reindex(&tx, tid)?;
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
            crate::sync_queue::on_email_linked(&tx, tid, mid, "reply", SYSTEM)?;
            // only fill values that were never extracted; staff-entered values are untouched
            for e in &analysis.fields {
                if e.key != "issue_description" {
                    set_extracted(&tx, tid, &e.key, &e.value, &e.confidence, true)?;
                }
            }
            for slot in 1..=analysis.school_count {
                ensure_school_slot(&tx, tid, slot, SYSTEM)?;
            }
            // new issues mentioned in the reply are added unless staff already chose the issue types
            let staff_issues: i64 = tx.query_row("SELECT COUNT(*) FROM ticket_issues WHERE ticket_id=?1 AND source='manual'", [tid], |r| r.get(0))?;
            if staff_issues == 0 && analysis.issue_type != "other" {
                for i in &analysis.issues {
                    tx.execute(
                        "INSERT OR IGNORE INTO ticket_issues(ticket_id,issue_type,source,position) VALUES(?1,?2,'auto',(SELECT COALESCE(MAX(position),0)+1 FROM ticket_issues WHERE ticket_id=?1))",
                        params![tid, i],
                    )?;
                }
            }
            refresh_school_columns(&tx, tid)?;
            // a genuinely new substantive reply moves a Done ticket to Review (rescans never get here: dedup
            // above; older emails found later and automatic replies do not reopen)
            if status == DONE {
                crate::workflow::on_new_incoming(&tx, tid, mid)?;
            }
            crate::search::reindex(&tx, tid)?;
            touched.insert(tid);
            rep.updated_tickets += 1;
            continue;
        }

        // 3. new ticket
        let ts = now();
        tx.execute(
            "INSERT INTO tickets(status,priority,issue_type,issue_type_source,classification_confidence,sender_name,sender_email,original_subject,assigned_staff,conversation_id,source,created_at,updated_at,last_email_received_at,first_received_at)
             VALUES(?10,'Normal',?1,'auto',?2,NULLIF(?3,''),NULLIF(?4,''),NULLIF(?5,''),NULLIF(?6,''),NULLIF(?7,''),'outlook',?8,?8,?9,COALESCE(NULLIF(?9,''),?8))",
            params![analysis.issue_type, analysis.confidence, m.sender_name, m.sender_email.to_lowercase(), m.subject,
                settings.default_assigned_staff, m.conversation_id, ts, m.received_at, UNASSIGNED],
        )?;
        let tid = tx.last_insert_rowid();
        touched.insert(tid);
        tx.execute("INSERT INTO ticket_email_links(ticket_id,message_id,link_type,linked_at) VALUES(?1,?2,'original',?3)", params![tid, mid, ts])?;
        for (pos, i) in analysis.issues.iter().enumerate() {
            tx.execute("INSERT OR IGNORE INTO ticket_issues(ticket_id,issue_type,source,position) VALUES(?1,?2,'auto',?3)", params![tid, i, pos as i64])?;
        }
        for slot in 1..=analysis.school_count.max(1) {
            ensure_school_slot(&tx, tid, slot, SYSTEM)?;
        }
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
                "SELECT id FROM tickets WHERE id<>?1 AND created_at>=?2 AND issue_type=?3
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
            tx.execute("UPDATE tickets SET needs_review=1 WHERE id=?1", [tid])?;
        }
        crate::search::reindex(&tx, tid)?;
        ensure_ticket_ref(&tx, tid)?;
        crate::sync_queue::on_ticket_created(&tx, tid, SYSTEM, "outlook")?;
        rep.created_tickets += 1;
    }
    // placement (current/history) and owner inferred from staff folders - never a notification
    for tid in touched {
        crate::scope::recompute_ticket(&tx, tid, SYSTEM)?;
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
        crate::workflow::mark_done(&c, 1, "staff").unwrap();
        let r = ingest(&c, &[msg(2, "C1", "RE: Subtype", "We are a charter school. School ID: 123456", "jane@school.org")], &s).unwrap();
        assert_eq!(r.updated_tickets, 1);
        assert_eq!(r.created_tickets, 0);
        let t = get(&c, 1).unwrap();
        assert_eq!(t.status, REVIEW, "a new reply moves a Done ticket to Review");
        assert!(t.review_reason.as_deref().unwrap_or("").starts_with("New reply from"));
        assert!(history_rows(&c, 1).unwrap().iter().any(|h| h.action.starts_with("Reopened for review: new reply from")));
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
        assert_eq!(t2.status, UNASSIGNED, "review hints are badges, not statuses");
        assert!(t2.needs_review);
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
        let l = list(&c, &ListQuery { status: UNASSIGNED.into(), search: "maple".into(), ..Default::default() }).unwrap();
        assert_eq!(l.total, 1);
        let l = list(&c, &ListQuery { search: format!("T-{id:06}"), ..Default::default() }).unwrap();
        assert_eq!(l.total, 1);
        assert!(update(&c, id, &TicketPatch { status: Some("Bogus".into()), ..Default::default() }, "staff").is_err());
        assert!(update(&c, id, &TicketPatch { status: Some(DONE.into()), ..Default::default() }, "staff").is_err(), "Done only via Mark Done");
        set_done(&c, id, "staff", "Marked Done").unwrap();
        update(&c, id, &TicketPatch { status: Some(IN_PROGRESS.into()), next_followup_date: Some("2026-11-20".into()), ..Default::default() }, "staff").unwrap();
        let h = history_rows(&c, id).unwrap();
        assert!(h.iter().any(|x| x.action == "Reopened by staff"));
        assert_eq!(get(&c, id).unwrap().status, UNASSIGNED, "reopened without an owner");
        // revert field to extracted
        set_field(&c, id, "school_name", None, false, "staff").unwrap();
        assert_eq!(get(&c, id).unwrap().school_name, None);
    }

    #[test]
    fn rescan_never_reopens_and_never_overwrites_staff_values() {
        let c = conn();
        let s = Settings::default();
        let m = msg(1, "C1", "Subtype", "Our school ID is 0012345. We are missing the subtype.", "jane@school.org");
        ingest(&c, &[m.clone()], &s).unwrap();
        set_field(&c, 1, "school_njiis_id", Some("0099999".into()), false, "staff").unwrap();
        set_field(&c, 1, "sender_title", Some("Head Nurse".into()), false, "staff").unwrap();
        set_done(&c, 1, "staff", "Marked Done").unwrap();
        // the same email seen again (e.g. after it was moved to the archive folder)
        let mut moved = m.clone();
        moved.entry_id = "E1-archived".into();
        moved.folder_path = "Inbox/Done".into();
        let r = ingest(&c, &[m.clone(), moved], &s).unwrap();
        assert_eq!(r.new_messages, 0);
        let t = get(&c, 1).unwrap();
        assert_eq!(t.status, DONE, "rescans never reopen");
        assert_eq!(t.school_njiis_id.as_deref(), Some("0099999"));
        let f = fields(&c, 1).unwrap();
        assert_eq!(f.iter().find(|x| x.key == "sender_title").unwrap().effective, "Head Nurse");
        // an internal (staff) message does not reopen either
        let mut s2 = s.clone();
        s2.internal_senders = vec!["@doh.nj.gov".into()];
        ingest(&c, &[msg(2, "C1", "RE: Subtype", "Done on our side.", "me@doh.nj.gov")], &s2).unwrap();
        assert_eq!(get(&c, 1).unwrap().status, DONE);
    }

    #[test]
    fn search_covers_fields_bodies_notes_and_leading_zero_ids() {
        let c = conn();
        let s = Settings::default();
        ingest(&c, &[msg(1, "C1", "Webinar", "The webinar shows a black screen. Our school ID is 0012345.\nThanks,\nAnn Lee\nSchool Nurse\nPhone: 609-555-0100 x12", "ann@school.org")], &s).unwrap();
        ingest(&c, &[msg(2, "C2", "Account", "I need an account. ASR ID: Q-778 for Oak Academy.", "bob@other.org")], &s).unwrap();
        update(&c, 2, &TicketPatch { internal_notes: Some("called the principal".into()), staff_actions: Some("Reset password".into()), ..Default::default() }, "staff").unwrap();
        let find = |q: &str| list(&c, &ListQuery { search: q.into(), ..Default::default() }).unwrap().rows.iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(find("0012345"), vec![1], "leading-zero NJIIS ID");
        assert_eq!(find("609-555-0100"), vec![1], "sender phone");
        assert_eq!(find("black screen"), vec![1], "email body");
        assert_eq!(find("Q-778"), vec![2], "other school ID");
        assert_eq!(find("principal"), vec![2], "staff notes");
        assert_eq!(find("reset password"), vec![2], "staff actions");
        assert_eq!(find("bob@other.org"), vec![2], "sender email");
        assert_eq!(find("T-000002"), vec![2], "ticket ID");
        assert!(find("nonexistentword").is_empty());
        let l = list(&c, &ListQuery { issue_type: "webinar_issue".into(), ..Default::default() }).unwrap();
        assert_eq!(l.total, 1);
    }

    #[test]
    fn date_filters_are_inclusive_and_sortable() {
        let c = conn();
        let s = Settings::default();
        let mut a = msg(1, "C1", "a", "user guide please", "a@x.org");
        a.received_at = "2026-11-10T04:59:00Z".into(); // Nov 9, 23:59 in New York (UTC-5)
        let mut b = msg(2, "C2", "b", "user guide please", "b@x.org");
        b.received_at = "2026-11-10T05:00:00Z".into(); // Nov 10, 00:00 in New York
        let mut d = msg(3, "C3", "d", "user guide please", "d@x.org");
        d.received_at = "2026-11-11T04:59:59Z".into(); // Nov 10, 23:59:59 in New York
        ingest(&c, &[a, b, d], &s).unwrap();
        // the UI sends local midnight of the start date and local midnight of the day after the end date
        let q = ListQuery { received_from: "2026-11-10T00:00:00-05:00".into(), received_to: "2026-11-11T00:00:00-05:00".into(), ..Default::default() };
        let ids: Vec<i64> = list(&c, &q).unwrap().rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![3, 2], "end date inclusive, previous day excluded, newest first");
        set_done(&c, 2, "staff", "Marked Done").unwrap();
        c.execute("UPDATE tickets SET completed_at='2026-11-12T15:00:00Z' WHERE id=2", []).unwrap();
        let q = ListQuery { completed_from: "2026-11-12T00:00:00-05:00".into(), completed_to: "2026-11-13T00:00:00-05:00".into(), ..Default::default() };
        assert_eq!(list(&c, &q).unwrap().total, 1);
        let q = ListQuery { status: DONE.into(), ..Default::default() };
        assert_eq!(list(&c, &q).unwrap().total, 1);
        let sorted = list(&c, &ListQuery { sort: "completed".into(), ..Default::default() }).unwrap();
        assert_eq!(sorted.rows[0].id, 2, "completed tickets first when sorting by completion date");
        let page = list(&c, &ListQuery { limit: 2, offset: 2, ..Default::default() }).unwrap();
        assert_eq!((page.total, page.rows.len()), (3, 1), "paging");
        assert!(list(&c, &ListQuery { received_from: "yesterday".into(), ..Default::default() }).is_err());
    }

    #[test]
    fn issues_and_schools_are_editable() {
        let c = conn();
        let s = Settings::default();
        ingest(&c, &[msg(1, "C1", "Black screen", "The webinar shows a black screen.", "a@b.org")], &s).unwrap();
        let v = set_issues(&c, 1, &["school_subtype_missing".into(), "webinar_issue".into()], "staff").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(get(&c, 1).unwrap().issue_type, "school_subtype_missing");
        assert!(set_issues(&c, 1, &[], "staff").is_err());
        let slot = add_school(&c, 1, "staff").unwrap();
        assert_eq!(slot, 2);
        set_field(&c, 1, &crate::analyzer::school_key(2, "njiis_id"), Some("0001".into()), false, "staff").unwrap();
        assert_eq!(school_slots(&c, 1).unwrap(), vec![1, 2]);
        remove_school(&c, 1, 2, "staff").unwrap();
        assert_eq!(school_slots(&c, 1).unwrap(), vec![1]);
        assert!(remove_school(&c, 1, 1, "staff").is_err());
        // a reply does not override the staff issue selection
        ingest(&c, &[msg(2, "C1", "RE", "How do I save the form as a PDF?", "a@b.org")], &s).unwrap();
        assert_eq!(issues(&c, 1).unwrap().len(), 2);
    }
}
