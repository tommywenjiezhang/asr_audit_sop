//! Advanced case management on this computer (P2): related-case suggestions, case links, correcting an
//! email's case, combining several cases (with a preview), and the complete activity timeline.
//!
//! Suggestions are evidence for staff, never actions: nothing is linked or combined automatically, and a
//! matching sender or school alone never combines cases. Every change is saved locally first and queued
//! for the shared database like the other workflow actions.

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::sync_queue::{self as q, NewOp};
use crate::tickets::{self, history};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

// ------------------------------------------------------------------ suggestions

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub ticket_id: i64,
    pub display_id: String,
    pub ticket_ref: Option<String>,
    pub status: String,
    pub owner: Option<String>,
    pub subject: Option<String>,
    pub school: Option<String>,
    pub received_at: Option<String>,
    pub score: i64,
    pub reasons: Vec<String>,
    /// already linked as related
    pub linked: bool,
    /// relation id when linked or suggested by the shared database
    pub relation_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Facts {
    id: i64,
    shared_id: Option<String>,
    status: String,
    sender: String,
    subject: String,
    conversations: BTreeSet<String>,
    njiis: BTreeSet<String>,
    schools: BTreeSet<String>,
    issues: BTreeSet<String>,
    received: Option<String>,
}

/// Subject without reply/forward prefixes and [tags], lower-case words of 3+ characters.
pub fn subject_words(s: &str) -> BTreeSet<String> {
    let mut t = s.to_lowercase();
    loop {
        let before = t.clone();
        for p in ["re:", "fw:", "fwd:", "re :", "fw :", "aw:", "[external]"] {
            t = t.trim().trim_start_matches(p).to_string();
        }
        if t == before {
            break;
        }
    }
    t.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3).map(String::from).collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(b).count() as f64 / a.union(b).count() as f64
}

fn facts_all(conn: &Connection) -> AppResult<HashMap<i64, Facts>> {
    let mut map: HashMap<i64, Facts> = HashMap::new();
    let mut st = conn.prepare("SELECT id, shared_id, status, lower(COALESCE(sender_email,'')), COALESCE(original_subject,''), COALESCE(first_received_at, created_at), lower(trim(COALESCE(school_njiis_id,''))), lower(trim(COALESCE(school_name,''))), conversation_id FROM tickets WHERE merged_into IS NULL")?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?, r.get::<_, Option<String>>(8)?))
    })?;
    for r in rows {
        let (id, sid, status, sender, subject, recv, nj, sn, conv) = r?;
        let mut f = Facts { id, shared_id: sid, status, sender, subject, received: recv, ..Default::default() };
        if !nj.is_empty() {
            f.njiis.insert(nj);
        }
        if !sn.is_empty() {
            f.schools.insert(sn);
        }
        if let Some(c) = conv.filter(|c| !c.is_empty()) {
            f.conversations.insert(c);
        }
        map.insert(id, f);
    }
    let mut st = conn.prepare("SELECT l.ticket_id, m.conversation_id FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id WHERE COALESCE(m.conversation_id,'')<>''")?;
    for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (t, c) = r?;
        if let Some(f) = map.get_mut(&t) {
            f.conversations.insert(c);
        }
    }
    let mut st = conn.prepare("SELECT ticket_id, field_key, lower(trim(COALESCE(NULLIF(manual_value,''), extracted_value, ''))) FROM ticket_fields WHERE field_key LIKE 'school%njiis_id' OR field_key LIKE 'school%name'")?;
    for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
        let (t, k, v) = r?;
        if v.is_empty() {
            continue;
        }
        if let Some(f) = map.get_mut(&t) {
            if k.ends_with("njiis_id") {
                f.njiis.insert(v);
            } else {
                f.schools.insert(v);
            }
        }
    }
    let mut st = conn.prepare("SELECT ticket_id, issue_type FROM ticket_issues")?;
    for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (t, i) = r?;
        if i != "other" {
            if let Some(f) = map.get_mut(&t) {
                f.issues.insert(i);
            }
        }
    }
    Ok(map)
}

