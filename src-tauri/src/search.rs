//! Full-text search index (SQLite FTS5, rowid = ticket id).
//!
//! One document per ticket, rebuilt whenever the ticket, its fields or its messages change.
//! It contains the ticket ID, issue names, sender details, all school fields (IDs keep their
//! leading zeros - they are indexed as text), subjects, email bodies, staff notes and actions.

use crate::catalog::catalog;
use crate::error::AppResult;
use crate::tickets::display_id;
use rusqlite::{params, Connection};

fn document(conn: &Connection, ticket_id: i64) -> AppResult<Option<String>> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT issue_type, sender_name, sender_email, original_subject, internal_notes, staff_actions, resolution_notes FROM tickets WHERE id=?1",
            [ticket_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .optional()?;
    let Some((issue, sname, semail, subj, notes, actions, resolution)) = row else { return Ok(None) };
    let mut parts: Vec<String> = vec![display_id(ticket_id), ticket_id.to_string()];
    let mut issues: Vec<String> = vec![issue];
    let mut st = conn.prepare("SELECT issue_type FROM ticket_issues WHERE ticket_id=?1")?;
    for i in st.query_map([ticket_id], |r| r.get::<_, String>(0))? {
        let i = i?;
        if !issues.contains(&i) {
            issues.push(i);
        }
    }
    for i in &issues {
        parts.push(catalog().issue(i).map(|d| d.name.clone()).unwrap_or_else(|| i.clone()));
    }
    parts.extend([sname, semail, subj, notes, actions, resolution].into_iter().flatten());
    // stable reference and shared notes (each staff note is its own record)
    if let Some(r) = conn.query_row("SELECT ticket_ref FROM tickets WHERE id=?1", [ticket_id], |r| r.get::<_, Option<String>>(0)).optional()?.flatten() {
        parts.push(r);
    }
    let mut st = conn.prepare("SELECT body FROM ticket_notes WHERE ticket_id=?1 AND deleted=0")?;
    for n in st.query_map([ticket_id], |r| r.get::<_, String>(0))? {
        parts.push(n?);
    }
    let mut st = conn.prepare("SELECT COALESCE(manual_value, extracted_value, '') FROM ticket_fields WHERE ticket_id=?1")?;
    for v in st.query_map([ticket_id], |r| r.get::<_, String>(0))? {
        parts.push(crate::render::display_value(&v?));
    }
    let mut st = conn.prepare(
        "SELECT COALESCE(m.subject,''), COALESCE(m.sender_name,''), COALESCE(m.sender_email,''), COALESCE(m.body_text,'')
         FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id WHERE l.ticket_id=?1",
    )?;
    for r in st.query_map([ticket_id], |r| Ok([r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?]))? {
        parts.extend(r?);
    }
    Ok(Some(parts.into_iter().filter(|p| !p.trim().is_empty()).collect::<Vec<_>>().join("\n")))
}

/// Rebuild the search document of one ticket.
pub fn reindex(conn: &Connection, ticket_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM ticket_search WHERE rowid=?1", [ticket_id])?;
    if let Some(doc) = document(conn, ticket_id)? {
        conn.execute("INSERT INTO ticket_search(rowid, content) VALUES(?1, ?2)", params![ticket_id, doc])?;
    }
    Ok(())
}

pub fn rebuild_all(conn: &Connection) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM ticket_search", [])?;
    let ids: Vec<i64> = {
        let mut st = tx.prepare("SELECT id FROM tickets")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for id in &ids {
        reindex(&tx, *id)?;
    }
    tx.commit()?;
    tracing::info!(tickets = ids.len(), "search index rebuilt");
    Ok(())
}

/// Turn free text into an FTS5 query: every word must match (as a prefix), punctuation is ignored.
/// "609-555-0100" -> "609"* "555"* "0100"*. Returns None when there is nothing to search for.
pub fn match_expr(input: &str) -> Option<String> {
    let toks: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(20)
        .map(|t| format!("\"{}\"*", t.to_lowercase()))
        .collect();
    if toks.is_empty() { None } else { Some(toks.join(" ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_expression() {
        assert_eq!(match_expr("  ").as_deref(), None);
        assert_eq!(match_expr("609-555-0100").as_deref(), Some("\"609\"* \"555\"* \"0100\"*"));
        assert_eq!(match_expr("Jane \"Doe\"").as_deref(), Some("\"jane\"* \"doe\"*"));
    }
}
