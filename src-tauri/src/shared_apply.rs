//! Applies queued operations to the shared database, one short transaction per operation, and reads
//! shared state back for synchronization.
//!
//! Every operation carries a unique `op_id` that is reused on retries. Inside one transaction this module
//! (1) returns the stored result if the op was already processed (lost acknowledgement), (2) refuses work
//! in maintenance mode, (3) applies the change with conditional checks against the values the user saw
//! (`original`), (4) writes activity, conflicts, the processed-operation result and change-feed rows, and
//! commits. Competing edits never overwrite each other silently: unchanged fields merge, different fields
//! merge, and the same field changed differently becomes a recorded conflict that keeps both values.
//! Workstation timestamps are never used to pick a winner.

use crate::shared_store::{col, col_i, col_s, ErrKind, Row, SResult, SharedConn, SharedError, Val};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// ------------------------------------------------------------------ operation model

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Op {
    pub op_id: String,
    pub op_type: String,
    /// shared ticket id (UUID) when the op concerns a ticket
    pub ticket_id: Option<String>,
    /// note id, field key, staff id, conflict id ... depending on the type
    pub entity_id: Option<String>,
    pub staff_id: Option<String>,
    pub staff_name: String,
    pub device_id: String,
    pub windows_account: String,
    /// shared ticket version the user's change was based on (0 = not yet known)
    pub base_version: i64,
    pub original: Value,
    pub proposed: Value,
    /// local creation time (informational only; never used to choose a winner)
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConflictInfo {
    pub conflict_id: String,
    pub field_name: String,
    /// "field" | "text" | "owner" | "status" | "note" | "duplicate_import" ...
    pub conflict_type: String,
    pub original: Option<String>,
    pub proposed: Option<String>,
    pub shared: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpResult {
    pub op_id: String,
    /// "accepted" | "conflict" (nothing or only part applied; see conflicts) | "rejected"
    pub result: String,
    pub ticket_id: Option<String>,
    /// set when the ticket in the op was found to be a duplicate of an existing shared case
    pub redirected_from: Option<String>,
    pub ticket_version: i64,
    pub ticket_ref: Option<String>,
    pub message: String,
    pub conflicts: Vec<ConflictInfo>,
    /// true when this result was stored by an earlier attempt (acknowledgement had been lost)
    pub duplicate: bool,
    /// op-specific data (e.g. the shared staff id an upsert was matched to)
    pub extra: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmailRec {
    pub email_id: String,
    /// computed by the shared database from its mailbox identity; clients may leave it blank
    pub match_key: String,
    /// "message_id" | "fallback"
    pub match_method: String,
    pub evidence_key: Option<String>,
    pub internet_message_id: String,
    pub conversation_id: String,
    pub subject: String,
    pub sender_name: String,
    pub sender_email: String,
    pub received_at: String,
    pub is_internal: bool,
    pub is_auto_reply: bool,
    pub body_text: String,
    pub body_truncated: bool,
    /// "original" | "reply" | "internal" | "manual"
    pub link_type: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FieldRec {
    pub key: String,
    pub extracted: Option<String>,
    pub manual: Option<String>,
    pub has_manual: bool,
    pub verified: bool,
    pub version: i64,
    pub updated_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NoteRec {
    pub note_id: String,
    pub author_staff_id: Option<String>,
    pub author_name: String,
    pub text: String,
    pub version: i64,
    pub deleted: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OwnershipRec {
    pub entry_id: String,
    pub staff_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub started_by: Option<String>,
    pub ended_by: Option<String>,
    pub start_reason: Option<String>,
    pub end_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CompletionRec {
    pub completion_id: String,
    pub actor_staff_id: Option<String>,
    pub actor_name: String,
    pub decided_at: String,
    pub accepted_at: Option<String>,
    /// shared email ids (or Internet Message-IDs in a create payload) covered by the decision
    pub covered: Vec<String>,
    pub max_covered_received_at: Option<String>,
    /// "accepted" | "superseded" | "legacy"
    pub state: String,
    pub state_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ActivityRec {
    pub activity_id: String,
    pub occurred_at: String,
    /// "staff" | "outlook" | "system" | "sync"
    pub source: String,
    pub actor_staff_id: Option<String>,
    pub actor_name: Option<String>,
    pub device_id: Option<String>,
    pub op_id: Option<String>,
    pub action: String,
    pub field_name: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

/// Case summary columns of the shared Tickets table.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TicketCore {
    pub ticket_id: String,
    pub ticket_ref: String,
    pub subject: Option<String>,
    pub sender_name: Option<String>,
    pub sender_email: Option<String>,
    pub school_name: Option<String>,
    pub njiis_id: Option<String>,
    pub priority: Option<String>,
    pub status: String,
    pub owner_staff_id: Option<String>,
    pub review_reason: Option<String>,
    pub staff_actions: Option<String>,
    pub first_received_at: Option<String>,
    pub last_incoming_at: Option<String>,
    pub done_at: Option<String>,
    pub merged_into: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub origin: Option<String>,
}

/// Payload of `ticket.create`: the whole case as this computer knows it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TicketSnapshot {
    pub core: TicketCore,
    pub emails: Vec<EmailRec>,
    /// (issue type, origin "auto" | "manual") in order
    pub issues: Vec<(String, String)>,
    /// (slot, removed)
    pub schools: Vec<(i64, bool)>,
    pub fields: Vec<FieldRec>,
    pub completion: Option<CompletionRec>,
    /// existing ownership periods (import of earlier local cases), with their stable entry ids so the
    /// shared history and the local history stay one record each
    pub ownership: Vec<OwnershipRec>,
}

/// Everything a client needs to mirror one shared case.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TicketBundle {
    pub core: TicketCore,
    pub emails: Vec<EmailRec>,
    pub issues: Vec<(String, String)>,
    pub schools: Vec<(i64, bool)>,
    pub fields: Vec<FieldRec>,
    pub notes: Vec<NoteRec>,
    pub ownership: Vec<OwnershipRec>,
    pub completions: Vec<CompletionRec>,
    pub activity: Vec<ActivityRec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StaffRec {
    pub staff_id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub windows_account: Option<String>,
    pub active: bool,
    pub version: i64,
}

// ------------------------------------------------------------------ helpers

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Shorten to the column width (characters, not bytes) so Access TEXT(n) columns never overflow.
pub fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}
fn cut_opt(s: &Option<String>, n: usize) -> Val {
    match s {
        Some(v) if !v.is_empty() => Val::Text(cut(v, n)),
        _ => Val::Null,
    }
}
fn txt(s: &Option<String>) -> Val {
    match s {
        Some(v) => Val::Text(v.clone()),
        None => Val::Null,
    }
}
fn opt_eq(a: &Option<String>, b: &Option<String>) -> bool {
    a.as_deref().unwrap_or("") == b.as_deref().unwrap_or("")
}
fn jstr(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Internet Message-ID normalized for matching: trimmed, without the surrounding angle brackets.
/// Case is preserved (the local part of a Message-ID is case-sensitive).
pub fn normalize_message_id(imid: &str) -> String {
    imid.trim().trim_start_matches('<').trim_end_matches('>').trim().to_string()
}

/// Logical mailbox identity: lower-case, trimmed, inner whitespace collapsed.
pub fn normalize_mailbox(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn sha256_hex(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Matching key (not encryption): SHA-256 of "asr-email-v1", the logical mailbox and the normalized
/// Internet Message-ID, UTF-8 encoded and separated by newlines.
pub fn message_match_key(mailbox_key: &str, imid: &str) -> String {
    sha256_hex(&format!("asr-email-v1\n{}\n{}", normalize_mailbox(mailbox_key), normalize_message_id(imid)))
}

/// Evidence key for emails without a Message-ID (sender + received time to the second + subject).
/// It is NEVER used to merge automatically; equal evidence only creates a "possible duplicate" relation.
pub fn fallback_evidence_key(mailbox_key: &str, sender_email: &str, received_at: &str, subject: &str) -> String {
    let subj = subject.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let recv: String = received_at.chars().take(19).collect();
    sha256_hex(&format!("asr-email-fallback-v1\n{}\n{}\n{}\n{}", normalize_mailbox(mailbox_key), sender_email.trim().to_lowercase(), recv, subj))
}

pub fn mailbox_key(c: &mut dyn SharedConn) -> SResult<String> {
    Ok(c
        .query_one("SELECT setting_value FROM SharedSettings WHERE setting_key = ?", &["mailbox_logical_key".into()])?
        .and_then(|r| col(&r, 0))
        .unwrap_or_default())
}

/// Set the logical mailbox identity once (first writer wins; later changes are an explicit admin step).
pub fn ensure_mailbox(c: &mut dyn SharedConn, display_name: &str, actor: &str) -> SResult<String> {
    let cur = mailbox_key(c)?;
    if !cur.is_empty() {
        return Ok(cur);
    }
    let key = normalize_mailbox(display_name);
    if key.is_empty() {
        return Ok(String::new());
    }
    let ts = crate::db::now();
    c.begin()?;
    let r = (|| -> SResult<()> {
        c.exec(
            "INSERT INTO Mailboxes (mailbox_id, logical_key, display_name, created_at) VALUES (?, ?, ?, ?)",
            &[new_id().into(), cut(&key, 255).into(), cut(display_name, 255).into(), ts.clone().into()],
        )?;
        c.exec(
            "INSERT INTO SharedSettings (setting_key, setting_value, row_version, updated_at, updated_by) VALUES (?, ?, 1, ?, ?)",
            &["mailbox_logical_key".into(), key.clone().into(), ts.clone().into(), cut(actor, 100).into()],
        )?;
        Ok(())
    })();
    match r {
        Ok(()) => {
            c.commit()?;
            Ok(key)
        }
        Err(e) => {
            let _ = c.rollback();
            if e.kind == ErrKind::Constraint {
                return mailbox_key(c);
            }
            Err(e)
        }
    }
}

// ------------------------------------------------------------------ transaction context

struct Tx<'a> {
    c: &'a mut dyn SharedConn,
    op: &'a Op,
    now: String,
    mailbox: String,
    conflicts: Vec<ConflictInfo>,
    activity: Vec<ActivityRec>,
    /// tickets changed by this op (their version is bumped once and a change-feed row written)
    changed: Vec<String>,
    feed: Vec<(Option<String>, String, Option<String>)>,
}

impl Tx<'_> {
    /// Record an activity row for the op's ticket (written in `finish`, with the final ticket id).
    fn act(&mut self, _ticket_id: &str, action: &str, field: Option<&str>, old: Option<String>, new: Option<String>) {
        self.activity.push(ActivityRec {
            activity_id: new_id(),
            occurred_at: self.now.clone(),
            source: "staff".into(),
            actor_staff_id: self.op.staff_id.clone(),
            actor_name: Some(self.op.staff_name.clone()).filter(|s| !s.is_empty()),
            device_id: Some(self.op.device_id.clone()),
            op_id: Some(self.op.op_id.clone()),
            action: action.into(),
            field_name: field.map(String::from),
            old_value: old,
            new_value: new,
        });
    }

    fn conflict(&mut self, field: &str, kind: &str, original: Option<String>, proposed: Option<String>, shared: Option<String>, reason: impl Into<String>) {
        self.conflicts.push(ConflictInfo {
            conflict_id: new_id(),
            field_name: field.into(),
            conflict_type: kind.into(),
            original,
            proposed,
            shared,
            reason: reason.into(),
        });
    }

    fn touch(&mut self, ticket_id: &str) {
        if !self.changed.iter().any(|t| t == ticket_id) {
            self.changed.push(ticket_id.to_string());
        }
    }

    fn exec(&mut self, sql: &str, p: &[Val]) -> SResult<usize> {
        self.c.exec(sql, p)
    }
    fn query(&mut self, sql: &str, p: &[Val]) -> SResult<Vec<Row>> {
        self.c.query(sql, p)
    }
    fn one(&mut self, sql: &str, p: &[Val]) -> SResult<Option<Row>> {
        self.c.query_one(sql, p)
    }
}

const CORE_COLS: &str = "ticket_id, ticket_ref, subject, sender_name, sender_email, school_name, njiis_id, priority, ticket_status, owner_staff_id, review_reason, staff_actions, first_received_at, last_incoming_at, done_at, merged_into, row_version, created_at, updated_at, origin";

fn map_core(r: &Row) -> TicketCore {
    TicketCore {
        ticket_id: col_s(r, 0),
        ticket_ref: col_s(r, 1),
        subject: col(r, 2),
        sender_name: col(r, 3),
        sender_email: col(r, 4),
        school_name: col(r, 5),
        njiis_id: col(r, 6),
        priority: col(r, 7),
        status: col_s(r, 8),
        owner_staff_id: col(r, 9),
        review_reason: col(r, 10),
        staff_actions: col(r, 11),
        first_received_at: col(r, 12),
        last_incoming_at: col(r, 13),
        done_at: col(r, 14),
        merged_into: col(r, 15),
        version: col_i(r, 16),
        created_at: col_s(r, 17),
        updated_at: col_s(r, 18),
        origin: col(r, 19),
    }
}

pub fn get_core(c: &mut dyn SharedConn, ticket_id: &str) -> SResult<Option<TicketCore>> {
    Ok(c.query_one(&format!("SELECT {CORE_COLS} FROM Tickets WHERE ticket_id = ?"), &[ticket_id.into()])?.map(|r| map_core(&r)))
}

/// Follow duplicate/merge redirects to the case that exists now.
pub fn resolve_ticket(c: &mut dyn SharedConn, id: &str) -> SResult<Option<String>> {
    let mut cur = id.to_string();
    for _ in 0..8 {
        if let Some(t) = get_core(c, &cur)? {
            match t.merged_into {
                Some(m) if !m.is_empty() => cur = m,
                _ => return Ok(Some(cur)),
            }
            continue;
        }
        match c.query_one("SELECT ticket_id FROM TicketAliases WHERE alias_key = ?", &[cur.clone().into()])? {
            Some(r) => cur = col_s(&r, 0),
            None => return Ok(None),
        }
    }
    Ok(None)
}

// ------------------------------------------------------------------ apply

/// Apply one operation in its own transaction. Constraint races (two computers inserting the same email
/// at once) are retried; the second attempt sees the other computer's row and reconciles to it.
pub fn apply(c: &mut dyn SharedConn, op: &Op) -> SResult<OpResult> {
    let mut last: Option<SharedError> = None;
    for _ in 0..3 {
        c.begin()?;
        match apply_tx(c, op) {
            Ok(res) => {
                c.commit()?;
                return Ok(res);
            }
            Err(e) => {
                let _ = c.rollback();
                if e.kind != ErrKind::Constraint {
                    return Err(e);
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| SharedError::new(ErrKind::Other, "operation could not be applied")))
}

fn apply_tx(c: &mut dyn SharedConn, op: &Op) -> SResult<OpResult> {
    if op.op_id.is_empty() {
        return Err(SharedError::new(ErrKind::Other, "operation without op_id"));
    }
    if let Some(r) = c.query_one("SELECT result_json FROM ProcessedOperations WHERE op_id = ?", &[op.op_id.clone().into()])? {
        let mut res: OpResult = serde_json::from_str(&col_s(&r, 0)).unwrap_or_default();
        res.duplicate = true;
        return Ok(res);
    }
    if let Some(r) = c.query_one("SELECT meta_value FROM DatabaseMetadata WHERE meta_key = ?", &["maintenance".into()])? {
        if col_s(&r, 0) == "1" {
            return Err(SharedError::new(ErrKind::Maintenance, "maintenance mode"));
        }
    }
    let mailbox = mailbox_key(c)?;
    let mut tx = Tx { c, op, now: crate::db::now(), mailbox, conflicts: vec![], activity: vec![], changed: vec![], feed: vec![] };
    let mut res = OpResult { op_id: op.op_id.clone(), result: "accepted".into(), ..Default::default() };

    // ticket ops: resolve redirects first (an offline duplicate may have been joined to another case)
    let ticket = match &op.ticket_id {
        Some(t) if op.op_type != "ticket.create" => match resolve_ticket(tx.c, t)? {
            Some(r) => {
                if &r != t {
                    res.redirected_from = Some(t.clone());
                }
                Some(r)
            }
            None => {
                res.result = "rejected".into();
                res.message = "The case does not exist in the shared database (it may not have been imported yet).".into();
                return finish(tx, res);
            }
        },
        None if !matches!(op.op_type.as_str(), "ticket.create" | "staff.upsert" | "conflict.resolve") => {
            res.result = "rejected".into();
            res.message = "The operation does not name a case.".into();
            return finish(tx, res);
        }
        other => other.clone(),
    };

    match op.op_type.as_str() {
        "ticket.create" => op_create(&mut tx, &mut res)?,
        "ticket.update" => op_update(&mut tx, ticket.as_deref().unwrap_or_default(), &mut res)?,
        "field.set" => op_field(&mut tx, ticket.as_deref().unwrap_or_default())?,
        "issues.set" => op_issues(&mut tx, ticket.as_deref().unwrap_or_default())?,
        "school.add" | "school.remove" => op_school(&mut tx, ticket.as_deref().unwrap_or_default())?,
        "note.add" => op_note_add(&mut tx, ticket.as_deref().unwrap_or_default())?,
        "note.edit" | "note.delete" => op_note_change(&mut tx, ticket.as_deref().unwrap_or_default())?,
        "email.attach" => op_email_attach(&mut tx, ticket.as_deref().unwrap_or_default(), &mut res)?,
        "staff.upsert" => op_staff(&mut tx, &mut res)?,
        "conflict.resolve" => op_conflict_resolve(&mut tx)?,
        other => {
            if !crate::shared_workflow::apply_workflow(&mut WorkflowTx::new(&mut tx), other, ticket.as_deref().unwrap_or_default(), &mut res)? {
                res.result = "rejected".into();
                res.message = format!("Unknown operation type {other}");
            }
        }
    }
    if res.ticket_id.is_none() {
        res.ticket_id = ticket.clone().or_else(|| op.ticket_id.clone());
    }
    finish(tx, res)
}

fn finish(mut tx: Tx<'_>, mut res: OpResult) -> SResult<OpResult> {
    if !tx.conflicts.is_empty() && res.result == "accepted" {
        res.result = "conflict".into();
    }
    // bump versions once per changed ticket and record change-feed rows
    for t in tx.changed.clone() {
        tx.exec("UPDATE Tickets SET row_version = row_version + 1, updated_at = ? WHERE ticket_id = ?", &[tx.now.clone().into(), t.clone().into()])?;
        let v = tx.one("SELECT row_version, ticket_ref FROM Tickets WHERE ticket_id = ?", &[t.clone().into()])?;
        let (ver, tref) = v.map(|r| (col_i(&r, 0), col(&r, 1))).unwrap_or((0, None));
        tx.feed.push((Some(t.clone()), "ticket".into(), Some(t.clone())));
        if res.ticket_id.as_deref() == Some(t.as_str()) || res.ticket_id.is_none() {
            res.ticket_version = ver;
            res.ticket_ref = tref;
        }
    }
    if res.ticket_version == 0 {
        if let Some(t) = res.ticket_id.clone() {
            if let Some(core) = get_core(tx.c, &t)? {
                res.ticket_version = core.version;
                res.ticket_ref = Some(core.ticket_ref);
            }
        }
    }
    let ticket_for_rows = res.ticket_id.clone();
    for a in tx.activity.clone() {
        let Some(tid) = ticket_for_rows.clone() else { break };
        tx.exec(
            "INSERT INTO ActivityLog (activity_id, ticket_id, occurred_at, activity_source, actor_staff_id, actor_name, device_id, op_id, action_text, field_name, old_value, new_value) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[a.activity_id.into(), tid.into(), a.occurred_at.into(), a.source.into(), txt(&a.actor_staff_id), cut_opt(&a.actor_name, 100), txt(&a.device_id), txt(&a.op_id), cut(&a.action, 255).into(), cut_opt(&a.field_name, 64), txt(&a.old_value), txt(&a.new_value)],
        )?;
    }
    let conflicts = tx.conflicts.clone();
    for cf in &conflicts {
        tx.exec(
            "INSERT INTO SyncConflicts (conflict_id, op_id, ticket_id, device_id, staff_id, staff_name, field_name, conflict_type, original_value, proposed_value, shared_value, reason, conflict_status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', ?)",
            &[cf.conflict_id.clone().into(), tx.op.op_id.clone().into(), txt(&ticket_for_rows), tx.op.device_id.clone().into(), txt(&tx.op.staff_id), cut(&tx.op.staff_name, 100).into(),
              cut(&cf.field_name, 64).into(), cut(&cf.conflict_type, 32).into(), txt(&cf.original), txt(&cf.proposed), txt(&cf.shared), cut(&cf.reason, 255).into(), tx.now.clone().into()],
        )?;
    }
    res.conflicts = conflicts;
    for (tid, kind, entity) in tx.feed.clone() {
        let ver = match &tid {
            Some(t) => get_core(tx.c, t)?.map(|c| c.version),
            None => None,
        };
        tx.exec(
            "INSERT INTO ChangeFeed (ticket_id, entity_type, entity_id, ticket_version, op_id, recorded_at) VALUES (?, ?, ?, ?, ?, ?)",
            &[txt(&tid), kind.into(), cut_opt(&entity, 64), ver.map(Val::Int).unwrap_or(Val::Null), tx.op.op_id.clone().into(), tx.now.clone().into()],
        )?;
    }
    let json = serde_json::to_string(&res).unwrap_or_default();
    tx.exec(
        "INSERT INTO ProcessedOperations (op_id, device_id, staff_id, op_type, ticket_id, processed_at, op_result, result_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        &[tx.op.op_id.clone().into(), tx.op.device_id.clone().into(), txt(&tx.op.staff_id), cut(&tx.op.op_type, 32).into(), txt(&res.ticket_id), tx.now.clone().into(), res.result.clone().into(), json.into()],
    )?;
    Ok(res)
}

// ------------------------------------------------------------------ emails

fn email_key(tx: &Tx<'_>, e: &EmailRec) -> (String, String, Option<String>) {
    let imid = normalize_message_id(&e.internet_message_id);
    if !imid.is_empty() && !tx.mailbox.is_empty() {
        (message_match_key(&tx.mailbox, &imid), "message_id".into(), None)
    } else if !imid.is_empty() {
        // no shared mailbox identity yet: still deterministic per Message-ID
        (message_match_key("", &imid), "message_id".into(), None)
    } else {
        // no Message-ID: unique key + evidence used only for "possible duplicate" review
        let ev = fallback_evidence_key(&tx.mailbox, &e.sender_email, &e.received_at, &e.subject);
        (format!("fb:{}", new_id()), "fallback".into(), Some(ev))
    }
}

/// Find the shared email for this record (by Message-ID match key), or insert it. Returns
/// (email_id, existed). Fallback-identified emails are always inserted (never merged); equal evidence
/// creates a possible-duplicate relation for review instead.
fn find_or_insert_email(tx: &mut Tx<'_>, e: &EmailRec, ticket_for_relation: &str) -> SResult<(String, bool)> {
    let (key, method, evidence) = email_key(tx, e);
    if method == "message_id" {
        if let Some(r) = tx.one("SELECT email_id FROM Emails WHERE match_key = ?", &[key.clone().into()])? {
            return Ok((col_s(&r, 0), true));
        }
    }
    let id = if e.email_id.is_empty() { new_id() } else { e.email_id.clone() };
    // an email id chosen by the client must not collide with an existing row
    let id = if tx.one("SELECT email_id FROM Emails WHERE email_id = ?", &[id.clone().into()])?.is_some() { new_id() } else { id };
    tx.exec(
        "INSERT INTO Emails (email_id, match_key, match_method, evidence_key, mailbox_id, internet_message_id, conversation_id, subject, sender_name, sender_email, received_at, is_internal, is_auto_reply, body_text, body_truncated, created_at, created_by_device) VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            id.clone().into(), key.into(), method.clone().into(), txt(&evidence),
            cut(&normalize_message_id(&e.internet_message_id), 255).into(), cut(&e.conversation_id, 255).into(), cut(&e.subject, 255).into(),
            cut(&e.sender_name, 255).into(), cut(&e.sender_email.to_lowercase(), 254).into(), cut(&e.received_at, 30).into(),
            e.is_internal.into(), e.is_auto_reply.into(), cut(&e.body_text, 60_000).into(), (e.body_truncated || e.body_text.chars().count() > 60_000).into(),
            tx.now.clone().into(), tx.op.device_id.clone().into(),
        ],
    )?;
    if let Some(ev) = evidence {
        let others = tx.query(
            "SELECT te.ticket_id FROM Emails em INNER JOIN TicketEmails te ON te.email_id = em.email_id WHERE em.evidence_key = ? AND em.email_id <> ? AND te.is_active = 1",
            &[ev.into(), id.clone().into()],
        )?;
        for o in others {
            let other = col_s(&o, 0);
            if other != ticket_for_relation && !ticket_for_relation.is_empty() {
                tx.exec(
                    "INSERT INTO TicketRelations (relation_id, ticket_a, ticket_b, relation_type, evidence, relation_status, created_at, created_by_device) VALUES (?, ?, ?, 'possible_duplicate', ?, 'open', ?, ?)",
                    &[new_id().into(), ticket_for_relation.into(), other.into(), "Email without Message-ID: same sender, received time and subject".into(), tx.now.clone().into(), tx.op.device_id.clone().into()],
                )?;
            }
        }
    }
    Ok((id, false))
}

fn active_ticket_of_email(tx: &mut Tx<'_>, email_id: &str) -> SResult<Option<String>> {
    let rows = tx.query("SELECT ticket_id, link_type FROM TicketEmails WHERE email_id = ? AND is_active = 1", &[email_id.into()])?;
    Ok(rows.iter().find(|r| col_s(r, 1) == "original").or(rows.first()).map(|r| col_s(r, 0)))
}

fn link_email(tx: &mut Tx<'_>, ticket_id: &str, email_id: &str, link_type: &str) -> SResult<bool> {
    if tx.one("SELECT link_type FROM TicketEmails WHERE ticket_id = ? AND email_id = ?", &[ticket_id.into(), email_id.into()])?.is_some() {
        return Ok(false);
    }
    tx.exec(
        "INSERT INTO TicketEmails (ticket_id, email_id, link_type, is_active, linked_at, linked_by_device) VALUES (?, ?, ?, 1, ?, ?)",
        &[ticket_id.into(), email_id.into(), cut(if link_type.is_empty() { "reply" } else { link_type }, 16).into(), tx.now.clone().into(), tx.op.device_id.clone().into()],
    )?;
    Ok(true)
}

// ------------------------------------------------------------------ ticket.create (import / new case)

fn unique_ref(tx: &mut Tx<'_>, wanted: &str) -> SResult<String> {
    let base = if wanted.is_empty() { format!("ASR-{}", &new_id()[..8].to_uppercase()) } else { cut(wanted, 24) };
    let mut cand = base.clone();
    for n in 2..50 {
        let taken = tx.one("SELECT ticket_id FROM Tickets WHERE ticket_ref = ?", &[cand.clone().into()])?.is_some()
            || tx.one("SELECT ticket_id FROM TicketAliases WHERE alias_key = ?", &[cand.clone().into()])?.is_some();
        if !taken {
            return Ok(cand);
        }
        cand = format!("{base}-{n}");
    }
    Ok(format!("ASR-{}", &new_id()[..8].to_uppercase()))
}

fn op_create(tx: &mut Tx<'_>, res: &mut OpResult) -> SResult<()> {
    let snap: TicketSnapshot = serde_json::from_value(tx.op.proposed.clone()).map_err(|e| SharedError::new(ErrKind::Other, format!("bad ticket payload: {e}")))?;
    let tid = tx.op.ticket_id.clone().unwrap_or_else(new_id);
    // re-sent create of a case that already exists (or was redirected): merge into it
    if let Some(existing) = resolve_ticket(tx.c, &tid)? {
        merge_snapshot(tx, &existing, &snap)?;
        res.ticket_id = Some(existing.clone());
        if existing != tid {
            res.redirected_from = Some(tid);
        }
        return Ok(());
    }
    // duplicate import: an email of this case (by Message-ID) already belongs to a shared case
    let mut existing: Option<String> = None;
    let mut ordered: Vec<&EmailRec> = snap.emails.iter().collect();
    ordered.sort_by_key(|e| if e.link_type == "original" { 0 } else { 1 });
    for e in ordered {
        let (key, method, _) = email_key(tx, e);
        if method != "message_id" {
            continue;
        }
        if let Some(r) = tx.one("SELECT email_id FROM Emails WHERE match_key = ?", &[key.into()])? {
            if let Some(t) = active_ticket_of_email(tx, &col_s(&r, 0))? {
                existing = Some(t);
                break;
            }
        }
    }
    if let Some(ex) = existing {
        tx.exec("INSERT INTO TicketAliases (alias_key, ticket_id, alias_type, created_at) VALUES (?, ?, 'duplicate', ?)", &[tid.clone().into(), ex.clone().into(), tx.now.clone().into()])?;
        if !snap.core.ticket_ref.is_empty() && tx.one("SELECT ticket_id FROM Tickets WHERE ticket_ref = ?", &[snap.core.ticket_ref.clone().into()])?.is_none()
            && tx.one("SELECT ticket_id FROM TicketAliases WHERE alias_key = ?", &[snap.core.ticket_ref.clone().into()])?.is_none()
        {
            tx.exec("INSERT INTO TicketAliases (alias_key, ticket_id, alias_type, created_at) VALUES (?, ?, 'ref', ?)", &[cut(&snap.core.ticket_ref, 64).into(), ex.clone().into(), tx.now.clone().into()])?;
        }
        merge_snapshot(tx, &ex, &snap)?;
        tx.act(&ex, "Duplicate import joined this case (the same email was imported on another computer)", None, None, Some(tx.op.device_id.clone()));
        res.ticket_id = Some(ex);
        res.redirected_from = Some(tid);
        return Ok(());
    }

    let k = &snap.core;
    let tref = unique_ref(tx, &k.ticket_ref)?;
    let status = if k.status.is_empty() { "Unassigned".to_string() } else { k.status.clone() };
    tx.exec(
        &format!("INSERT INTO Tickets ({CORE_COLS}, created_by_device) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 1, ?, ?, ?, ?)"),
        &[
            tid.clone().into(), tref.clone().into(), cut_opt(&k.subject, 255), cut_opt(&k.sender_name, 255), cut_opt(&k.sender_email, 254),
            cut_opt(&k.school_name, 255), cut_opt(&k.njiis_id, 64), cut_opt(&k.priority, 16), cut(&status, 16).into(), cut_opt(&k.owner_staff_id, 36),
            cut_opt(&k.review_reason, 255), txt(&k.staff_actions), cut_opt(&k.first_received_at, 30), cut_opt(&k.last_incoming_at, 30), cut_opt(&k.done_at, 30),
            (if k.created_at.is_empty() { tx.now.clone() } else { cut(&k.created_at, 30) }).into(), tx.now.clone().into(), cut_opt(&k.origin, 16), tx.op.device_id.clone().into(),
        ],
    )?;
    if !k.ticket_ref.is_empty() && tref != k.ticket_ref {
        tx.act(&tid, "Reference changed: another case already used it", Some("ticket_ref"), Some(k.ticket_ref.clone()), Some(tref.clone()));
    }
    let mut email_ids: Vec<(String, String)> = vec![];
    for e in &snap.emails {
        let (eid, _) = find_or_insert_email(tx, e, &tid)?;
        // an email already belonging to another case stays there (re-association is an explicit action)
        match active_ticket_of_email(tx, &eid)? {
            Some(other) if other != tid => {
                tx.act(&tid, "Email already belongs to another case; not moved", None, Some(other), Some(e.subject.clone()));
            }
            _ => {
                link_email(tx, &tid, &eid, &e.link_type)?;
            }
        }
        email_ids.push((normalize_message_id(&e.internet_message_id), eid));
    }
    for (i, (it, origin)) in snap.issues.iter().enumerate() {
        tx.exec("INSERT INTO TicketIssueTypes (ticket_id, issue_type, sort_order, origin) VALUES (?, ?, ?, ?)", &[tid.clone().into(), cut(it, 64).into(), (i as i64).into(), cut(origin, 16).into()])?;
    }
    for (slot, removed) in &snap.schools {
        tx.exec("INSERT INTO TicketSchools (ticket_id, slot_no, is_removed) VALUES (?, ?, ?)", &[tid.clone().into(), (*slot).into(), (*removed).into()])?;
    }
    for f in &snap.fields {
        insert_field(tx, &tid, f)?;
    }
    let mut owner_open = false;
    for o in snap.ownership.iter().filter(|o| !o.entry_id.is_empty() && !o.staff_id.is_empty()) {
        if tx.c.query_one("SELECT entry_id FROM OwnershipHistory WHERE entry_id = ?", &[o.entry_id.clone().into()])?.is_some() {
            continue;
        }
        // only the period of the current owner may stay open
        let is_current = o.ended_at.is_none() && k.owner_staff_id.as_deref() == Some(o.staff_id.as_str()) && !owner_open;
        let ended = if o.ended_at.is_none() && !is_current { Some(tx.now.clone()) } else { o.ended_at.clone() };
        owner_open |= is_current;
        tx.exec(
            "INSERT INTO OwnershipHistory (entry_id, ticket_id, staff_id, started_at, ended_at, started_by, ended_by, start_reason, end_reason, op_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                cut(&o.entry_id, 36).into(), tid.clone().into(), cut(&o.staff_id, 36).into(), cut(&o.started_at, 30).into(), cut_opt(&ended, 30),
                cut_opt(&o.started_by, 100), cut_opt(&o.ended_by, 100), cut_opt(&o.start_reason, 255), cut_opt(&o.end_reason, 255), tx.op.op_id.clone().into(),
            ],
        )?;
    }
    if let Some(owner) = k.owner_staff_id.clone().filter(|s| !s.is_empty() && !owner_open) {
        tx.exec(
            "INSERT INTO OwnershipHistory (entry_id, ticket_id, staff_id, started_at, started_by, start_reason, op_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &[new_id().into(), tid.clone().into(), owner.into(), tx.now.clone().into(), cut(&tx.op.staff_name, 100).into(), "import".into(), tx.op.op_id.clone().into()],
        )?;
    }
    if let Some(cmp) = &snap.completion {
        // covered emails arrive as Message-IDs (the client does not know shared ids yet)
        let covered: Vec<String> = cmp.covered.iter().filter_map(|m| email_ids.iter().find(|(im, _)| im == &normalize_message_id(m)).map(|(_, id)| id.clone())).collect();
        tx.exec(
            "INSERT INTO CompletionHistory (completion_id, ticket_id, actor_staff_id, actor_name, decided_at, accepted_at, covered_email_ids, max_covered_received_at, op_id, completion_state, state_reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                (if cmp.completion_id.is_empty() { new_id() } else { cmp.completion_id.clone() }).into(), tid.clone().into(), txt(&cmp.actor_staff_id), cut(&cmp.actor_name, 100).into(),
                cut(&cmp.decided_at, 30).into(), tx.now.clone().into(), serde_json::to_string(&covered).unwrap_or_default().into(), cut_opt(&cmp.max_covered_received_at, 30),
                tx.op.op_id.clone().into(), cut(if cmp.state.is_empty() { "accepted" } else { &cmp.state }, 16).into(), cut_opt(&cmp.state_reason, 255),
            ],
        )?;
    }
    let what = match k.origin.as_deref() {
        Some("outlook") => "Case created from Outlook email",
        Some("migration") => "Case imported from a local database",
        _ => "Case created",
    };
    tx.act(&tid, what, None, None, Some(tref.clone()));
    tx.feed.push((Some(tid.clone()), "ticket".into(), Some(tid.clone())));
    res.ticket_id = Some(tid);
    res.ticket_ref = Some(tref);
    Ok(())
}

fn insert_field(tx: &mut Tx<'_>, tid: &str, f: &FieldRec) -> SResult<()> {
    tx.exec(
        "INSERT INTO TicketFields (ticket_id, field_key, extracted_value, manual_value, has_manual, is_verified, row_version, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
        &[tid.into(), cut(&f.key, 64).into(), txt(&f.extracted), txt(&f.manual), f.has_manual.into(), f.verified.into(), tx.now.clone().into(), cut(&tx.op.staff_name, 100).into()],
    )?;
    Ok(())
}

fn get_field(tx: &mut Tx<'_>, tid: &str, key: &str) -> SResult<Option<FieldRec>> {
    Ok(tx
        .one("SELECT field_key, extracted_value, manual_value, has_manual, is_verified, row_version, updated_by FROM TicketFields WHERE ticket_id = ? AND field_key = ?", &[tid.into(), key.into()])?
        .map(|r| FieldRec { key: col_s(&r, 0), extracted: col(&r, 1), manual: col(&r, 2), has_manual: col_i(&r, 3) != 0, verified: col_i(&r, 4) != 0, version: col_i(&r, 5), updated_by: col(&r, 6) }))
}

/// Join a duplicate import into an existing case without losing either side's information.
fn merge_snapshot(tx: &mut Tx<'_>, tid: &str, snap: &TicketSnapshot) -> SResult<()> {
    let cur = get_core(tx.c, tid)?.ok_or_else(|| SharedError::new(ErrKind::Other, "case vanished during merge"))?;
    let mut changed = false;
    for e in &snap.emails {
        let (eid, _) = find_or_insert_email(tx, e, tid)?;
        if active_ticket_of_email(tx, &eid)?.is_none() && link_email(tx, tid, &eid, &e.link_type)? {
            changed = true;
        }
    }
    let manual_issues = tx.query("SELECT issue_type FROM TicketIssueTypes WHERE ticket_id = ? AND origin = 'manual'", &[tid.into()])?.len();
    let existing_issues: Vec<String> = tx.query("SELECT issue_type FROM TicketIssueTypes WHERE ticket_id = ?", &[tid.into()])?.iter().map(|r| col_s(r, 0)).collect();
    if manual_issues == 0 {
        for (i, (it, origin)) in snap.issues.iter().enumerate() {
            if !existing_issues.contains(it) && it != "other" {
                tx.exec("INSERT INTO TicketIssueTypes (ticket_id, issue_type, sort_order, origin) VALUES (?, ?, ?, ?)", &[tid.into(), cut(it, 64).into(), (100 + i as i64).into(), cut(origin, 16).into()])?;
                changed = true;
            }
        }
    }
    for (slot, removed) in &snap.schools {
        if tx.one("SELECT slot_no FROM TicketSchools WHERE ticket_id = ? AND slot_no = ?", &[tid.into(), (*slot).into()])?.is_none() {
            tx.exec("INSERT INTO TicketSchools (ticket_id, slot_no, is_removed) VALUES (?, ?, ?)", &[tid.into(), (*slot).into(), (*removed).into()])?;
            changed = true;
        }
    }
    for f in &snap.fields {
        match get_field(tx, tid, &f.key)? {
            None => {
                insert_field(tx, tid, f)?;
                changed = true;
            }
            Some(ex) => {
                if ex.extracted.as_deref().unwrap_or("").trim().is_empty() && f.extracted.as_deref().unwrap_or("").trim() != "" {
                    tx.exec("UPDATE TicketFields SET extracted_value = ? WHERE ticket_id = ? AND field_key = ?", &[txt(&f.extracted), tid.into(), f.key.clone().into()])?;
                    changed = true;
                }
                if f.has_manual && !ex.has_manual {
                    tx.exec(
                        "UPDATE TicketFields SET manual_value = ?, has_manual = 1, is_verified = ?, row_version = row_version + 1, updated_at = ?, updated_by = ? WHERE ticket_id = ? AND field_key = ?",
                        &[txt(&f.manual), f.verified.into(), tx.now.clone().into(), cut(&tx.op.staff_name, 100).into(), tid.into(), f.key.clone().into()],
                    )?;
                    changed = true;
                } else if f.has_manual && ex.has_manual && !opt_eq(&f.manual, &ex.manual) {
                    tx.conflict(&format!("field:{}", f.key), "field", None, f.manual.clone(), ex.manual.clone(), "The same email was imported on two computers with different staff-entered values.");
                }
            }
        }
    }
    let theirs = snap.core.staff_actions.clone().filter(|s| !s.trim().is_empty());
    if let Some(th) = theirs {
        match cur.staff_actions.clone().filter(|s| !s.trim().is_empty()) {
            None => {
                tx.exec("UPDATE Tickets SET staff_actions = ? WHERE ticket_id = ?", &[th.into(), tid.into()])?;
                changed = true;
            }
            Some(mine) if mine != th => tx.conflict("staff_actions", "text", None, Some(th), Some(mine), "Different staff actions were recorded for the same email on two computers."),
            _ => {}
        }
    }
    // ownership from the duplicate: adopted only when nobody owns the case; otherwise a decision is needed
    if let Some(owner) = snap.core.owner_staff_id.clone().filter(|s| !s.is_empty()) {
        match cur.owner_staff_id.clone().filter(|s| !s.is_empty()) {
            None => {
                let st = if cur.status == "Unassigned" { "Assigned".to_string() } else { cur.status.clone() };
                tx.exec("UPDATE Tickets SET owner_staff_id = ?, ticket_status = ? WHERE ticket_id = ?", &[owner.clone().into(), st.into(), tid.into()])?;
                tx.exec(
                    "INSERT INTO OwnershipHistory (entry_id, ticket_id, staff_id, started_at, started_by, start_reason, op_id) VALUES (?, ?, ?, ?, ?, 'duplicate_import', ?)",
                    &[new_id().into(), tid.into(), owner.clone().into(), tx.now.clone().into(), cut(&tx.op.staff_name, 100).into(), tx.op.op_id.clone().into()],
                )?;
                tx.act(tid, "Owner set from duplicate import", Some("owner"), None, Some(owner));
                changed = true;
            }
            Some(o) if o != owner => tx.conflict("owner", "owner", None, Some(owner), Some(o), "Two computers recorded different owners for the same email. Choose who owns the case."),
            _ => {}
        }
    }
    if !snap.core.status.is_empty() && snap.core.status != cur.status && snap.core.status != "Unassigned" && cur.status != "Unassigned" {
        tx.conflict("status", "status", None, Some(snap.core.status.clone()), Some(cur.status.clone()), "Two computers recorded different statuses for the same email.");
    }
    if changed {
        tx.touch(tid);
    }
    Ok(())
}

// ------------------------------------------------------------------ edits

const UPDATABLE: &[(&str, &str, usize)] = &[
    ("priority", "priority", 16),
    ("sender_name", "sender_name", 255),
    ("sender_email", "sender_email", 254),
    ("staff_actions", "staff_actions", 0),
    ("subject", "subject", 255),
];

fn core_value(c: &TicketCore, key: &str) -> Option<String> {
    match key {
        "priority" => c.priority.clone(),
        "sender_name" => c.sender_name.clone(),
        "sender_email" => c.sender_email.clone(),
        "staff_actions" => c.staff_actions.clone(),
        "subject" => c.subject.clone(),
        _ => None,
    }
}

/// Field-by-field merge: a key is applied when the shared value still equals what the user saw;
/// a key already equal to the proposal is a no-op; anything else is a conflict that keeps both values.
fn op_update(tx: &mut Tx<'_>, tid: &str, _res: &mut OpResult) -> SResult<()> {
    let cur = get_core(tx.c, tid)?.ok_or_else(|| SharedError::new(ErrKind::Other, "case missing"))?;
    let Value::Object(prop) = tx.op.proposed.clone() else { return Ok(()) };
    for (key, pv) in prop {
        let Some((_, colname, width)) = UPDATABLE.iter().find(|(k, _, _)| *k == key) else { continue };
        let proposed = jstr(&pv).filter(|s| !s.is_empty());
        let original = jstr(tx.op.original.get(&key).unwrap_or(&Value::Null)).filter(|s| !s.is_empty());
        let shared = core_value(&cur, &key).filter(|s| !s.is_empty());
        if opt_eq(&shared, &proposed) {
            continue;
        }
        if opt_eq(&shared, &original) {
            let v = match &proposed {
                Some(p) if *width > 0 => Val::Text(cut(p, *width)),
                Some(p) => Val::Text(p.clone()),
                None => Val::Null,
            };
            tx.exec(&format!("UPDATE Tickets SET {colname} = ? WHERE ticket_id = ?"), &[v, tid.into()])?;
            tx.act(tid, "Case updated", Some(&key), shared, proposed);
            tx.touch(tid);
        } else {
            let kind = if key == "staff_actions" { "text" } else { "field" };
            tx.conflict(&key, kind, original, proposed, shared, "Someone else changed this value after you last saw it.");
        }
    }
    Ok(())
}

fn op_field(tx: &mut Tx<'_>, tid: &str) -> SResult<()> {
    let key = tx.op.entity_id.clone().unwrap_or_default();
    if key.is_empty() {
        return Ok(());
    }
    let want = |v: &Value| (jstr(v.get("manual").unwrap_or(&Value::Null)), v.get("has_manual").and_then(|b| b.as_bool()).unwrap_or(false), v.get("verified").and_then(|b| b.as_bool()).unwrap_or(false));
    let (p_manual, p_has, p_ver) = want(&tx.op.proposed);
    let (o_manual, o_has, o_ver) = want(&tx.op.original);
    let p_extracted = jstr(tx.op.proposed.get("extracted").unwrap_or(&Value::Null));
    let cur = get_field(tx, tid, &key)?;
    let (c_manual, c_has, c_ver) = cur.as_ref().map(|f| (f.manual.clone(), f.has_manual, f.verified)).unwrap_or((None, false, false));
    let same = |a: &(Option<String>, bool, bool), b: &(Option<String>, bool, bool)| a.1 == b.1 && a.2 == b.2 && (!a.1 || opt_eq(&a.0, &b.0));
    let cur_t = (c_manual.clone(), c_has, c_ver);
    let prop_t = (p_manual.clone(), p_has, p_ver);
    if same(&cur_t, &prop_t) {
        return Ok(());
    }
    if !same(&cur_t, &(o_manual.clone(), o_has, o_ver)) {
        tx.conflict(&format!("field:{key}"), "field", if o_has { o_manual } else { None }, if p_has { p_manual } else { None }, if c_has { c_manual } else { None }, "Someone else changed this field after you last saw it.");
        return Ok(());
    }
    if cur.is_none() {
        insert_field(tx, tid, &FieldRec { key: key.clone(), extracted: p_extracted, manual: p_manual.clone(), has_manual: p_has, verified: p_ver, ..Default::default() })?;
    } else {
        tx.exec(
            "UPDATE TicketFields SET manual_value = ?, has_manual = ?, is_verified = ?, row_version = row_version + 1, updated_at = ?, updated_by = ? WHERE ticket_id = ? AND field_key = ?",
            &[if p_has { txt(&p_manual) } else { Val::Null }, p_has.into(), p_ver.into(), tx.now.clone().into(), cut(&tx.op.staff_name, 100).into(), tid.into(), key.clone().into()],
        )?;
    }
    // keep the case summary columns in step with the first school group
    if key == "school_name" || key == "school_njiis_id" {
        let colname = if key == "school_name" { "school_name" } else { "njiis_id" };
        let eff = if p_has { p_manual.clone() } else { get_field(tx, tid, &key)?.and_then(|f| f.extracted) };
        tx.exec(&format!("UPDATE Tickets SET {colname} = ? WHERE ticket_id = ?"), &[cut_opt(&eff, if colname == "njiis_id" { 64 } else { 255 }), tid.into()])?;
    }
    let action = if p_ver { "Field verified" } else if !p_has { "Field reverted to extracted value" } else { "Field updated" };
    tx.act(tid, action, Some(&key), if c_has { c_manual } else { None }, if p_has { p_manual } else { None });
    tx.touch(tid);
    Ok(())
}

fn issue_list(tx: &mut Tx<'_>, tid: &str) -> SResult<Vec<String>> {
    Ok(tx.query("SELECT issue_type, sort_order FROM TicketIssueTypes WHERE ticket_id = ? ORDER BY sort_order, issue_type", &[tid.into()])?.iter().map(|r| col_s(r, 0)).collect())
}

fn op_issues(tx: &mut Tx<'_>, tid: &str) -> SResult<()> {
    let list = |v: &Value| -> Vec<String> { v.get("list").and_then(|l| l.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
    let prop = list(&tx.op.proposed);
    let orig = list(&tx.op.original);
    let cur = issue_list(tx, tid)?;
    if cur == prop || prop.is_empty() {
        return Ok(());
    }
    if cur != orig {
        tx.conflict("issues", "field", Some(orig.join(", ")), Some(prop.join(", ")), Some(cur.join(", ")), "Someone else changed the issue types after you last saw them.");
        return Ok(());
    }
    tx.exec("DELETE FROM TicketIssueTypes WHERE ticket_id = ?", &[tid.into()])?;
    for (i, it) in prop.iter().enumerate() {
        tx.exec("INSERT INTO TicketIssueTypes (ticket_id, issue_type, sort_order, origin) VALUES (?, ?, ?, 'manual')", &[tid.into(), cut(it, 64).into(), (i as i64).into()])?;
    }
    tx.act(tid, "Issue types set", Some("issue_type"), Some(cur.join(", ")), Some(prop.join(", ")));
    tx.touch(tid);
    Ok(())
}

fn op_school(tx: &mut Tx<'_>, tid: &str) -> SResult<()> {
    let slot: i64 = tx.op.entity_id.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
    if slot <= 0 {
        return Ok(());
    }
    let removed = tx.op.op_type == "school.remove";
    match tx.one("SELECT is_removed FROM TicketSchools WHERE ticket_id = ? AND slot_no = ?", &[tid.into(), slot.into()])? {
        Some(r) if (col_i(&r, 0) != 0) == removed => return Ok(()),
        Some(_) => {
            tx.exec("UPDATE TicketSchools SET is_removed = ? WHERE ticket_id = ? AND slot_no = ?", &[removed.into(), tid.into(), slot.into()])?;
        }
        None => {
            tx.exec("INSERT INTO TicketSchools (ticket_id, slot_no, is_removed) VALUES (?, ?, ?)", &[tid.into(), slot.into(), removed.into()])?;
        }
    }
    tx.act(tid, if removed { "School removed" } else { "School added" }, None, None, Some(format!("School {slot}")));
    tx.touch(tid);
    Ok(())
}

// ------------------------------------------------------------------ notes

fn op_note_add(tx: &mut Tx<'_>, tid: &str) -> SResult<()> {
    let n: NoteRec = serde_json::from_value(tx.op.proposed.clone()).unwrap_or_default();
    if n.note_id.is_empty() {
        return Ok(());
    }
    if tx.one("SELECT note_id FROM TicketNotes WHERE note_id = ?", &[n.note_id.clone().into()])?.is_some() {
        return Ok(());
    }
    tx.exec(
        "INSERT INTO TicketNotes (note_id, ticket_id, author_staff_id, author_name, note_text, row_version, is_deleted, created_at, updated_at, device_id) VALUES (?, ?, ?, ?, ?, 1, 0, ?, ?, ?)",
        &[n.note_id.clone().into(), tid.into(), txt(&n.author_staff_id), cut(&n.author_name, 100).into(), n.text.clone().into(),
          (if n.created_at.is_empty() { tx.now.clone() } else { cut(&n.created_at, 30) }).into(), tx.now.clone().into(), tx.op.device_id.clone().into()],
    )?;
    tx.act(tid, "Note added", Some("note"), None, None);
    tx.touch(tid);
    Ok(())
}

fn op_note_change(tx: &mut Tx<'_>, tid: &str) -> SResult<()> {
    let nid = tx.op.entity_id.clone().unwrap_or_default();
    let Some(r) = tx.one("SELECT note_text, is_deleted, row_version FROM TicketNotes WHERE note_id = ?", &[nid.clone().into()])? else {
        tx.conflict("note", "note", None, None, None, "The note no longer exists in the shared database.");
        return Ok(());
    };
    let (cur_text, deleted) = (col(&r, 0), col_i(&r, 1) != 0);
    let orig = jstr(tx.op.original.get("text").unwrap_or(&Value::Null));
    if tx.op.op_type == "note.delete" {
        if deleted {
            return Ok(());
        }
        if !opt_eq(&cur_text, &orig) {
            tx.conflict(&format!("note:{nid}"), "note", orig, None, cur_text, "The note was edited by someone else; it was not deleted.");
            return Ok(());
        }
        tx.exec("UPDATE TicketNotes SET is_deleted = 1, row_version = row_version + 1, updated_at = ? WHERE note_id = ?", &[tx.now.clone().into(), nid.into()])?;
        tx.act(tid, "Note deleted", Some("note"), None, None);
    } else {
        let prop = jstr(tx.op.proposed.get("text").unwrap_or(&Value::Null));
        if opt_eq(&cur_text, &prop) && !deleted {
            return Ok(());
        }
        if deleted || !opt_eq(&cur_text, &orig) {
            tx.conflict(&format!("note:{nid}"), "note", orig, prop, if deleted { None } else { cur_text }, if deleted { "The note was deleted by someone else." } else { "Someone else edited this note after you last saw it." });
            return Ok(());
        }
        tx.exec("UPDATE TicketNotes SET note_text = ?, row_version = row_version + 1, updated_at = ? WHERE note_id = ?", &[txt(&prop), tx.now.clone().into(), nid.into()])?;
        tx.act(tid, "Note edited", Some("note"), None, None);
    }
    tx.touch(tid);
    Ok(())
}

// ------------------------------------------------------------------ emails attached later (replies)

fn op_email_attach(tx: &mut Tx<'_>, tid: &str, res: &mut OpResult) -> SResult<()> {
    let e: EmailRec = serde_json::from_value(tx.op.proposed.clone()).unwrap_or_default();
    let (eid, _) = find_or_insert_email(tx, &e, tid)?;
    match active_ticket_of_email(tx, &eid)? {
        Some(t) if t == tid => return Ok(()),
        Some(other) => {
            // never re-associated implicitly; the client keeps showing it and staff can move it later
            res.extra = json!({ "email_id": eid, "belongs_to": other });
            res.message = "This email already belongs to another case.".into();
            return Ok(());
        }
        None => {}
    }
    link_email(tx, tid, &eid, &e.link_type)?;
    res.extra = json!({ "email_id": eid });
    if !e.is_internal {
        let cur = get_core(tx.c, tid)?.unwrap_or_default();
        if cur.last_incoming_at.as_deref().unwrap_or("") < e.received_at.as_str() {
            tx.exec("UPDATE Tickets SET last_incoming_at = ? WHERE ticket_id = ?", &[cut(&e.received_at, 30).into(), tid.into()])?;
        }
    }
    tx.act(tid, if e.is_internal { "Staff email linked" } else { "Email added to case" }, None, None, Some(cut(&e.subject, 200)));
    crate::shared_workflow::after_email_attached(&mut WorkflowTx::new(tx), tid, &eid, &e)?;
    tx.touch(tid);
    Ok(())
}

// ------------------------------------------------------------------ staff

fn op_staff(tx: &mut Tx<'_>, res: &mut OpResult) -> SResult<()> {
    let p: StaffRec = serde_json::from_value(tx.op.proposed.clone()).unwrap_or_default();
    let o: StaffRec = serde_json::from_value(tx.op.original.clone()).unwrap_or_default();
    let mut sid = tx.op.entity_id.clone().unwrap_or_else(|| p.staff_id.clone());
    let mut row = tx.one("SELECT staff_id, display_name, email, windows_account, is_active FROM Staff WHERE staff_id = ?", &[sid.clone().into()])?;
    if row.is_none() {
        // the same person set up separately on two computers: matched by email address
        if let Some(em) = p.email.clone().filter(|e| !e.trim().is_empty()) {
            for r in tx.query("SELECT staff_id, display_name, email, windows_account, is_active FROM Staff", &[])? {
                if col_s(&r, 2).eq_ignore_ascii_case(em.trim()) {
                    sid = col_s(&r, 0);
                    row = Some(r);
                    break;
                }
            }
        }
    }
    res.extra = json!({ "staff_id": sid });
    match row {
        None => {
            tx.exec(
                "INSERT INTO Staff (staff_id, display_name, email, windows_account, is_active, row_version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
                &[sid.clone().into(), cut(&p.display_name, 100).into(), cut_opt(&p.email, 254), cut_opt(&p.windows_account, 128), p.active.into(), tx.now.clone().into(), tx.now.clone().into()],
            )?;
        }
        Some(r) => {
            let cur = StaffRec { staff_id: sid.clone(), display_name: col_s(&r, 1), email: col(&r, 2), windows_account: col(&r, 3), active: col_i(&r, 4) != 0, version: 0 };
            let mut sets: Vec<(&str, Val)> = vec![];
            let mut check = |name: &'static str, c: Option<String>, orig: Option<String>, prop: Option<String>, width: usize, tx: &mut Tx<'_>| {
                if opt_eq(&c, &prop) {
                    return;
                }
                if opt_eq(&c, &orig) || orig.is_none() && tx.op.base_version == 0 {
                    sets.push((name, prop.map(|v| Val::Text(cut(&v, width))).unwrap_or(Val::Null)));
                } else {
                    tx.conflict(&format!("staff:{name}"), "field", orig, prop, c, "Someone else changed this staff setting.");
                }
            };
            check("display_name", Some(cur.display_name.clone()), Some(o.display_name.clone()).filter(|s| !s.is_empty()), Some(p.display_name.clone()), 100, tx);
            check("email", cur.email.clone(), o.email.clone(), p.email.clone(), 254, tx);
            check("windows_account", cur.windows_account.clone(), o.windows_account.clone(), p.windows_account.clone(), 128, tx);
            if cur.active != p.active {
                sets.push(("is_active", p.active.into()));
            }
            if !sets.is_empty() {
                let mut params: Vec<Val> = sets.iter().map(|(_, v)| v.clone()).collect();
                let assign = sets.iter().map(|(n, _)| format!("{n} = ?")).collect::<Vec<_>>().join(", ");
                params.push(tx.now.clone().into());
                params.push(sid.clone().into());
                tx.exec(&format!("UPDATE Staff SET {assign}, row_version = row_version + 1, updated_at = ? WHERE staff_id = ?"), &params)?;
            }
        }
    }
    tx.feed.push((None, "staff".into(), Some(sid)));
    Ok(())
}

fn op_conflict_resolve(tx: &mut Tx<'_>) -> SResult<()> {
    let cid = tx.op.entity_id.clone().unwrap_or_default();
    let resolution = tx.op.proposed.get("resolution").and_then(|v| v.as_str()).unwrap_or("accept_shared").to_string();
    tx.exec(
        "UPDATE SyncConflicts SET conflict_status = 'resolved', resolution_kind = ?, resolved_at = ?, resolved_by = ? WHERE conflict_id = ? AND conflict_status = 'open'",
        &[cut(&resolution, 32).into(), tx.now.clone().into(), cut(&tx.op.staff_name, 100).into(), cid.into()],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ workflow extension point

/// Narrow access to the transaction for workflow rules (Claim, Done, Review ...) kept in `shared_workflow`.
pub struct WorkflowTx<'a, 'b> {
    tx: &'a mut Tx<'b>,
}

impl<'a, 'b> WorkflowTx<'a, 'b> {
    fn new(tx: &'a mut Tx<'b>) -> Self {
        WorkflowTx { tx }
    }
    pub fn op(&self) -> &Op {
        self.tx.op
    }
    pub fn now(&self) -> String {
        self.tx.now.clone()
    }
    pub fn core(&mut self, tid: &str) -> SResult<TicketCore> {
        get_core(self.tx.c, tid)?.ok_or_else(|| SharedError::new(ErrKind::Other, "case missing"))
    }
    pub fn exec(&mut self, sql: &str, p: &[Val]) -> SResult<usize> {
        self.tx.exec(sql, p)
    }
    pub fn query(&mut self, sql: &str, p: &[Val]) -> SResult<Vec<Row>> {
        self.tx.query(sql, p)
    }
    pub fn act(&mut self, tid: &str, action: &str, field: Option<&str>, old: Option<String>, new: Option<String>) {
        self.tx.act(tid, action, field, old, new)
    }
    pub fn conflict(&mut self, field: &str, kind: &str, original: Option<String>, proposed: Option<String>, shared: Option<String>, reason: &str) {
        self.tx.conflict(field, kind, original, proposed, shared, reason.to_string())
    }
    pub fn touch(&mut self, tid: &str) {
        self.tx.touch(tid)
    }
}

// ------------------------------------------------------------------ reading shared state (pull)

#[derive(Debug, Clone, Default, Serialize)]
pub struct FeedRow {
    pub seq: i64,
    pub ticket_id: Option<String>,
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub ticket_version: i64,
}

/// Change-feed rows after `since` (callers pass an overlapping lower bound; AutoNumber order is not
/// commit order, so a periodic full version reconciliation backs this up).
pub fn feed_since(c: &mut dyn SharedConn, since: i64, max: i64) -> SResult<Vec<FeedRow>> {
    let sql = c.dialect().limit("SELECT seq, ticket_id, entity_type, entity_id, ticket_version FROM ChangeFeed WHERE seq > ? ORDER BY seq", max);
    Ok(c.query(&sql, &[since.into()])?
        .iter()
        .map(|r| FeedRow { seq: col_i(r, 0), ticket_id: col(r, 1), entity_type: col_s(r, 2), entity_id: col(r, 3), ticket_version: col_i(r, 4) })
        .collect())
}

pub fn max_feed_seq(c: &mut dyn SharedConn) -> SResult<i64> {
    Ok(c.query_one("SELECT MAX(seq) FROM ChangeFeed", &[])?.map(|r| col_i(&r, 0)).unwrap_or(0))
}

/// (ticket id, version) of every case: the full reconciliation pass.
pub fn ticket_versions(c: &mut dyn SharedConn) -> SResult<Vec<(String, i64)>> {
    Ok(c.query("SELECT ticket_id, row_version FROM Tickets", &[])?.iter().map(|r| (col_s(r, 0), col_i(r, 1))).collect())
}

pub fn staff_all(c: &mut dyn SharedConn) -> SResult<Vec<StaffRec>> {
    Ok(c.query("SELECT staff_id, display_name, email, windows_account, is_active, row_version FROM Staff", &[])?
        .iter()
        .map(|r| StaffRec { staff_id: col_s(r, 0), display_name: col_s(r, 1), email: col(r, 2), windows_account: col(r, 3), active: col_i(r, 4) != 0, version: col_i(r, 5) })
        .collect())
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// Full shared state of the given cases (chunked IN queries; a handful of round trips per chunk).
pub fn fetch_bundles(c: &mut dyn SharedConn, ids: &[String]) -> SResult<Vec<TicketBundle>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(40) {
        let ph = placeholders(chunk.len());
        let p: Vec<Val> = chunk.iter().map(|s| s.clone().into()).collect();
        let mut map: std::collections::HashMap<String, TicketBundle> = std::collections::HashMap::new();
        for r in c.query(&format!("SELECT {CORE_COLS} FROM Tickets WHERE ticket_id IN ({ph})"), &p)? {
            let core = map_core(&r);
            map.insert(core.ticket_id.clone(), TicketBundle { core, ..Default::default() });
        }
        for r in c.query(
            &format!("SELECT te.ticket_id, em.email_id, em.match_key, em.match_method, em.evidence_key, em.internet_message_id, em.conversation_id, em.subject, em.sender_name, em.sender_email, em.received_at, em.is_internal, em.is_auto_reply, em.body_text, em.body_truncated, te.link_type, te.is_active FROM TicketEmails te INNER JOIN Emails em ON em.email_id = te.email_id WHERE te.ticket_id IN ({ph})"),
            &p,
        )? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.emails.push(EmailRec {
                    email_id: col_s(&r, 1), match_key: col_s(&r, 2), match_method: col_s(&r, 3), evidence_key: col(&r, 4), internet_message_id: col_s(&r, 5), conversation_id: col_s(&r, 6),
                    subject: col_s(&r, 7), sender_name: col_s(&r, 8), sender_email: col_s(&r, 9), received_at: col_s(&r, 10), is_internal: col_i(&r, 11) != 0, is_auto_reply: col_i(&r, 12) != 0,
                    body_text: col_s(&r, 13), body_truncated: col_i(&r, 14) != 0, link_type: col_s(&r, 15), is_active: col_i(&r, 16) != 0,
                });
            }
        }
        for r in c.query(&format!("SELECT ticket_id, issue_type, origin, sort_order FROM TicketIssueTypes WHERE ticket_id IN ({ph}) ORDER BY sort_order, issue_type"), &p)? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.issues.push((col_s(&r, 1), col_s(&r, 2)));
            }
        }
        for r in c.query(&format!("SELECT ticket_id, slot_no, is_removed FROM TicketSchools WHERE ticket_id IN ({ph}) ORDER BY slot_no"), &p)? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.schools.push((col_i(&r, 1), col_i(&r, 2) != 0));
            }
        }
        for r in c.query(&format!("SELECT ticket_id, field_key, extracted_value, manual_value, has_manual, is_verified, row_version, updated_by FROM TicketFields WHERE ticket_id IN ({ph})"), &p)? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.fields.push(FieldRec { key: col_s(&r, 1), extracted: col(&r, 2), manual: col(&r, 3), has_manual: col_i(&r, 4) != 0, verified: col_i(&r, 5) != 0, version: col_i(&r, 6), updated_by: col(&r, 7) });
            }
        }
        for r in c.query(&format!("SELECT ticket_id, note_id, author_staff_id, author_name, note_text, row_version, is_deleted, created_at, updated_at FROM TicketNotes WHERE ticket_id IN ({ph})"), &p)? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.notes.push(NoteRec { note_id: col_s(&r, 1), author_staff_id: col(&r, 2), author_name: col_s(&r, 3), text: col_s(&r, 4), version: col_i(&r, 5), deleted: col_i(&r, 6) != 0, created_at: col_s(&r, 7), updated_at: col_s(&r, 8) });
            }
        }
        for r in c.query(&format!("SELECT ticket_id, entry_id, staff_id, started_at, ended_at, started_by, ended_by, start_reason, end_reason FROM OwnershipHistory WHERE ticket_id IN ({ph}) ORDER BY started_at"), &p)? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.ownership.push(OwnershipRec { entry_id: col_s(&r, 1), staff_id: col_s(&r, 2), started_at: col_s(&r, 3), ended_at: col(&r, 4), started_by: col(&r, 5), ended_by: col(&r, 6), start_reason: col(&r, 7), end_reason: col(&r, 8) });
            }
        }
        for r in c.query(&format!("SELECT ticket_id, completion_id, actor_staff_id, actor_name, decided_at, accepted_at, covered_email_ids, max_covered_received_at, completion_state, state_reason FROM CompletionHistory WHERE ticket_id IN ({ph}) ORDER BY decided_at"), &p)? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.completions.push(CompletionRec {
                    completion_id: col_s(&r, 1), actor_staff_id: col(&r, 2), actor_name: col_s(&r, 3), decided_at: col_s(&r, 4), accepted_at: col(&r, 5),
                    covered: serde_json::from_str(&col_s(&r, 6)).unwrap_or_default(), max_covered_received_at: col(&r, 7), state: col_s(&r, 8), state_reason: col(&r, 9),
                });
            }
        }
        for r in c.query(&format!("SELECT ticket_id, activity_id, occurred_at, activity_source, actor_staff_id, actor_name, device_id, op_id, action_text, field_name, old_value, new_value FROM ActivityLog WHERE ticket_id IN ({ph}) ORDER BY occurred_at"), &p)? {
            if let Some(b) = map.get_mut(&col_s(&r, 0)) {
                b.activity.push(ActivityRec {
                    activity_id: col_s(&r, 1), occurred_at: col_s(&r, 2), source: col_s(&r, 3), actor_staff_id: col(&r, 4), actor_name: col(&r, 5), device_id: col(&r, 6), op_id: col(&r, 7),
                    action: col_s(&r, 8), field_name: col(&r, 9), old_value: col(&r, 10), new_value: col(&r, 11),
                });
            }
        }
        for id in chunk {
            if let Some(b) = map.remove(id) {
                out.push(b);
            }
        }
    }
    Ok(out)
}

/// Open conflicts recorded in the shared database for this device (used to restore local conflict
/// records after a local restore, and by the conflict view).
pub fn open_conflicts_for_device(c: &mut dyn SharedConn, device_id: &str) -> SResult<Vec<(String, String, ConflictInfo)>> {
    Ok(c.query(
        "SELECT op_id, ticket_id, conflict_id, field_name, conflict_type, original_value, proposed_value, shared_value, reason FROM SyncConflicts WHERE device_id = ? AND conflict_status = 'open'",
        &[device_id.into()],
    )?
    .iter()
    .map(|r| {
        (col_s(r, 0), col_s(r, 1), ConflictInfo { conflict_id: col_s(r, 2), field_name: col_s(r, 3), conflict_type: col_s(r, 4), original: col(r, 5), proposed: col(r, 6), shared: col(r, 7), reason: col_s(r, 8) })
    })
    .collect())
}