/// Score how related `b` is to `a`; returns (score, reasons).
fn score(a: &Facts, b: &Facts) -> (i64, Vec<String>) {
    let mut s = 0;
    let mut why = vec![];
    if !a.conversations.is_disjoint(&b.conversations) {
        s += 60;
        why.push("Same Outlook conversation".to_string());
    }
    if !a.njiis.is_disjoint(&b.njiis) {
        s += 50;
        why.push("Same school NJIIS ID".to_string());
    } else if !a.schools.is_disjoint(&b.schools) {
        s += 15;
        why.push("Same school name".to_string());
    }
    if !a.sender.is_empty() && a.sender == b.sender {
        s += 25;
        why.push("Same sender".to_string());
    }
    let j = jaccard(&subject_words(&a.subject), &subject_words(&b.subject));
    if j >= 0.6 {
        s += 20;
        why.push("Similar subject".to_string());
    }
    let common: Vec<_> = a.issues.intersection(&b.issues).cloned().collect();
    if !common.is_empty() && s > 0 {
        s += 10;
        why.push(format!("Same issue type ({})", common.iter().filter_map(|i| crate::catalog::catalog().issue(i).map(|x| x.name.clone())).collect::<Vec<_>>().join(", ")));
    }
    (s, why)
}

/// Related-case suggestions for a case (highest evidence first), plus cases already linked to it.
pub fn suggestions(conn: &Connection, tid: i64, limit: usize) -> AppResult<Vec<Suggestion>> {
    let all = facts_all(conn)?;
    let me = all.get(&tid).cloned().ok_or_else(|| AppError::coded("NOT_FOUND", "Ticket not found."))?;
    let my_sid = me.shared_id.clone().unwrap_or_default();
    // links and shared "possible duplicate" suggestions involving this case
    let mut rel: HashMap<String, (String, String, String)> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT relation_id, ticket_a, ticket_b, relation_type, status FROM ticket_relations WHERE (ticket_a=?1 OR ticket_b=?1) AND status IN ('linked','open')")?;
        for r in st.query_map([&my_sid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))? {
            let (rid, a, b, kind, status) = r?;
            let other = if a == my_sid { b } else { a };
            rel.insert(other, (rid, kind, status));
        }
    }
    let mut out = vec![];
    for (id, f) in &all {
        if *id == tid {
            continue;
        }
        let (mut sc, mut why) = score(&me, f);
        let r = f.shared_id.as_ref().and_then(|s| rel.get(s)).cloned();
        let linked = matches!(&r, Some((_, k, st)) if k == "related" && st == "linked");
        if let Some((_, kind, st)) = &r {
            if kind == "possible_duplicate" && st == "open" {
                sc += 40;
                why.insert(0, "Possible duplicate (email without Message-ID: same sender, time and subject)".into());
            }
        }
        if linked || sc >= 25 {
            out.push((sc, *id, why, linked, r.map(|x| x.0)));
        }
    }
    out.sort_by(|a, b| b.3.cmp(&a.3).then(b.0.cmp(&a.0)).then(b.1.cmp(&a.1)));
    out.truncate(limit.max(1));
    let mut res = vec![];
    for (sc, id, why, linked, rid) in out {
        let t = tickets::get(conn, id)?;
        res.push(Suggestion {
            ticket_id: id,
            display_id: t.display_id.clone(),
            ticket_ref: t.ticket_ref.clone(),
            status: t.status.clone(),
            owner: t.assignee_name.clone(),
            subject: t.original_subject.clone(),
            school: t.school_name.clone(),
            received_at: t.first_received_at.clone(),
            score: sc,
            reasons: why,
            linked,
            relation_id: rid,
        });
        let _ = all.get(&id).map(|f| &f.status);
    }
    Ok(res)
}

// ------------------------------------------------------------------ links

