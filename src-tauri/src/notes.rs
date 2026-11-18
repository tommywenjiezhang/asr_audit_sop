//! Case notes: one record per note with its own id, author and version, so notes written by different
//! staff (or on different computers while offline) never overwrite each other. Deleting sets a marker.

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::sync_queue::{self as q, NewOp};
use crate::tickets::history;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub note_id: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub origin: String,
    /// "pending" while an add/edit/delete has not been accepted by the shared database
    pub sync_state: String,
}

pub fn list(conn: &Connection, ticket_id: i64) -> AppResult<Vec<Note>> {
    let mut st = conn.prepare(
        "SELECT n.note_id, n.author_name, n.body, n.created_at, n.updated_at, n.origin,
                CASE WHEN EXISTS(SELECT 1 FROM sync_ops o WHERE o.entity_type='note' AND o.entity_id=n.note_id AND o.state IN ('held','pending')) THEN 'pending'
                     WHEN EXISTS(SELECT 1 FROM sync_conflicts c WHERE c.field_name='note:'||n.note_id AND c.status<>'resolved') THEN 'conflict'
                     ELSE '' END
         FROM ticket_notes n WHERE n.ticket_id=?1 AND n.deleted=0 ORDER BY n.created_at, n.id",
    )?;
    let v = st
        .query_map([ticket_id], |r| {
            Ok(Note { note_id: r.get(0)?, author_name: r.get(1)?, body: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)?, origin: r.get(5)?, sync_state: r.get(6)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

fn validate(body: &str) -> AppResult<String> {
    let b = body.trim();
    if b.is_empty() {
        return Err(AppError::coded("INVALID", "The note is empty."));
    }
    if b.len() > 50_000 {
        return Err(AppError::coded("INVALID", "The note is too long."));
    }
    Ok(b.to_string())
}

/// Add a note (saved locally at once; queued for the shared database in the same transaction).
pub fn add(conn: &Connection, ticket_id: i64, body: &str, actor_name: &str) -> AppResult<Note> {
    let body = validate(body)?;
    let tx = conn.unchecked_transaction()?;
    crate::tickets::get(&tx, ticket_id)?;
    let a = q::actor(&tx, actor_name);
    let id = crate::shared_apply::new_id();
    let ts = now();
    tx.execute(
        "INSERT INTO ticket_notes(note_id, ticket_id, author_staff_id, author_name, body, version, created_at, updated_at, origin) VALUES(?1,?2,?3,?4,?5,0,?6,?6,'local')",
        params![id, ticket_id, a.staff_id, a.name, body, ts],
    )?;
    let mark = q::last_history_row(&tx)?;
    history(&tx, ticket_id, &a.name, "Note added", None, None, None)?;
    let op = q::enqueue(&tx, actor_name, NewOp {
        ticket_id: Some(ticket_id),
        entity_type: "note",
        entity_id: Some(id.clone()),
        op_type: "note.add",
        original: json!({}),
        proposed: json!({"note_id": id, "text": body, "author_name": a.name, "author_staff_id": a.staff_shared_id, "created_at": ts}),
    })?;
    q::tag_history(&tx, ticket_id, &op, mark)?;
    tx.execute("UPDATE tickets SET updated_at=?2 WHERE id=?1", params![ticket_id, ts])?;
    crate::search::reindex(&tx, ticket_id)?;
    tx.commit()?;
    list(conn, ticket_id)?.into_iter().find(|n| n.note_id == id).ok_or_else(|| AppError::msg("note not saved"))
}

fn current(conn: &Connection, note_id: &str) -> AppResult<(i64, String)> {
    conn.query_row("SELECT ticket_id, body FROM ticket_notes WHERE note_id=?1 AND deleted=0", [note_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| AppError::coded("NOT_FOUND", "Note not found."))
}

pub fn edit(conn: &Connection, note_id: &str, body: &str, actor_name: &str) -> AppResult<()> {
    let body = validate(body)?;
    let tx = conn.unchecked_transaction()?;
    let (tid, old) = current(&tx, note_id)?;
    if old == body {
        return Ok(());
    }
    tx.execute("UPDATE ticket_notes SET body=?2, updated_at=?3 WHERE note_id=?1", params![note_id, body, now()])?;
    let mark = q::last_history_row(&tx)?;
    history(&tx, tid, actor_name, "Note edited", None, None, None)?;
    let op = q::enqueue(&tx, actor_name, NewOp { ticket_id: Some(tid), entity_type: "note", entity_id: Some(note_id.into()), op_type: "note.edit", original: json!({"text": old}), proposed: json!({"text": body}) })?;
    q::tag_history(&tx, tid, &op, mark)?;
    crate::search::reindex(&tx, tid)?;
    tx.commit()?;
    Ok(())
}

pub fn delete(conn: &Connection, note_id: &str, actor_name: &str) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    let (tid, old) = current(&tx, note_id)?;
    tx.execute("UPDATE ticket_notes SET deleted=1, updated_at=?2 WHERE note_id=?1", params![note_id, now()])?;
    let mark = q::last_history_row(&tx)?;
    history(&tx, tid, actor_name, "Note deleted", None, None, None)?;
    let op = q::enqueue(&tx, actor_name, NewOp { ticket_id: Some(tid), entity_type: "note", entity_id: Some(note_id.into()), op_type: "note.delete", original: json!({"text": old}), proposed: json!({}) })?;
    q::tag_history(&tx, tid, &op, mark)?;
    crate::search::reindex(&tx, tid)?;
    tx.commit()?;
    Ok(())
}