/// Link two cases as related (no data moves; both stay separate cases).
pub fn link(conn: &Connection, a: i64, b: i64, actor: &str) -> AppResult<String> {
    if a == b {
        return Err(AppError::coded("INVALID", "Choose a different case."));
    }
    let ta = tickets::get(conn, a)?;
    let tb = tickets::get(conn, b)?;
    let tx = conn.unchecked_transaction()?;
    let sa_id = q::ensure_shared_id(&tx, a)?;
    let sb_id = q::ensure_shared_id(&tx, b)?;
    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT relation_id, status FROM ticket_relations WHERE relation_type='related' AND ((ticket_a=?1 AND ticket_b=?2) OR (ticket_a=?2 AND ticket_b=?1))",
            params![sa_id, sb_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((rid, st)) = &existing {
        if st == "linked" {
            return Ok(rid.clone());
        }
    }
    let rid = existing.map(|e| e.0).unwrap_or_else(crate::shared_apply::new_id);
    tx.execute(
        "INSERT INTO ticket_relations(relation_id, ticket_a, ticket_b, relation_type, status, created_at, origin) VALUES(?1,?2,?3,'related','linked',?4,'local')
         ON CONFLICT(relation_id) DO UPDATE SET status='linked'",
        params![rid, sa_id, sb_id, now()],
    )?;
    let mark = q::last_history_row(&tx)?;
    history(&tx, a, actor, "Linked to related case", None, None, tb.ticket_ref.as_deref())?;
    history(&tx, b, actor, "Linked to related case", None, None, ta.ticket_ref.as_deref())?;
    let op = q::enqueue(&tx, actor, NewOp { ticket_id: Some(a), entity_type: "relation", entity_id: Some(rid.clone()), op_type: "relation.link", original: json!({}), proposed: json!({"other_ticket_id": sb_id, "relation_type": "related", "relation_id": rid}) })?;
    q::tag_history(&tx, a, &op, mark)?;
    tx.commit()?;
    Ok(rid)
}

/// Remove a link, or dismiss a "possible duplicate" suggestion.
pub fn set_relation(conn: &Connection, tid: i64, relation_id: &str, status: &str, actor: &str) -> AppResult<()> {
    if !matches!(status, "linked" | "dismissed" | "removed") {
        return Err(AppError::coded("INVALID", "Unknown link status."));
    }
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute("UPDATE ticket_relations SET status=?2 WHERE relation_id=?1", params![relation_id, status])?;
    if n == 0 {
        return Err(AppError::coded("NOT_FOUND", "The link was not found."));
    }
    let mark = q::last_history_row(&tx)?;
    history(&tx, tid, actor, if status == "dismissed" { "Possible duplicate dismissed" } else if status == "removed" { "Case link removed" } else { "Linked to related case" }, None, None, None)?;
    let op = q::enqueue(&tx, actor, NewOp { ticket_id: Some(tid), entity_type: "relation", entity_id: Some(relation_id.into()), op_type: "relation.set_status", original: json!({}), proposed: json!({"relation_id": relation_id, "status": status}) })?;
    q::tag_history(&tx, tid, &op, mark)?;
    tx.commit()?;
    Ok(())
}

// ------------------------------------------------------------------ correcting an email's case

/// Move one email from a case to another case (wrongly threaded or wrongly added). The email is kept;
/// only its case changes. The last email of a case cannot be moved (combine the cases instead).
pub fn move_email(conn: &Connection, src: i64, message_id: i64, target: i64, actor: &str) -> AppResult<()> {
    if src == target {
        return Err(AppError::coded("INVALID", "Choose a different case."));
    }
    let s = tickets::get(conn, src)?;
    let t = tickets::get(conn, target)?;
    let tx = conn.unchecked_transaction()?;
    let link: Option<String> = tx.query_row("SELECT link_type FROM ticket_email_links WHERE ticket_id=?1 AND message_id=?2", params![src, message_id], |r| r.get(0)).optional()?;
    let Some(link_type) = link else {
        return Err(AppError::coded("NOT_FOUND", "This email is not part of the case."));
    };
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM ticket_email_links WHERE ticket_id=?1", [src], |r| r.get(0))?;
    if count <= 1 {
        return Err(AppError::coded("INVALID", "This is the only email of the case. Use Add to existing case to combine the cases instead."));
    }
    let shared_email: Option<String> = tx.query_row("SELECT shared_email_id FROM email_messages WHERE id=?1", [message_id], |r| r.get(0))?;
    let target_shared = q::ensure_shared_id(&tx, target)?;
    tx.execute("DELETE FROM ticket_email_links WHERE ticket_id=?1 AND message_id=?2", params![src, message_id])?;
    tx.execute(
        "INSERT OR IGNORE INTO ticket_email_links(ticket_id, message_id, link_type, linked_at) VALUES(?1,?2,?3,?4)",
        params![target, message_id, if link_type == "original" { "reply" } else { link_type.as_str() }, now()],
    )?;
    let subj: Option<String> = tx.query_row("SELECT subject FROM email_messages WHERE id=?1", [message_id], |r| r.get(0))?;
    let mark = q::last_history_row(&tx)?;
    history(&tx, src, actor, "Email moved to another case", None, subj.as_deref(), t.ticket_ref.as_deref())?;
    history(&tx, target, actor, "Email added from another case", None, s.ticket_ref.as_deref(), subj.as_deref())?;
    if let Some(eid) = shared_email {
        let op = q::enqueue(&tx, actor, NewOp { ticket_id: Some(src), entity_type: "email", entity_id: Some(eid.clone()), op_type: "email.move", original: json!({}), proposed: json!({"email_id": eid, "target_ticket_id": target_shared}) })?;
        q::tag_history(&tx, src, &op, mark)?;
    }
    for id in [src, target] {
        crate::scope::recompute_ticket(&tx, id, "system")?;
        crate::search::reindex(&tx, id)?;
    }
    tx.execute("UPDATE tickets SET updated_at=?2 WHERE id IN (?1, ?3)", params![src, now(), target])?;
    tx.commit()?;
    Ok(())
}

// ------------------------------------------------------------------ combining several cases

fn merged(conn: &Connection, id: i64) -> AppResult<bool> {
    Ok(conn.query_row("SELECT merged_into IS NOT NULL FROM tickets WHERE id=?1", [id], |r| r.get::<_, bool>(0)).optional()?.unwrap_or(false))
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeSource {
    pub ticket_id: i64,
    pub display_id: String,
    pub ticket_ref: Option<String>,
    pub status: String,
    pub owner: Option<String>,
    pub emails: i64,
    pub notes: i64,
    pub schools: Vec<String>,
    pub issues: Vec<String>,
    /// why this case cannot be combined now (None = OK)
    pub blocked: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergePreview {
    pub target_id: i64,
    pub target_display: String,
    pub target_ref: Option<String>,
    pub target_status: String,
    pub target_owner: Option<String>,
    pub sources: Vec<MergeSource>,
    /// combined totals after the merge
    pub emails_after: i64,
    pub notes_after: i64,
    pub schools_after: Vec<String>,
    pub issues_after: Vec<String>,
    pub status_after: String,
    pub can_run: bool,
}

fn school_labels(conn: &Connection, tid: i64) -> AppResult<Vec<String>> {
    let slots = tickets::school_slots(conn, tid)?;
    let mut out = vec![];
    for s in slots {
        let g = |attr: &str| -> AppResult<String> {
            Ok(conn
                .query_row("SELECT COALESCE(NULLIF(manual_value,''), extracted_value, '') FROM ticket_fields WHERE ticket_id=?1 AND field_key=?2", params![tid, crate::analyzer::school_key(s, attr)], |r| r.get::<_, String>(0))
                .optional()?
                .unwrap_or_default())
        };
        let (n, id) = (g("name")?, g("njiis_id")?);
        if !n.trim().is_empty() || !id.trim().is_empty() {
            out.push(if id.trim().is_empty() { n.trim().to_string() } else { format!("{} ({})", n.trim(), id.trim()) });
        }
    }
    Ok(out)
}

/// What combining `sources` into `target` would do (nothing is changed).
pub fn merge_preview(conn: &Connection, sources: &[i64], target: i64) -> AppResult<MergePreview> {
    let t = tickets::get(conn, target)?;
    if merged(conn, target)? {
        return Err(AppError::coded("INVALID", "The target case was itself combined into another case. Choose the remaining case."));
    }
    let cnt = |sql: &str, id: i64| -> AppResult<i64> { Ok(conn.query_row(sql, [id], |r| r.get(0))?) };
    let issues = |id: i64| -> AppResult<Vec<String>> { Ok(tickets::issues(conn, id)?.into_iter().map(|i| crate::catalog::catalog().issue(&i.issue_type).map(|x| x.name.clone()).unwrap_or(i.issue_type)).collect()) };
    let mut out_sources = vec![];
    let mut emails_after = cnt("SELECT COUNT(*) FROM ticket_email_links WHERE ticket_id=?1", target)?;
    let mut notes_after = cnt("SELECT COUNT(*) FROM ticket_notes WHERE ticket_id=?1 AND deleted=0", target)?;
    let mut schools_after = school_labels(conn, target)?;
    let mut issues_after = issues(target)?;
    let mut status_after = t.status.clone();
    let mut seen = BTreeSet::new();
    for &sid in sources {
        if sid == target || !seen.insert(sid) {
            continue;
        }
        let s = tickets::get(conn, sid)?;
        let mut blocked = None;
        let mut warnings = vec![];
        if merged(conn, sid)? {
            blocked = Some("Already combined into another case.".to_string());
        } else if let (Some(a), Some(b)) = (s.assignee_staff_id, t.assignee_staff_id) {
            if a != b {
                blocked = Some(format!("Different owners ({} and {}). Agree on one owner first (Take over).", s.assignee_name.clone().unwrap_or_default(), t.assignee_name.clone().unwrap_or_default()));
            }
        }
        if s.sync_state == "pending" || t.sync_state == "pending" {
            warnings.push("Unsynchronized changes are combined too; the shared database re-checks the merge.".into());
        }
        if s.status == tickets::DONE && t.status != tickets::DONE {
            warnings.push("This case is Done; its completion history is kept.".into());
        }
        let e = cnt("SELECT COUNT(*) FROM ticket_email_links WHERE ticket_id=?1", sid)?;
        let n = cnt("SELECT COUNT(*) FROM ticket_notes WHERE ticket_id=?1 AND deleted=0", sid)?;
        let sch = school_labels(conn, sid)?;
        let iss = issues(sid)?;
        if blocked.is_none() {
            emails_after += e;
            notes_after += n;
            for x in &sch {
                if !schools_after.contains(x) {
                    schools_after.push(x.clone());
                }
            }
            for x in &iss {
                if !issues_after.contains(x) {
                    issues_after.push(x.clone());
                }
            }
            if t.status == tickets::DONE && s.status != tickets::DONE {
                status_after = tickets::REVIEW.to_string();
            }
        }
        out_sources.push(MergeSource { ticket_id: sid, display_id: s.display_id, ticket_ref: s.ticket_ref, status: s.status, owner: s.assignee_name, emails: e, notes: n, schools: sch, issues: iss, blocked, warnings });
    }
    let can_run = !out_sources.is_empty() && out_sources.iter().all(|s| s.blocked.is_none());
    Ok(MergePreview {
        target_id: target,
        target_display: t.display_id,
        target_ref: t.ticket_ref,
        target_status: t.status,
        target_owner: t.assignee_name,
        sources: out_sources,
        emails_after,
        notes_after,
        schools_after,
        issues_after,
        status_after,
        can_run,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeResult {
    pub ticket_id: i64,
    pub display_id: String,
    pub ok: bool,
    pub message: String,
}

/// Combine every source into the target, one at a time (each one is reported separately).
pub fn merge_run(conn: &Connection, sources: &[i64], target: i64, actor: &str) -> AppResult<Vec<MergeResult>> {
    let preview = merge_preview(conn, sources, target)?;
    let mut out = vec![];
    for s in &preview.sources {
        if let Some(b) = &s.blocked {
            out.push(MergeResult { ticket_id: s.ticket_id, display_id: s.display_id.clone(), ok: false, message: b.clone() });
            continue;
        }
        match crate::workflow::join(conn, s.ticket_id, target, actor) {
            Ok(_) => out.push(MergeResult { ticket_id: s.ticket_id, display_id: s.display_id.clone(), ok: true, message: format!("Combined into {}", preview.target_display) }),
            Err(e) => out.push(MergeResult { ticket_id: s.ticket_id, display_id: s.display_id.clone(), ok: false, message: e.to_string() }),
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ timeline

#[derive(Debug, Clone, Serialize)]
pub struct TimelineItem {
    pub at: String,
    /// "email" | "note" | "owner" | "done" | "outlook" | "history"
    pub kind: String,
    pub actor: Option<String>,
    pub title: String,
    pub detail: Option<String>,
    pub ref_id: Option<String>,
}

/// Everything that happened to a case (newest first): emails, notes, ownership periods, Done decisions,
/// Outlook runs and the activity history (including cases combined into it).
pub fn timeline(conn: &Connection, tid: i64) -> AppResult<Vec<TimelineItem>> {
    let mut v: Vec<TimelineItem> = vec![];
    {
        let mut st = conn.prepare(
            "SELECT m.received_at, COALESCE(NULLIF(m.sender_name,''), m.sender_email), m.subject, l.link_type, m.is_internal, m.id FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id WHERE l.ticket_id=?1",
        )?;
        for r in st.query_map([tid], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?)))? {
            let (at, who, subj, lt, internal, mid) = r?;
            v.push(TimelineItem {
                at: at.unwrap_or_default(),
                kind: "email".into(),
                actor: who,
                title: if internal != 0 { "Internal email".into() } else if lt == "original" { "Email received (first)".into() } else { "Email received".into() },
                detail: subj,
                ref_id: Some(mid.to_string()),
            });
        }
    }
    {
        let mut st = conn.prepare("SELECT created_at, author_name, body, deleted, note_id FROM ticket_notes WHERE ticket_id=?1")?;
        for r in st.query_map([tid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?)))? {
            let (at, who, body, del, id) = r?;
            if del == 0 {
                v.push(TimelineItem { at, kind: "note".into(), actor: Some(who), title: "Note".into(), detail: Some(body.chars().take(400).collect()), ref_id: Some(id) });
            }
        }
    }
    {
        let mut st = conn.prepare("SELECT started_at, ended_at, COALESCE(staff_name, 'staff'), started_by, start_reason, ended_by, end_reason FROM ownership_history WHERE ticket_id=?1")?;
        for r in st.query_map([tid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?)))? {
            let (s, e, who, by, why, eby, ewhy) = r?;
            v.push(TimelineItem { at: s, kind: "owner".into(), actor: by, title: format!("Owner: {who}"), detail: why.map(|w| format!("reason: {w}")), ref_id: None });
            if let Some(e) = e {
                v.push(TimelineItem { at: e, kind: "owner".into(), actor: eby, title: format!("Ownership ended: {who}"), detail: ewhy.map(|w| format!("reason: {w}")), ref_id: None });
            }
        }
    }
    {
        let mut st = conn.prepare("SELECT decided_at, actor_name, state, state_reason, json_array_length(covered_json) FROM completion_history WHERE ticket_id=?1")?;
        for r in st.query_map([tid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)?)))? {
            let (at, who, state, why, n) = r?;
            v.push(TimelineItem { at, kind: "done".into(), actor: who, title: format!("Done decision ({state})"), detail: Some(format!("{n} incoming email(s) covered{}", why.map(|w| format!(" — {w}")).unwrap_or_default())), ref_id: None });
        }
    }
    {
        let mut st = conn.prepare("SELECT created_at, finished_at, kind, state, notify_state, staff_name, device_name, moved_count, email_count, last_error FROM shared_outlook_ops WHERE ticket_id=?1")?;
        for r in st.query_map([tid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?, r.get::<_, Option<String>>(9)?))
        })? {
            let (at, fin, kind, state, ns, who, dev, moved, total, err) = r?;
            let what = if kind == "assign" { "Outlook assignment" } else { "Outlook archiving" };
            let notify = ns.filter(|n| n != "skipped").map(|n| format!("; handoff {n}")).unwrap_or_default();
            v.push(TimelineItem {
                at: fin.unwrap_or(at),
                kind: "outlook".into(),
                actor: who,
                title: format!("{what}: {state}"),
                detail: Some(format!("{moved} of {total} email(s) moved{notify}{}{}", dev.map(|d| format!(" · {d}")).unwrap_or_default(), err.map(|e| format!(" · {e}")).unwrap_or_default())),
                ref_id: None,
            });
        }
    }
    {
        let mut st = conn.prepare("SELECT at, actor, action, field, old_value, new_value FROM ticket_history WHERE ticket_id=?1")?;
        for r in st.query_map([tid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?)))? {
            let (at, who, action, field, old, new) = r?;
            let detail = match (old, new) {
                (Some(o), Some(n)) if !o.is_empty() || !n.is_empty() => Some(format!("{}{o} → {n}", field.map(|f| format!("{f}: ")).unwrap_or_default())),
                (None, Some(n)) if !n.is_empty() => Some(n),
                (Some(o), None) if !o.is_empty() => Some(o),
                _ => None,
            };
            v.push(TimelineItem { at, kind: "history".into(), actor: Some(who), title: action, detail, ref_id: None });
        }
    }
    v.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(v)
}

/// Cases linked to this one (shared ids resolved to local cases) for the detail view.
#[derive(Debug, Clone, Serialize)]
pub struct LinkedCase {
    pub relation_id: String,
    pub relation_type: String,
    pub status: String,
    pub ticket_id: Option<i64>,
    pub display_id: Option<String>,
    pub ticket_ref: Option<String>,
    pub case_status: Option<String>,
    pub subject: Option<String>,
    pub evidence: Option<String>,
}

pub fn linked(conn: &Connection, tid: i64) -> AppResult<Vec<LinkedCase>> {
    let sid: Option<String> = conn.query_row("SELECT shared_id FROM tickets WHERE id=?1", [tid], |r| r.get(0))?;
    let Some(sid) = sid else { return Ok(vec![]) };
    let mut st = conn.prepare(
        "SELECT r.relation_id, r.relation_type, r.status, t.id, t.ticket_ref, t.status, t.original_subject, r.evidence
         FROM ticket_relations r LEFT JOIN tickets t ON t.shared_id = CASE WHEN r.ticket_a=?1 THEN r.ticket_b ELSE r.ticket_a END
         WHERE (r.ticket_a=?1 OR r.ticket_b=?1) AND r.status IN ('linked','open') ORDER BY r.created_at",
    )?;
    let v = st
        .query_map([&sid], |r| {
            let id: Option<i64> = r.get(3)?;
            Ok(LinkedCase {
                relation_id: r.get(0)?,
                relation_type: r.get(1)?,
                status: r.get(2)?,
                ticket_id: id,
                display_id: id.map(tickets::display_id),
                ticket_ref: r.get(4)?,
                case_status: r.get(5)?,
                subject: r.get(6)?,
                evidence: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_store::tests::new_shared;
    use crate::shared_sync::tests::{shared_count, Client};
    use crate::tickets::tests::msg;

    fn team() -> (tempfile::TempDir, std::path::PathBuf, Client, Client) {
        let dir = tempfile::tempdir().unwrap();
        let shared = new_shared(dir.path());
        let a = Client::new("Aaron");
        let w = Client::new("Wenjie");
        for (cl, n) in [(&a, "Aaron"), (&w, "Wenjie")] {
            let st = crate::staff::save(&cl.conn(), &crate::staff::tests::input(n, &format!("{}@example.org", n.to_lowercase()), &format!("F-{n}"))).unwrap();
            crate::staff::set_current_staff_id(&cl.conn(), Some(st.id)).unwrap();
        }
        a.connect(&shared);
        w.connect(&shared);
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        (dir, shared, a, w)
    }

    #[test]
    fn links_are_shared_and_never_combine_cases() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Subtype for Lincoln", "body", "nurse@school.org"));
        a.ingest(msg(2, "C2", "Audit for Lincoln", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let (t1, t2) = (a.ticket_by_imid("<m1@x>"), a.ticket_by_imid("<m2@x>"));
        let sug = suggestions(&a.conn(), t1, 10).unwrap();
        assert!(sug.iter().any(|s| s.ticket_id == t2 && s.reasons.contains(&"Same sender".to_string())), "{sug:?}");
        link(&a.conn(), t1, t2, "Aaron").unwrap();
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let w1 = w.ticket_by_imid("<m1@x>");
        let l = linked(&w.conn(), w1).unwrap();
        assert_eq!(l.len(), 1, "Wenjie sees the link");
        assert_eq!(l[0].ticket_id, Some(w.ticket_by_imid("<m2@x>")));
        assert_eq!(w.count("SELECT COUNT(*) FROM tickets WHERE merged_into IS NULL"), 2, "linking never combines");
        // Wenjie removes the link; Aaron sees it gone
        set_relation(&w.conn(), w1, &l[0].relation_id, "removed", "Wenjie").unwrap();
        w.sync(&shared).unwrap();
        a.sync(&shared).unwrap();
        assert!(linked(&a.conn(), t1).unwrap().is_empty());
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketRelations WHERE relation_status='removed'"), 1, "kept as history");
    }

    #[test]
    fn moving_a_wrongly_threaded_email_is_seen_by_the_colleague() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Subtype question", "body", "nurse@school.org"));
        let mut m2 = msg(2, "C1", "RE: Subtype question", "different school entirely", "nurse@school.org");
        m2.received_at = "2026-11-16T11:00:00Z".into();
        a.ingest(m2);
        a.ingest(msg(3, "C3", "Audit for Oak", "body", "office@oak.org"));
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        let t1 = a.ticket_by_imid("<m1@x>");
        assert_eq!(t1, a.ticket_by_imid("<m2@x>"), "threaded together by conversation");
        let t3 = a.ticket_by_imid("<m3@x>");
        let mid: i64 = a.count("SELECT id FROM email_messages WHERE internet_message_id='<m2@x>'");
        move_email(&a.conn(), t1, mid, t3, "Aaron").unwrap();
        assert_eq!(a.ticket_by_imid("<m2@x>"), t3);
        a.sync(&shared).unwrap();
        w.sync(&shared).unwrap();
        assert_eq!(w.ticket_by_imid("<m2@x>"), w.ticket_by_imid("<m3@x>"), "the colleague sees the corrected case");
        assert_eq!(w.count("SELECT COUNT(*) FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id WHERE m.internet_message_id='<m2@x>'"), 1, "one association only");
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketEmails WHERE is_active=0"), 1, "old association kept as history");
        // the last email of a case cannot be moved away
        let m3: i64 = a.count("SELECT id FROM email_messages WHERE internet_message_id='<m1@x>'");
        assert!(move_email(&a.conn(), t1, m3, t3, "Aaron").is_err());
    }

    #[test]
    fn combining_cases_keeps_schools_notes_history_and_refuses_loops() {
        let (_d, shared, a, w) = team();
        a.ingest(msg(1, "C1", "Subtype school A", "body", "nurse@school.org"));
        a.ingest(msg(2, "C2", "Subtype school B", "body", "nurse@school.org"));
        a.ingest(msg(3, "C3", "Subtype school C", "body", "nurse@school.org"));
        a.sync(&shared).unwrap();
        let (t1, t2, t3) = (a.ticket_by_imid("<m1@x>"), a.ticket_by_imid("<m2@x>"), a.ticket_by_imid("<m3@x>"));
        for (t, name, id) in [(t1, "Alpha School", "0001"), (t2, "Beta School", "0002"), (t3, "Gamma School", "0003")] {
            crate::tickets::set_field(&a.conn(), t, "school_name", Some(name.into()), false, "Aaron").unwrap();
            crate::tickets::set_field(&a.conn(), t, "school_njiis_id", Some(id.into()), true, "Aaron").unwrap();
        }
        crate::notes::add(&a.conn(), t2, "note on B", "Aaron").unwrap();
        crate::workflow::claim(&a.conn(), t3, "Aaron", false).unwrap();
        a.sync(&shared).unwrap();
        let p = merge_preview(&a.conn(), &[t2, t3], t1).unwrap();
        assert!(p.can_run, "{p:?}");
        assert_eq!(p.schools_after.len(), 3);
        assert_eq!((p.emails_after, p.notes_after), (3, 1));
        let r = merge_run(&a.conn(), &[t2, t3], t1, "Aaron").unwrap();
        assert!(r.iter().all(|x| x.ok), "{r:?}");
        a.sync(&shared).unwrap();
        // a fresh computer pulls everything and sees one case with three schools, the note and both aliases
        let f = Client::new("Fresh");
        f.connect(&shared);
        f.sync(&shared).unwrap();
        assert_eq!(f.count("SELECT COUNT(*) FROM tickets WHERE merged_into IS NULL"), 1, "{}", f.count("SELECT COUNT(*) FROM tickets"));
        let tf = f.ticket_by_imid("<m1@x>");
        let schools = school_labels(&f.conn(), tf).unwrap();
        assert_eq!(schools.len(), 3, "{schools:?}");
        assert!(schools.iter().any(|s| s.contains("0003")));
        assert_eq!(crate::notes::list(&f.conn(), tf).unwrap().len(), 1);
        assert_eq!(shared_count(&shared, "SELECT COUNT(*) FROM TicketAliases"), 2, "original references kept");
        assert_eq!(f.count(&format!("SELECT COUNT(*) FROM ownership_history WHERE ticket_id={tf}")), 2, "the combined case's owner period is kept and the owner carries over");
        assert_eq!(f.count(&format!("SELECT COUNT(*) FROM ownership_history WHERE ticket_id={tf} AND ended_at IS NULL")), 1, "exactly one current owner");
        assert_eq!(crate::tickets::get(&f.conn(), tf).unwrap().assignee_name.as_deref(), Some("Aaron"));
        let tl = timeline(&f.conn(), tf).unwrap();
        assert!(tl.iter().any(|i| i.title.contains("School added from combined case")));
        // circular: the remaining case cannot be combined into one of the cases combined into it
        w.sync(&shared).unwrap();
        let w1 = w.ticket_by_imid("<m1@x>");
        assert!(merge_preview(&w.conn(), &[w1], w1).unwrap().sources.is_empty(), "a case is never merged into itself");
        let hidden: i64 = w.count("SELECT id FROM tickets WHERE merged_into IS NOT NULL LIMIT 1");
        assert!(merge_preview(&w.conn(), &[w1], hidden).is_err(), "a combined case cannot become a target");
    }

    #[test]
    fn subject_normalization_and_similarity() {
        let a = subject_words("RE: FW: ASR submission for Lincoln Elementary");
        let b = subject_words("ASR submission - Lincoln Elementary");
        assert!(jaccard(&a, &b) >= 0.6, "{a:?} {b:?}");
        assert!(jaccard(&subject_words("Audit question"), &subject_words("Password reset")) < 0.2);
    }

    #[test]
    fn sender_alone_is_only_a_weak_suggestion_and_never_a_merge() {
        let mut a = Facts { sender: "x@school.org".into(), subject: "ASR question".into(), ..Default::default() };
        let mut b = Facts { sender: "x@school.org".into(), subject: "Different topic entirely".into(), ..Default::default() };
        let (s, why) = score(&a, &b);
        assert_eq!(s, 25);
        assert_eq!(why, vec!["Same sender".to_string()]);
        a.njiis.insert("12345".into());
        b.njiis.insert("12345".into());
        a.conversations.insert("C1".into());
        b.conversations.insert("C1".into());
        assert!(score(&a, &b).0 >= 135);
    }
}
