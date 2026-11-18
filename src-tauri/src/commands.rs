//! Tauri command handlers (thin wrappers; logic lives in the domain modules).

use crate::catalog::{catalog, Catalog};
use crate::completion;
use crate::drafts::{self, Draft, DraftCheck, Proposal};
use crate::error::{AppError, AppResult};
use crate::export;
use crate::render;
use crate::settings::Settings;
use crate::sync::{self, SyncOutcome, SyncStatus};
use crate::templates_store::{self as ts, TemplateRow, TemplateVersion};
use crate::tickets::{self, ListQuery, ListResult, NewTicket, TicketDetail, TicketField, TicketPatch};
use crate::AppState;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, State};

fn actor(state: &AppState) -> String {
    let s = state.settings.read().unwrap();
    if s.staff_name.trim().is_empty() { "staff".into() } else { s.staff_name.trim().to_string() }
}

fn settings(state: &AppState) -> Settings {
    state.settings.read().unwrap().clone()
}

// ------------------------------------------------------------------ catalog

#[derive(Serialize)]
pub struct CatalogInfo {
    #[serde(flatten)]
    pub catalog: Catalog,
    pub statuses: Vec<&'static str>,
    pub priorities: Vec<&'static str>,
}

#[tauri::command]
pub async fn catalog_get() -> AppResult<CatalogInfo> {
    Ok(CatalogInfo { catalog: catalog().clone(), statuses: tickets::STATUSES.to_vec(), priorities: tickets::PRIORITIES.to_vec() })
}

// ------------------------------------------------------------------ tickets

#[tauri::command]
pub async fn tickets_list(state: State<'_, AppState>, query: ListQuery) -> AppResult<ListResult> {
    tickets::list(&state.db.lock().unwrap(), &query)
}

#[tauri::command]
pub async fn ticket_detail(state: State<'_, AppState>, id: i64) -> AppResult<TicketDetail> {
    tickets::detail(&state.db.lock().unwrap(), id)
}

#[tauri::command]
pub async fn ticket_create(state: State<'_, AppState>, input: NewTicket) -> AppResult<i64> {
    let s = settings(&state);
    let id = tickets::create_manual(&state.db.lock().unwrap(), &input, &s, &actor(&state))?;
    sync::nudge_shared(&state);
    Ok(id)
}

#[tauri::command]
pub async fn ticket_update(state: State<'_, AppState>, id: i64, patch: TicketPatch) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    let t = tickets::update(&state.db.lock().unwrap(), id, &patch, &a)?;
    sync::nudge_shared(&state);
    Ok(t)
}

#[tauri::command]
pub async fn ticket_set_field(state: State<'_, AppState>, id: i64, key: String, value: Option<String>, verified: bool) -> AppResult<TicketField> {
    let a = actor(&state);
    let f = tickets::set_field(&state.db.lock().unwrap(), id, &key, value, verified, &a)?;
    sync::nudge_shared(&state);
    Ok(f)
}

/// Re-read one message's full body from Outlook (lazy, on demand).
#[tauri::command]
pub async fn message_refresh(state: State<'_, AppState>, message_id: i64) -> AppResult<String> {
    let (entry, store): (String, String) = state.db.lock().unwrap().query_row(
        "SELECT COALESCE(entry_id,''), COALESCE(store_id,'') FROM email_messages WHERE id=?1",
        [message_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let cfg = sync::worker_config(&state)?;
    let data = sync::call_with_retry(&cfg, "get_message", serde_json::json!({"entry_id": entry, "store_id": store, "body_max_chars": 10_000_000}), Duration::from_secs(60), 2).await?;
    let body = data.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
    let conn = state.db.lock().unwrap();
    conn.execute("UPDATE email_messages SET body_text=?2, body_truncated=0 WHERE id=?1", rusqlite::params![message_id, body])?;
    let tids: Vec<i64> = {
        let mut st = conn.prepare("SELECT ticket_id FROM ticket_email_links WHERE message_id=?1")?;
        let v = st.query_map([message_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for t in tids {
        crate::search::reindex(&conn, t)?;
    }
    Ok(body)
}

#[tauri::command]
pub async fn ticket_set_issues(state: State<'_, AppState>, id: i64, issues: Vec<String>) -> AppResult<Vec<tickets::IssueRow>> {
    let a = actor(&state);
    let r = tickets::set_issues(&state.db.lock().unwrap(), id, &issues, &a)?;
    sync::nudge_shared(&state);
    Ok(r)
}

#[tauri::command]
pub async fn ticket_add_school(state: State<'_, AppState>, id: i64) -> AppResult<i64> {
    let a = actor(&state);
    tickets::add_school(&state.db.lock().unwrap(), id, &a)
}

#[tauri::command]
pub async fn ticket_remove_school(state: State<'_, AppState>, id: i64, slot: i64) -> AppResult<()> {
    let a = actor(&state);
    tickets::remove_school(&state.db.lock().unwrap(), id, slot, &a)
}

/// Reopen a Done ticket (local status only; the archived email is not moved back or unflagged).
#[tauri::command]
pub async fn ticket_reopen(state: State<'_, AppState>, id: i64) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    let t = crate::workflow::reopen(&state.db.lock().unwrap(), id, &a, "Reopened by staff")?;
    sync::nudge_shared(&state);
    Ok(t)
}

/// Claim (or Take over with `takeover`): ownership only — emails stay where they are, no notification.
#[tauri::command]
pub async fn ticket_claim(state: State<'_, AppState>, id: i64, takeover: bool) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    let t = crate::workflow::claim(&state.db.lock().unwrap(), id, &a, takeover)?;
    sync::nudge_shared(&state);
    Ok(t)
}

#[tauri::command]
pub async fn ticket_release(state: State<'_, AppState>, id: i64) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    let t = crate::workflow::release(&state.db.lock().unwrap(), id, &a)?;
    sync::nudge_shared(&state);
    Ok(t)
}

#[tauri::command]
pub async fn ticket_flag_review(state: State<'_, AppState>, id: i64, reason: String) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    let t = crate::workflow::flag_review(&state.db.lock().unwrap(), id, &reason, &a)?;
    sync::nudge_shared(&state);
    Ok(t)
}

#[tauri::command]
pub async fn ticket_resume(state: State<'_, AppState>, id: i64) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    let t = crate::workflow::resume(&state.db.lock().unwrap(), id, &a)?;
    sync::nudge_shared(&state);
    Ok(t)
}

/// Add to Existing Case: `target` is a ticket number (T-000123) or a stable reference (ASR-…).
#[tauri::command]
pub async fn ticket_join(state: State<'_, AppState>, id: i64, target: String) -> AppResult<tickets::Ticket> {
    use rusqlite::OptionalExtension;
    let a = actor(&state);
    let conn = state.db.lock().unwrap();
    let tgt = target.trim();
    let digits = tgt.trim_start_matches(['T', 't']).trim_start_matches('-');
    let tid: Option<i64> = if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        conn.query_row("SELECT id FROM tickets WHERE upper(ticket_ref)=upper(?1) AND merged_into IS NULL", [tgt], |r| r.get(0)).optional()?
    };
    let tid = tid.ok_or_else(|| AppError::coded("NOT_FOUND", format!("No case {tgt} on this computer. Use the ticket number (T-…) or reference (ASR-…).")))?;
    let t = crate::workflow::join(&conn, id, tid, &a)?;
    drop(conn);
    sync::nudge_shared(&state);
    Ok(t)
}

/// Retry a refused action against the latest shared version (explicit; the conflict closes only when
/// the shared database accepts the retry).
#[tauri::command]
pub async fn conflict_retry(state: State<'_, AppState>, conflict_id: String) -> AppResult<()> {
    let a = actor(&state);
    crate::workflow::retry_conflict(&state.db.lock().unwrap(), &conflict_id, &a)?;
    sync::nudge_shared(&state);
    Ok(())
}

// ------------------------------------------------------------------ Copilot prompt

/// The prompt text exactly as it will be copied (built from saved data; works without Outlook).
#[tauri::command]
pub async fn prompt_get(state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let s = settings(&state);
    crate::prompt::build_prompt(&state.db.lock().unwrap(), id, &s)
}

/// Default general instructions (Settings › Restore Default).
#[tauri::command]
pub async fn prompt_default_instructions() -> AppResult<String> {
    Ok(crate::settings::DEFAULT_PROMPT_INSTRUCTIONS.to_string())
}

/// Called after the UI put the prompt on the clipboard: stores the copied text and time.
#[tauri::command]
pub async fn prompt_copied(state: State<'_, AppState>, id: i64, text: String) -> AppResult<String> {
    let a = actor(&state);
    tickets::record_prompt(&state.db.lock().unwrap(), id, &text, &a)
}

// ------------------------------------------------------------------ Outlook reply window

/// Folders where a message may be found again by Internet Message-ID if its EntryID changed
/// (staff often file emails into other subfolders): its last folder, the completion folder, the monitored
/// folders, every staff folder and the whole tree under the top folder. Lookup only - no tickets.
fn candidate_folders(state: &AppState, s: &Settings, own_store: &str, own_folder: &str) -> Vec<serde_json::Value> {
    let staff = crate::staff::list(&state.db.lock().unwrap(), true).unwrap_or_default();
    sync::candidate_folders(s, &staff, own_store, own_folder)
}

#[derive(Serialize)]
pub struct OpenReplyResult {
    pub opened: bool,
    pub reply_all: bool,
    pub message_id: i64,
    pub relocated: bool,
}

/// Open classic Outlook's Reply / Reply All window for the ticket's source email (default: the latest
/// incoming email). Nothing is inserted, saved or sent, and the ticket status does not change.
#[tauri::command]
pub async fn outlook_open_reply(state: State<'_, AppState>, ticket_id: i64, message_id: Option<i64>, reply_all: bool) -> AppResult<OpenReplyResult> {
    use rusqlite::OptionalExtension;
    let s = settings(&state);
    let src: Option<(i64, String, String, String, String)> = {
        let conn = state.db.lock().unwrap();
        tickets::get(&conn, ticket_id)?;
        let sql = "SELECT m.id, COALESCE(m.entry_id,''), COALESCE(m.store_id,''), COALESCE(m.internet_message_id,''), COALESCE(m.folder_entry_id,'')
                   FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
                   WHERE l.ticket_id=?1 AND COALESCE(m.entry_id,'')<>'' AND (?2 IS NULL AND l.link_type IN ('original','reply') AND m.is_internal=0 OR m.id=?2)
                   ORDER BY m.received_at DESC, m.id DESC LIMIT 1";
        conn.query_row(sql, rusqlite::params![ticket_id, message_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?
    };
    let Some((mid, entry, store, imid, folder)) = src else {
        return Err(AppError::coded("NO_SOURCE_EMAIL", "No Outlook email is linked to this ticket."));
    };
    let cfg = sync::worker_config(&state)?;
    let params = serde_json::json!({"entry_id": entry, "store_id": store, "imid": imid, "folders": candidate_folders(&state, &s, &store, &folder), "reply_all": reply_all});
    // no retries: a retry could open a second window
    let data = sync::call_with_retry(&cfg, "open_reply", params, Duration::from_secs(60), 1).await?;
    let relocated = data.get("relocated").and_then(|v| v.as_bool()).unwrap_or(false);
    let conn = state.db.lock().unwrap();
    if relocated {
        if let Some(src) = data.get("source") {
            let g = |k: &str| src.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !g("entry_id").is_empty() {
                conn.execute(
                    "UPDATE email_messages SET entry_id=?2, store_id=COALESCE(NULLIF(?3,''),store_id), folder_entry_id=COALESCE(NULLIF(?4,''),folder_entry_id), folder_path=COALESCE(NULLIF(?5,''),folder_path) WHERE id=?1",
                    rusqlite::params![mid, g("entry_id"), g("store_id"), g("folder_entry_id"), g("folder_path")],
                )?;
            }
        }
    }
    tickets::history(&conn, ticket_id, &actor(&state), if reply_all { "Opened Outlook Reply All window" } else { "Opened Outlook reply window" }, None, None, None)?;
    Ok(OpenReplyResult { opened: true, reply_all, message_id: mid, relocated })
}

/// Explicit fallback when the source email cannot be found: a new, unthreaded message to the sender.
#[tauri::command]
pub async fn outlook_open_new(state: State<'_, AppState>, ticket_id: i64) -> AppResult<()> {
    let t = tickets::get(&state.db.lock().unwrap(), ticket_id)?;
    let to = t.sender_email.clone().unwrap_or_default();
    if to.trim().is_empty() {
        return Err(AppError::coded("NO_RECIPIENT", "This ticket has no sender email address. Add it under Sender first."));
    }
    let subj = t.original_subject.clone().unwrap_or_default();
    let subject = if subj.to_lowercase().starts_with("re:") { subj } else { format!("RE: {subj}") };
    let cfg = sync::worker_config(&state)?;
    sync::call_with_retry(&cfg, "open_new_message", serde_json::json!({"to": to, "subject": subject}), Duration::from_secs(60), 1).await?;
    tickets::history(&state.db.lock().unwrap(), ticket_id, &actor(&state), "Opened new (unthreaded) Outlook message to sender", None, None, Some(&to))?;
    Ok(())
}

// ------------------------------------------------------------------ Mark Done

/// Prevents two conflicting Outlook operations (Mark Done / assignment) on the same ticket at the same time.
struct BusyGuard<'a> {
    set: &'a std::sync::Mutex<std::collections::HashSet<i64>>,
    id: i64,
}
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.set.lock().unwrap().remove(&self.id);
    }
}
fn busy<'a>(set: &'a std::sync::Mutex<std::collections::HashSet<i64>>, id: i64) -> AppResult<BusyGuard<'a>> {
    if !set.lock().unwrap().insert(id) {
        return Err(AppError::coded("BUSY", format!("{} is busy: an assignment or Mark Done is already running for it.", tickets::display_id(id))));
    }
    Ok(BusyGuard { set, id })
}

#[derive(Serialize)]
pub struct MarkDoneResult {
    pub decision: crate::workflow::DoneDecision,
    /// Outlook archiving result when it ran (standalone mode with a completion folder)
    pub outlook: Option<completion::Outcome>,
    /// plain-language note about Outlook (not updated / failed / done)
    pub outlook_message: String,
}

fn shared_bound(state: &AppState) -> bool {
    let conn = state.db.lock().unwrap();
    crate::sync_queue::mode(&conn) == crate::sync_queue::MODE_SHARED && crate::sync_queue::bound_db(&conn).is_some()
}

/// Mark Done: the staff decision is saved at once (works offline; no sent-reply verification). With a
/// shared database, Outlook archiving is a separate step after the shared database accepted the decision;
/// on a standalone computer it runs right away. An Outlook failure never undoes the Done decision.
#[tauri::command]
pub async fn ticket_complete(state: State<'_, AppState>, id: i64) -> AppResult<MarkDoneResult> {
    let _guard = busy(&state.completing, id)?;
    let a = actor(&state);
    let decision = crate::workflow::mark_done(&state.db.lock().unwrap(), id, &a)?;
    sync::nudge_shared(&state);
    if shared_bound(&state) {
        return Ok(MarkDoneResult { decision, outlook: None, outlook_message: "Outlook was not changed yet. After the shared database confirms the Done decision, use Archive in Outlook to flag and move the covered emails.".into() });
    }
    if settings(&state).archive_folder.is_none() {
        return Ok(MarkDoneResult { decision, outlook: None, outlook_message: "Outlook was not changed: no completion folder is configured (Settings › Completion Folder).".into() });
    }
    let (outlook, outlook_message) = match run_archive(&state, id).await {
        Ok(o) if o.done => (Some(o.clone()), if o.dest_path.is_empty() { "No Outlook emails to archive.".into() } else { format!("Outlook updated: {} email(s) flagged complete and moved to {}.", o.moved, o.dest_path) }),
        Ok(o) => (Some(o), "The case is Done, but some Outlook steps failed. Use Retry Outlook update.".into()),
        Err(e) => (None, format!("The case is Done, but Outlook could not be updated: {e} Use Retry Outlook update.")),
    };
    Ok(MarkDoneResult { decision, outlook, outlook_message })
}

/// Archive in Outlook (after Mark Done): flag complete + move only the emails covered by the latest Done
/// decision. With a shared database the decision must be accepted there and the case still Done.
#[tauri::command]
pub async fn ticket_archive_outlook(state: State<'_, AppState>, id: i64) -> AppResult<completion::Outcome> {
    let _guard = busy(&state.completing, id)?;
    {
        let conn = state.db.lock().unwrap();
        let t = tickets::get(&conn, id)?;
        if t.status != tickets::DONE {
            return Err(AppError::coded("NOT_DONE", "Only Done cases are archived in Outlook."));
        }
        if crate::sync_queue::mode(&conn) == crate::sync_queue::MODE_SHARED && crate::sync_queue::bound_db(&conn).is_some() {
            let st: Option<String> = conn.query_row("SELECT state FROM completion_history WHERE ticket_id=?1 ORDER BY decided_at DESC, id DESC LIMIT 1", [id], |r| r.get(0)).ok();
            if !matches!(st.as_deref(), Some("accepted") | Some("legacy")) || t.sync_state != "synced" {
                return Err(AppError::coded("NOT_CONFIRMED", "The Done decision is not confirmed by the shared database yet (or the case has unsynchronized changes). Sync first; Outlook is changed only for confirmed decisions."));
            }
        }
    }
    run_archive(&state, id).await
}

async fn run_archive(state: &AppState, id: i64) -> AppResult<completion::Outcome> {
    let s = settings(state);
    let a = actor(state);
    let items = {
        let conn = state.db.lock().unwrap();
        let items = completion::pending(&conn, id)?;
        if items.is_empty() {
            return Ok(completion::Outcome { done: true, flagged: 0, moved: 0, failed: vec![], dest_path: String::new() });
        }
        items
    };
    let dest = s.archive_folder.clone().ok_or_else(|| {
        AppError::coded("NOT_CONFIGURED", "Choose a completion folder in Settings › Completion Folder before marking tickets Done. The app never guesses where to archive emails.")
    })?;
    completion::begin_attempt(&state.db.lock().unwrap(), id, &items)?;
    let req_items: Vec<serde_json::Value> = items
        .iter()
        .map(|i| {
            serde_json::json!({
                "message_id": i.message_id, "entry_id": i.entry_id, "store_id": i.store_id, "imid": i.internet_message_id,
                "folders": candidate_folders(state, &s, &i.store_id, &i.folder_entry_id), "need_flag": i.need_flag, "need_move": i.need_move,
            })
        })
        .collect();
    let cfg = sync::worker_config(state)?;
    let params = serde_json::json!({"dest": {"store_id": dest.store_id, "entry_id": dest.entry_id}, "items": req_items});
    // no automatic retries: results must be recorded before anything is attempted again
    let res = crate::outlook::call(&cfg, "complete_messages", params, Duration::from_secs(180)).await.and_then(crate::outlook::into_result);
    let conn = state.db.lock().unwrap();
    match res {
        Ok(data) => {
            let results: Vec<completion::ItemResult> = serde_json::from_value(data.get("items").cloned().unwrap_or_default())?;
            let o = completion::apply_results(&conn, id, &dest, &items, &results, &a)?;
            crate::diag::record(&conn, if o.failed.is_empty() { crate::diag::Severity::Info } else { crate::diag::Severity::Warning }, "outlook-action", if o.failed.is_empty() { "ARCHIVE_OK" } else { "ARCHIVE_PARTIAL" },
                &format!("Outlook archive for {}: {} flagged, {} moved, {} failed", tickets::display_id(id), o.flagged, o.moved, o.failed.len()), "", &tickets::display_id(id));
            Ok(o)
        }
        Err(e) => {
            completion::record_failure(&conn, id, &items, &e.to_string(), &a)?;
            crate::diag::record(&conn, crate::diag::Severity::Error, "outlook-action", e.code(), &format!("Outlook archive for {} failed: {e}", tickets::display_id(id)), "", &tickets::display_id(id));
            Err(e)
        }
    }
}

/// Check that a folder exists and is a mail folder (used for the completion folder in Settings).
#[tauri::command]
pub async fn outlook_validate_folder(state: State<'_, AppState>, store_id: String, entry_id: String) -> AppResult<serde_json::Value> {
    let cfg = sync::worker_config(&state)?;
    sync::call_with_retry(&cfg, "validate_folder", serde_json::json!({"store_id": store_id, "entry_id": entry_id}), Duration::from_secs(60), 1).await
}

// ------------------------------------------------------------------ templates

#[tauri::command]
pub async fn templates_list(state: State<'_, AppState>, issue_type: Option<String>) -> AppResult<Vec<TemplateRow>> {
    ts::list(&state.db.lock().unwrap(), issue_type.as_deref())
}

#[tauri::command]
pub async fn template_get(state: State<'_, AppState>, id: String) -> AppResult<TemplateRow> {
    ts::get(&state.db.lock().unwrap(), &id)
}

#[tauri::command]
pub async fn template_save(state: State<'_, AppState>, id: String, subject: String, body_html: String, name: Option<String>, note: Option<String>) -> AppResult<TemplateRow> {
    ts::save(&state.db.lock().unwrap(), &id, &subject, &body_html, name.as_deref(), note.as_deref())
}

#[tauri::command]
pub async fn template_duplicate(state: State<'_, AppState>, id: String, name: String) -> AppResult<TemplateRow> {
    ts::duplicate(&state.db.lock().unwrap(), &id, &name)
}

#[tauri::command]
pub async fn template_set_enabled(state: State<'_, AppState>, id: String, enabled: bool) -> AppResult<TemplateRow> {
    ts::set_enabled(&state.db.lock().unwrap(), &id, enabled)
}

#[tauri::command]
pub async fn template_restore_default(state: State<'_, AppState>, id: String) -> AppResult<TemplateRow> {
    ts::restore_default(&state.db.lock().unwrap(), &id)
}

#[tauri::command]
pub async fn template_versions(state: State<'_, AppState>, id: String) -> AppResult<Vec<TemplateVersion>> {
    ts::versions(&state.db.lock().unwrap(), &id)
}

#[derive(Serialize)]
pub struct Preview {
    pub subject: String,
    pub body_html: String,
    pub check: DraftCheck,
}

/// Preview a template body with sample data or with a ticket's data.
#[tauri::command]
pub async fn template_preview(state: State<'_, AppState>, template_id: Option<String>, subject: String, body_html: String, ticket_id: Option<i64>) -> AppResult<Preview> {
    let s = settings(&state);
    let conn = state.db.lock().unwrap();
    let ctx = match ticket_id {
        Some(id) => tickets::build_ctx(&conn, id, &s)?,
        None => render::sample_ctx(),
    };
    let body = render::fill(&ts::sanitize_html(&ts::strip_placeholder_marks(&body_html)), &ctx);
    let subject = render::fill_text(&subject, &ctx);
    let t = template_id.as_deref().and_then(|id| ts::get(&conn, id).ok());
    Ok(Preview { check: drafts::check(t.as_ref(), &subject, &body, &ctx), subject, body_html: body })
}

// ------------------------------------------------------------------ drafts

#[derive(Serialize)]
pub struct DraftState {
    pub draft: Option<Draft>,
    pub check: Option<DraftCheck>,
}

#[tauri::command]
pub async fn draft_get(state: State<'_, AppState>, ticket_id: i64) -> AppResult<DraftState> {
    let s = settings(&state);
    let conn = state.db.lock().unwrap();
    let d = drafts::get(&conn, ticket_id)?;
    let check = match &d {
        Some(d) => {
            let ctx = tickets::build_ctx(&conn, ticket_id, &s)?;
            let t = d.template_id.as_deref().and_then(|id| ts::get(&conn, id).ok());
            Some(drafts::check(t.as_ref(), &d.subject, &d.body_html, &ctx))
        }
        None => None,
    };
    Ok(DraftState { draft: d, check })
}

#[tauri::command]
pub async fn draft_check(state: State<'_, AppState>, ticket_id: i64, template_id: Option<String>, subject: String, body_html: String) -> AppResult<DraftCheck> {
    let s = settings(&state);
    let conn = state.db.lock().unwrap();
    let ctx = tickets::build_ctx(&conn, ticket_id, &s)?;
    let t = template_id.as_deref().and_then(|id| ts::get(&conn, id).ok());
    Ok(drafts::check(t.as_ref(), &subject, &ts::strip_placeholder_marks(&body_html), &ctx))
}

#[tauri::command]
pub async fn draft_load_default(state: State<'_, AppState>, ticket_id: i64, template_id: Option<String>) -> AppResult<Proposal> {
    let s = settings(&state);
    drafts::load_default(&state.db.lock().unwrap(), ticket_id, template_id.as_deref(), &s)
}

#[tauri::command]
pub async fn draft_reset(state: State<'_, AppState>, ticket_id: i64, template_id: Option<String>) -> AppResult<Proposal> {
    let s = settings(&state);
    drafts::reset_default(&state.db.lock().unwrap(), ticket_id, template_id.as_deref(), &s)
}

#[tauri::command]
pub async fn draft_generate(state: State<'_, AppState>, ticket_id: i64) -> AppResult<Proposal> {
    let s = settings(&state);
    drafts::generate(&state.db.lock().unwrap(), ticket_id, &s)
}

#[tauri::command]
pub async fn draft_fill(state: State<'_, AppState>, ticket_id: i64, template_id: Option<String>, subject: String, body_html: String) -> AppResult<Proposal> {
    let s = settings(&state);
    drafts::fill_from_ticket(&state.db.lock().unwrap(), ticket_id, template_id.as_deref(), &subject, &body_html, &s)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn draft_save(state: State<'_, AppState>, ticket_id: i64, template_id: Option<String>, subject: String, body_html: String, user_edited: bool, reason: Option<String>) -> AppResult<Draft> {
    let a = actor(&state);
    drafts::save(&state.db.lock().unwrap(), ticket_id, template_id.as_deref(), &subject, &body_html, user_edited, reason.as_deref().unwrap_or("Saved"), &a)
}

#[tauri::command]
pub async fn draft_versions(state: State<'_, AppState>, ticket_id: i64) -> AppResult<Vec<drafts::DraftVersion>> {
    drafts::versions(&state.db.lock().unwrap(), ticket_id)
}

#[derive(Serialize)]
pub struct OutlookDraftResult {
    pub created: bool,
    pub needs_confirmation: bool,
    pub check: DraftCheck,
    pub entry_id: Option<String>,
    pub mode: Option<String>,
}

#[tauri::command]
pub async fn draft_create_outlook(state: State<'_, AppState>, ticket_id: i64, confirm_incomplete: bool) -> AppResult<OutlookDraftResult> {
    let s = settings(&state);
    let (draft, ticket, check, latest): (Draft, tickets::Ticket, DraftCheck, Option<(String, String)>) = {
        let conn = state.db.lock().unwrap();
        let d = drafts::get(&conn, ticket_id)?.ok_or_else(|| AppError::coded("NO_DRAFT", "Save the draft before creating an Outlook draft."))?;
        let t = tickets::get(&conn, ticket_id)?;
        let ctx = tickets::build_ctx(&conn, ticket_id, &s)?;
        let tpl = d.template_id.as_deref().and_then(|id| ts::get(&conn, id).ok());
        let chk = drafts::check(tpl.as_ref(), &d.subject, &d.body_html, &ctx);
        use rusqlite::OptionalExtension;
        let latest = conn
            .query_row(
                "SELECT m.entry_id, m.store_id FROM ticket_email_links l JOIN email_messages m ON m.id=l.message_id
                 WHERE l.ticket_id=?1 AND l.link_type IN ('original','reply') AND COALESCE(m.entry_id,'')<>''
                 ORDER BY m.received_at DESC LIMIT 1",
                [ticket_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())),
            )
            .optional()?;
        (d, t, chk, latest)
    };
    let incomplete = !check.unresolved.is_empty() || !check.unverified_claims.is_empty();
    if incomplete && !confirm_incomplete {
        return Ok(OutlookDraftResult { created: false, needs_confirmation: true, check, entry_id: None, mode: None });
    }
    let (html, images) = drafts::outlook_html(&draft.body_html, incomplete, &state.images_dir);
    let subject = if incomplete { format!("[INCOMPLETE DRAFT] {}", draft.subject) } else { draft.subject.clone() };
    let mode = if latest.is_some() { "reply" } else { "new" };
    if mode == "new" && ticket.sender_email.as_deref().unwrap_or("").is_empty() {
        return Err(AppError::coded("NO_RECIPIENT", "This ticket has no linked Outlook email and no sender email address."));
    }
    let (entry_id, store_id) = latest.unwrap_or_default();
    let cfg = sync::worker_config(&state)?;
    // no retries: a retry after a partial success could create a second draft
    let data = sync::call_with_retry(
        &cfg,
        "create_reply_draft",
        serde_json::json!({
            "mode": mode,
            "entry_id": entry_id,
            "store_id": store_id,
            "to": ticket.sender_email.clone().unwrap_or_default(),
            "subject": subject,
            "html_body": html,
            "inline_images": images,
            "display": s.open_outlook_draft_after_create,
        }),
        Duration::from_secs(90),
        1,
    )
    .await?;
    let new_entry = data.get("entry_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    drafts::mark_outlook_created(&state.db.lock().unwrap(), ticket_id, &new_entry, incomplete, &actor(&state))?;
    Ok(OutlookDraftResult { created: true, needs_confirmation: false, check, entry_id: Some(new_entry), mode: Some(mode.into()) })
}

// ------------------------------------------------------------------ outlook / sync

#[tauri::command]
pub async fn outlook_test(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let cfg = sync::worker_config(&state)?;
    sync::call_with_retry(&cfg, "test_connection", serde_json::json!({}), Duration::from_secs(60), 1).await
}

#[tauri::command]
pub async fn outlook_list_mailboxes(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let cfg = sync::worker_config(&state)?;
    sync::call_with_retry(&cfg, "list_mailboxes", serde_json::json!({}), Duration::from_secs(90), 1).await
}

#[tauri::command]
pub async fn outlook_list_folders(state: State<'_, AppState>, store_id: Option<String>, shared_mailbox: Option<String>, root_entry_id: Option<String>) -> AppResult<serde_json::Value> {
    let cfg = sync::worker_config(&state)?;
    sync::call_with_retry(
        &cfg,
        "list_folders",
        serde_json::json!({"store_id": store_id.unwrap_or_default(), "shared_mailbox": shared_mailbox.unwrap_or_default(),
            "root_entry_id": root_entry_id.unwrap_or_default(), "max_depth": 6}),
        Duration::from_secs(120),
        1,
    )
    .await
}

#[tauri::command]
pub async fn sync_now(app: AppHandle) -> AppResult<SyncOutcome> {
    sync::run_once(&app, "manual").await
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> AppResult<SyncStatus> {
    let mut s = state.sync.snapshot();
    if s.last_success_at.is_none() {
        // fall back to persisted state after restart
        s.last_success_at = state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT MAX(last_success_at) FROM sync_state", [], |r| r.get::<_, Option<String>>(0))?;
    }
    s.folder_display = {
        let st = state.settings.read().unwrap();
        st.folder.as_ref().map(|_| st.scan_summary())
    };
    Ok(s)
}

// ------------------------------------------------------------------ settings

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> AppResult<Settings> {
    Ok(settings(&state))
}

#[tauri::command]
pub async fn settings_save(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> AppResult<Settings> {
    let old = self::settings(&state);
    let mut settings = settings.normalized();
    // shared database settings have their own save (shared_save_config); a stale form never overwrites them
    settings.shared_mode = old.shared_mode.clone();
    settings.shared_db_path = old.shared_db_path.clone();
    settings.shared_sync_minutes = old.shared_sync_minutes;
    settings.validate()?;
    // a newly chosen completion folder must exist and be a mail folder
    if let Some(a) = settings.archive_folder.as_ref().filter(|a| old.archive_folder.as_ref() != Some(*a)) {
        let cfg = sync::worker_config(&state)?;
        let r = sync::call_with_retry(&cfg, "validate_folder", serde_json::json!({"store_id": a.store_id, "entry_id": a.entry_id}), Duration::from_secs(60), 1).await;
        match r {
            Ok(_) => {}
            Err(e) if ["FOLDER_NOT_FOUND", "NOT_MAIL_FOLDER"].contains(&e.code()) => return Err(e),
            Err(e) => tracing::warn!(error = %e, "completion folder could not be validated now (Outlook unavailable?)"),
        }
    }
    settings.save(&state.settings_path)?;
    *state.settings.write().unwrap() = settings.clone();
    if old.launch_at_login != settings.launch_at_login {
        use tauri_plugin_autostart::ManagerExt;
        let al = app.autolaunch();
        let r = if settings.launch_at_login { al.enable() } else { al.disable() };
        if let Err(e) = r {
            tracing::warn!(error = %e, "could not change launch-at-login");
        }
    }
    // a different folder selection: confirmed memberships become "awaiting verification", then re-check
    let new_key = crate::scope::cfg(&settings).key;
    if crate::scope::cfg(&old).key != new_key && settings.folder.is_some() {
        let n = crate::scope::invalidate_for_new_scope(&state.db.lock().unwrap(), &new_key)?;
        tracing::info!(emails = n, "folder selection changed; membership will be re-verified");
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = sync::run_once(&app2, "scope-change").await {
                tracing::warn!(error = %e, "check after folder change failed");
            }
        });
    }
    state.sync.wake.notify_one();
    tracing::info!(interval = settings.interval_minutes, enabled = settings.sync_enabled, "settings saved");
    Ok(settings)
}

// ------------------------------------------------------------------ reports / data

#[tauri::command]
pub async fn report_get(state: State<'_, AppState>) -> AppResult<export::Report> {
    export::report(&state.db.lock().unwrap())
}

#[tauri::command]
pub async fn export_csv(state: State<'_, AppState>, path: String, status: Option<String>) -> AppResult<usize> {
    export::export_csv(&state.db.lock().unwrap(), &PathBuf::from(path), status.as_deref().unwrap_or(""))
}

#[tauri::command]
pub async fn db_backup(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let p = PathBuf::from(&path);
    if p == state.data_dir.join("support_desk.db") {
        return Err(AppError::coded("INVALID", "Choose a different file than the live database."));
    }
    export::backup(&state.db.lock().unwrap(), &p)
}

#[tauri::command]
pub async fn db_restore(app: AppHandle, state: State<'_, AppState>, path: String) -> AppResult<String> {
    let safety = state.data_dir.join("backups").join(format!("pre-restore-{}.db", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::create_dir_all(safety.parent().unwrap())?;
    {
        let mut conn = state.db.lock().unwrap();
        export::restore(&mut conn, &PathBuf::from(path), &safety)?;
        // queued changes from the backup may be older than shared work: hold them for review, never replay
        let held = crate::sync_queue::after_restore(&conn)?;
        crate::diag::record(&conn, crate::diag::Severity::Warning, "backup", "RESTORED", &format!("Local backup restored; {held} queued change(s) held for review instead of being sent"), "", "");
    }
    use tauri::Emitter;
    let _ = app.emit("tickets-changed", ());
    Ok(safety.to_string_lossy().to_string())
}

#[derive(Serialize)]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub db_path: String,
    pub log_dir: String,
    pub startup_ms: u64,
    pub schema_version: i64,
    pub catalog_version: i64,
    pub template_count: i64,
    /// Outlook worker the app will run (packaged exe next to the app, or Python fallback), or the error.
    pub worker: String,
}

#[tauri::command]
pub async fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppResult<AppInfo> {
    let worker = match sync::worker_config(&state) {
        Ok(w) if w.args.is_empty() => w.program.display().to_string(),
        Ok(w) => format!("{} {}", w.program.display(), w.args.join(" ")),
        Err(e) => format!("not found: {e}"),
    };
    let conn = state.db.lock().unwrap();
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        data_dir: state.data_dir.to_string_lossy().to_string(),
        db_path: state.data_dir.join("support_desk.db").to_string_lossy().to_string(),
        log_dir: state.data_dir.join("logs").to_string_lossy().to_string(),
        startup_ms: state.startup_ms.load(std::sync::atomic::Ordering::Relaxed),
        schema_version: crate::db::schema_version(&conn)?,
        catalog_version: catalog().catalog_version,
        template_count: conn.query_row("SELECT COUNT(*) FROM email_templates", [], |r| r.get(0))?,
        worker,
    })
}

// ------------------------------------------------------------------ staff (Settings > Staff Assignment)

#[tauri::command]
pub async fn staff_list(state: State<'_, AppState>, include_inactive: bool) -> AppResult<Vec<crate::staff::Staff>> {
    crate::staff::list(&state.db.lock().unwrap(), include_inactive)
}

#[tauri::command]
pub async fn staff_save(state: State<'_, AppState>, input: crate::staff::StaffInput) -> AppResult<crate::staff::Staff> {
    let conn = state.db.lock().unwrap();
    let st = crate::staff::save(&conn, &input)?;
    // ownership is inferred from staff folders: re-evaluate with the new mapping (no notifications)
    crate::scope::recompute_all(&conn, "system")?;
    drop(conn);
    sync::nudge_shared(&state);
    Ok(st)
}

#[tauri::command]
pub async fn staff_set_active(state: State<'_, AppState>, id: i64, active: bool) -> AppResult<crate::staff::Staff> {
    crate::staff::set_active(&state.db.lock().unwrap(), id, active)
}

#[tauri::command]
pub async fn staff_current_get(state: State<'_, AppState>) -> AppResult<Option<i64>> {
    crate::staff::current_staff_id(&state.db.lock().unwrap())
}

#[tauri::command]
pub async fn staff_current_set(state: State<'_, AppState>, id: Option<i64>) -> AppResult<()> {
    crate::staff::set_current_staff_id(&state.db.lock().unwrap(), id)
}

#[tauri::command]
pub async fn outlook_list_accounts(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let cfg = sync::worker_config(&state)?;
    sync::call_with_retry(&cfg, "list_accounts", serde_json::json!({}), Duration::from_secs(60), 1).await
}

// ------------------------------------------------------------------ assignment

#[tauri::command]
pub async fn assign_preview(state: State<'_, AppState>, ticket_ids: Vec<i64>, staff_id: i64) -> AppResult<Vec<crate::assignment::AssignPreview>> {
    let conn = state.db.lock().unwrap();
    ticket_ids.iter().map(|id| crate::assignment::preview(&conn, *id, staff_id)).collect()
}

#[derive(Serialize, Clone)]
pub struct AssignOutcome {
    pub ticket_id: i64,
    pub display_id: String,
    /// "assigned" | "no_change" | "moves_failed" | "blocked" | "busy"
    pub result: String,
    pub message: String,
    pub failures: Vec<crate::assignment::MoveFailure>,
    pub op_id: Option<i64>,
    /// "pending" | "submitted" | "failed" | "uncertain" | "skipped" | "none"
    pub notify_state: String,
    pub notify_error: Option<String>,
}

fn outcome(id: i64, result: &str, message: impl Into<String>) -> AssignOutcome {
    AssignOutcome { ticket_id: id, display_id: tickets::display_id(id), result: result.into(), message: message.into(), failures: vec![], op_id: None, notify_state: "none".into(), notify_error: None }
}

/// Move the pending emails of an assignment; returns (finalized, failures).
async fn run_moves(state: &AppState, op_id: i64) -> AppResult<(bool, Vec<crate::assignment::MoveFailure>)> {
    use crate::assignment as a;
    let (op, pending) = {
        let conn = state.db.lock().unwrap();
        (a::get_op(&conn, op_id)?, a::pending_moves(&conn, op_id)?)
    };
    if pending.is_empty() {
        return Ok((a::finalize_if_complete(&state.db.lock().unwrap(), op_id)?, vec![]));
    }
    let s = settings(state);
    a::record_attempt(&state.db.lock().unwrap(), op_id, &pending)?;
    let items: Vec<serde_json::Value> = pending
        .iter()
        .map(|m| serde_json::json!({"message_id": m.message_id, "entry_id": m.entry_id, "store_id": m.store_id, "imid": m.imid,
            "folders": candidate_folders(state, &s, &m.store_id, &m.folder_entry_id)}))
        .collect();
    let cfg = sync::worker_config(state)?;
    let params = serde_json::json!({"dest": {"store_id": op.dest_store_id, "entry_id": op.dest_folder_entry_id}, "items": items});
    // no automatic retries: results must be recorded before anything is attempted again
    let res = crate::outlook::call(&cfg, "move_messages", params, Duration::from_secs(300)).await.and_then(crate::outlook::into_result);
    let current = crate::scope::cfg(&settings(state));
    let conn = state.db.lock().unwrap();
    match res {
        Ok(data) => {
            let results: Vec<a::MoveResult> = serde_json::from_value(data.get("items").cloned().unwrap_or_default())?;
            let failures = a::apply_moves(&conn, op_id, &current, &pending, &results)?;
            Ok((a::get_op(&conn, op_id)?.state == "done", failures))
        }
        Err(e) => {
            a::record_worker_failure(&conn, op_id, &pending, &e.to_string())?;
            Ok((false, pending.iter().map(|m| a::MoveFailure { message_id: m.message_id, subject: m.subject.clone(), error: e.to_string() }).collect()))
        }
    }
}

/// Send the handoff notification of a finished assignment (claimed atomically: never twice).
async fn send_notice(state: &AppState, op_id: i64) -> AppResult<(String, Option<String>)> {
    use crate::assignment::{self as a, NotifyOutcome};
    let n = {
        let conn = state.db.lock().unwrap();
        a::claim_notification(&conn, op_id)?;
        a::build_notification(&conn, op_id)?
    };
    let account = settings(state).notify_account;
    let params = serde_json::json!({"to": n.to, "subject": n.subject, "html_body": n.html_body, "account": account, "notification_id": n.notification_id});
    let outcome = match sync::worker_config(state) {
        Err(e) => NotifyOutcome::Failed(e.to_string()),
        Ok(cfg) => match crate::outlook::call(&cfg, "send_notification", params, Duration::from_secs(120)).await {
            Ok(resp) if resp.ok => NotifyOutcome::Submitted {
                entry_id: resp.data.get("draft_entry_id").and_then(|v| v.as_str()).map(String::from),
                account: resp.data.get("account").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            },
            Ok(resp) => {
                let msg = resp.message.clone().unwrap_or_else(|| "Outlook could not send the notification.".into());
                let stage = resp.diagnostics.get("stage").and_then(|v| v.as_str()).unwrap_or("");
                if resp.error_code.as_deref() == Some("SEND_UNCERTAIN") || stage == "send" { NotifyOutcome::Uncertain(msg) } else { NotifyOutcome::Failed(msg) }
            }
            // the worker never started: certainly not sent. Anything else (timeout, crash) may have happened after Send().
            Err(e) if e.code() == "WORKER_START_FAILED" => NotifyOutcome::Failed(e.to_string()),
            Err(e) => NotifyOutcome::Uncertain(format!("{e} The notification may or may not have been sent.")),
        },
    };
    let conn = state.db.lock().unwrap();
    a::record_notification(&conn, op_id, outcome)?;
    let op = a::get_op(&conn, op_id)?;
    Ok((op.notify_state, op.notify_error))
}

async fn run_assignment(state: &AppState, ticket_id: i64, staff_id: i64, note: &str) -> AssignOutcome {
    use crate::assignment::{self as a, Begin};
    let _g = match busy(&state.completing, ticket_id) {
        Ok(g) => g,
        Err(e) => return outcome(ticket_id, "busy", e.to_string()),
    };
    let operator = actor(state);
    let begin = a::begin(&state.db.lock().unwrap(), ticket_id, staff_id, &operator, note);
    let op_id = match begin {
        Err(e) => return outcome(ticket_id, "blocked", e.to_string()),
        Ok(Begin::NoChange) => return outcome(ticket_id, "no_change", "Already assigned to this person and the emails are already in their folder. No notification sent."),
        Ok(Begin::Op(op)) => op,
    };
    finish_assignment(state, ticket_id, op_id).await
}

async fn finish_assignment(state: &AppState, ticket_id: i64, op_id: i64) -> AssignOutcome {
    let mut o = outcome(ticket_id, "moves_failed", "");
    o.op_id = Some(op_id);
    match run_moves(state, op_id).await {
        Err(e) => {
            o.message = e.to_string();
            return o;
        }
        Ok((false, failures)) => {
            o.message = "Not all emails could be moved. The assignment is not finished and no notification was sent. Fix the problem and choose Retry.".into();
            o.failures = failures;
            return o;
        }
        Ok((true, _)) => {}
    }
    o.result = "assigned".into();
    let op = match crate::assignment::get_op(&state.db.lock().unwrap(), op_id) {
        Ok(op) => op,
        Err(e) => {
            o.message = e.to_string();
            return o;
        }
    };
    if op.notify_state == "pending" || op.notify_state == "failed" {
        match send_notice(state, op_id).await {
            Ok((st, err)) => {
                o.notify_state = st;
                o.notify_error = err;
            }
            Err(e) => {
                o.notify_state = "failed".into();
                o.notify_error = Some(e.to_string());
            }
        }
    } else {
        o.notify_state = op.notify_state.clone();
        o.notify_error = op.notify_error.clone();
    }
    o.message = match o.notify_state.as_str() {
        "submitted" => format!("Assigned to {}. Notification submitted to Outlook (delivery not confirmed).", op.to_name.unwrap_or_default()),
        "skipped" => "Emails moved; same person, so no notification.".into(),
        "uncertain" => "Assigned. The notification outcome is uncertain - check Sent Items before resending.".into(),
        _ => "Assigned — notification failed. Use Retry Notification.".into(),
    };
    o
}

/// Assign one or several tickets to one staff member. Each ticket is handled (and reported) independently.
#[tauri::command]
pub async fn tickets_assign(app: AppHandle, state: State<'_, AppState>, ticket_ids: Vec<i64>, staff_id: i64, note: Option<String>) -> AppResult<Vec<AssignOutcome>> {
    if shared_bound(&state) {
        return Err(AppError::coded("SHARED_ASSIGN_NOT_YET", "With the shared database, use Claim / Take over for now. Assign/Reassign with Outlook moves and handoff emails needs the shared reservation of moves (next phase), so two computers can never move or notify twice."));
    }
    let note = note.unwrap_or_default();
    let mut out = Vec::new();
    for id in ticket_ids {
        out.push(run_assignment(&state, id, staff_id, &note).await);
    }
    use tauri::Emitter;
    let _ = app.emit("tickets-changed", ());
    Ok(out)
}

/// Retry the newest unfinished assignment of a ticket: remaining moves first, then the notification.
#[tauri::command]
pub async fn assignment_retry(state: State<'_, AppState>, ticket_id: i64) -> AppResult<AssignOutcome> {
    let _g = busy(&state.completing, ticket_id)?;
    let op = crate::assignment::latest_op(&state.db.lock().unwrap(), ticket_id)?.ok_or_else(|| AppError::coded("NOT_FOUND", "No assignment to retry."))?;
    if op.state == "done" && !(op.notify_state == "failed" || op.notify_state == "pending") {
        return Err(AppError::coded("NOTHING_TO_RETRY", "This assignment is finished; there is nothing to retry."));
    }
    Ok(finish_assignment(&state, ticket_id, op.id).await)
}

/// Retry only the notification (emails are not moved again).
#[tauri::command]
pub async fn notification_retry(state: State<'_, AppState>, op_id: i64) -> AppResult<(String, Option<String>)> {
    let tid = crate::assignment::get_op(&state.db.lock().unwrap(), op_id)?.ticket_id;
    let _g = busy(&state.completing, tid)?;
    send_notice(&state, op_id).await
}

/// Uncertain outcome: look for the notification in Sent Items / Outbox of the sending account.
#[tauri::command]
pub async fn notification_check(state: State<'_, AppState>, op_id: i64) -> AppResult<String> {
    let op = crate::assignment::get_op(&state.db.lock().unwrap(), op_id)?;
    let cfg = sync::worker_config(&state)?;
    let account = op.notify_account.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| settings(&state).notify_account);
    let data = sync::call_with_retry(&cfg, "find_notification", serde_json::json!({"notification_id": op.notify_id, "account": account}), Duration::from_secs(60), 1).await?;
    let found = data.get("found").and_then(|v| v.as_bool()).unwrap_or(false);
    let place = data.get("where").and_then(|v| v.as_str()).map(String::from);
    let entry = data.get("entry_id").and_then(|v| v.as_str()).map(String::from);
    crate::assignment::record_check(&state.db.lock().unwrap(), op_id, if found { place.as_deref() } else { None }, entry.as_deref())
}

/// Uncertain outcome: staff explicitly confirm to send again.
#[tauri::command]
pub async fn notification_resend(state: State<'_, AppState>, op_id: i64) -> AppResult<(String, Option<String>)> {
    let tid = crate::assignment::get_op(&state.db.lock().unwrap(), op_id)?.ticket_id;
    let _g = busy(&state.completing, tid)?;
    crate::assignment::allow_resend(&state.db.lock().unwrap(), op_id, &actor(&state))?;
    send_notice(&state, op_id).await
}

#[tauri::command]
pub async fn ticket_start_work(state: State<'_, AppState>, id: i64) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    let t = crate::workflow::start_work(&state.db.lock().unwrap(), id, &a)?;
    sync::nudge_shared(&state);
    Ok(t)
}

// ------------------------------------------------------------------ scope status / overview

#[derive(Serialize)]
pub struct ScopeStatus {
    pub configured: bool,
    pub mailbox: String,
    pub summary: String,
    pub last_ok_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_error: Option<String>,
    /// the last successful check was made for another folder selection, or is older than 3 check intervals
    pub stale: bool,
    pub running: bool,
    pub current: i64,
    pub history: i64,
    pub unverified: i64,
}

#[tauri::command]
pub async fn scope_status(state: State<'_, AppState>) -> AppResult<ScopeStatus> {
    use crate::db::get_setting;
    let s = settings(&state);
    let conn = state.db.lock().unwrap();
    let (current, history, unverified) = crate::scope::counts(&conn)?;
    let last_ok = get_setting(&conn, "scope_last_ok_at")?.filter(|v| !v.is_empty());
    let key_ok = get_setting(&conn, "scope_last_key")?.unwrap_or_default() == crate::scope::cfg(&s).key;
    let old = last_ok
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) > chrono::Duration::minutes(3 * s.interval_minutes.max(5) as i64))
        .unwrap_or(true);
    Ok(ScopeStatus {
        configured: s.folder.is_some(),
        mailbox: s.mailbox_name.clone(),
        summary: if s.folder.is_some() { s.scan_summary() } else { "No folder selected".into() },
        stale: !key_ok || old,
        last_ok_at: last_ok,
        last_attempt_at: get_setting(&conn, "scope_last_attempt_at")?,
        last_error: get_setting(&conn, "scope_last_error")?.filter(|v| !v.is_empty()),
        running: state.sync.is_running(),
        current,
        history,
        unverified,
    })
}

// ------------------------------------------------------------------ shared database (Settings › Shared Database)

#[tauri::command]
pub async fn shared_status(state: State<'_, AppState>) -> AppResult<crate::shared_sync::SharedStatus> {
    let mut s = state.shared.snapshot();
    let conn = state.db.lock().unwrap();
    let st = settings(&state);
    s.mode = st.shared_mode.clone();
    s.path = st.shared_db_path.clone();
    if s.state.is_empty() {
        s.state = if st.shared_mode == "standalone" { "standalone".into() } else { "not_checked".into() };
    }
    s.queue = crate::sync_queue::summary(&conn)?;
    s.bound_database_id = crate::sync_queue::bound_db(&conn);
    s.bound_database_name = crate::db::get_setting(&conn, "shared_db_name")?;
    s.last_success_at = crate::db::get_setting(&conn, "shared_last_success_at")?;
    s.app_schema_version = crate::shared_store::SHARED_SCHEMA_VERSION;
    Ok(s)
}

/// Test Connection: reachable / readable / writable / identity / compatibility. Sends and receives nothing.
#[tauri::command]
pub async fn shared_test_connection(app: AppHandle) -> AppResult<crate::shared_sync::SharedStatus> {
    sync::shared_run(&app, "test", false).await
}

#[tauri::command]
pub async fn shared_sync_now(app: AppHandle) -> AppResult<crate::shared_sync::SharedStatus> {
    sync::shared_run(&app, "manual", true).await
}

/// Save the per-workstation mode and path. A different path never re-targets queued changes: the
/// identity check refuses a different database until staff explicitly switch.
#[tauri::command]
pub async fn shared_save_config(state: State<'_, AppState>, mode: String, path: String, minutes: Option<u32>) -> AppResult<Settings> {
    crate::settings::validate_shared(&mode, &path)?;
    let mut s = settings(&state);
    s.shared_mode = mode.clone();
    s.shared_db_path = path.trim().to_string();
    if let Some(m) = minutes {
        s.shared_sync_minutes = m;
    }
    s.validate()?;
    s.save(&state.settings_path)?;
    *state.settings.write().unwrap() = s.clone();
    {
        let conn = state.db.lock().unwrap();
        crate::sync_queue::set_mode(&conn, &mode)?;
        crate::diag::record(&conn, crate::diag::Severity::Info, "settings", "SHARED_CONFIG", &format!("Shared database settings saved: mode {mode}"), "", "");
        *state.shared.status.lock().unwrap() = crate::shared_sync::compose_status(&conn, &s.shared_mode, &s.shared_db_path, None, None);
    }
    sync::nudge_shared(&state);
    Ok(s)
}

/// Explicit review: submit changes that were made before the shared database identity was confirmed
/// (or that were held after restoring a backup).
#[tauri::command]
pub async fn shared_release_held(state: State<'_, AppState>) -> AppResult<usize> {
    let n = crate::sync_queue::release_held(&state.db.lock().unwrap())?;
    sync::nudge_shared(&state);
    Ok(n)
}

#[tauri::command]
pub async fn shared_discard_held(state: State<'_, AppState>) -> AppResult<usize> {
    crate::sync_queue::discard_held(&state.db.lock().unwrap())
}

/// Use the database now found at the configured path (identity changed). Requires a fresh check showing
/// that identity; changes bound to the old database are isolated, never sent to the new one.
#[tauri::command]
pub async fn shared_switch_database(state: State<'_, AppState>, database_id: String) -> AppResult<usize> {
    let snap = state.shared.snapshot();
    let name = match (&snap.database_id, &snap.database_name) {
        (Some(id), Some(n)) if *id == database_id => n.clone(),
        _ => return Err(AppError::coded("STALE", "Run Test Connection first; the database identity must be confirmed before switching.")),
    };
    let conn = state.db.lock().unwrap();
    let n = crate::sync_queue::rebind(&conn, &database_id, &name)?;
    crate::diag::record(&conn, crate::diag::Severity::Warning, "shared-db", "DB_SWITCHED", &format!("Switched to shared database \"{name}\"; {n} change(s) for the previous database were isolated (not sent)."), "", &database_id);
    Ok(n)
}

/// Import preview (dry run). With a reachable database it also counts cases that already exist there.
#[tauri::command]
pub async fn shared_import_preview(state: State<'_, AppState>) -> AppResult<crate::sync_queue::ImportPreview> {
    let (mut p, imids) = {
        let conn = state.db.lock().unwrap();
        (crate::sync_queue::import_preview(&conn)?, crate::sync_queue::local_only_message_ids(&conn)?)
    };
    let path = std::path::PathBuf::from(settings(&state).shared_db_path.trim());
    let r = tauri::async_runtime::spawn_blocking(move || -> crate::shared_store::SResult<i64> {
        let mut c = crate::shared_store::open(&path, true)?;
        crate::shared_store::identity(c.as_mut())?;
        crate::shared_sync::import_dry_run(c.as_mut(), &imids)
    });
    if let Ok(Ok(Ok(n))) = tokio::time::timeout(Duration::from_secs(120), r).await {
        p.already_shared = Some(n);
    }
    Ok(p)
}

/// Queue local-only cases for the shared database (restartable batches; duplicates are matched by the
/// shared database, ambiguous ones are kept separate for review).
#[tauri::command]
pub async fn shared_import_run(state: State<'_, AppState>, limit: Option<usize>) -> AppResult<usize> {
    let a = actor(&state);
    let n = {
        let conn = state.db.lock().unwrap();
        let n = crate::sync_queue::queue_import(&conn, &a, limit.unwrap_or(500).clamp(1, 5000))?;
        crate::diag::record(&conn, crate::diag::Severity::Info, "migration", "IMPORT_QUEUED", &format!("{n} local case(s) queued for the shared database"), "", "");
        n
    };
    sync::nudge_shared(&state);
    Ok(n)
}

/// Deliberate setup path: create a NEW shared database file (never overwrites; refuses existing files).
#[tauri::command]
pub async fn shared_create_database(state: State<'_, AppState>, path: String, name: String, environment: String) -> AppResult<crate::shared_store::DbIdentity> {
    crate::settings::validate_shared("shared", &path)?;
    if !["test", "production"].contains(&environment.as_str()) {
        return Err(AppError::coded("INVALID", "Environment must be test or production."));
    }
    let who = format!("{} ({})", actor(&state), crate::sync_queue::windows_account());
    let p = std::path::PathBuf::from(path.trim());
    let name2 = name.trim().to_string();
    let id = tauri::async_runtime::spawn_blocking(move || crate::shared_store::create_database(&p, &name2, &environment, &who))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    crate::diag::record(&state.db.lock().unwrap(), crate::diag::Severity::Info, "shared-db", "DB_CREATED", &format!("New shared database \"{}\" ({}) created", id.display_name, id.environment), "", &id.database_id);
    Ok(id)
}

// ------------------------------------------------------------------ notes / conflicts / diagnostics

#[tauri::command]
pub async fn notes_add(state: State<'_, AppState>, ticket_id: i64, body: String) -> AppResult<crate::notes::Note> {
    let a = actor(&state);
    let n = crate::notes::add(&state.db.lock().unwrap(), ticket_id, &body, &a)?;
    sync::nudge_shared(&state);
    Ok(n)
}

#[tauri::command]
pub async fn notes_edit(state: State<'_, AppState>, note_id: String, body: String) -> AppResult<()> {
    let a = actor(&state);
    crate::notes::edit(&state.db.lock().unwrap(), &note_id, &body, &a)?;
    sync::nudge_shared(&state);
    Ok(())
}

#[tauri::command]
pub async fn notes_delete(state: State<'_, AppState>, note_id: String) -> AppResult<()> {
    let a = actor(&state);
    crate::notes::delete(&state.db.lock().unwrap(), &note_id, &a)?;
    sync::nudge_shared(&state);
    Ok(())
}

#[tauri::command]
pub async fn conflicts_list(state: State<'_, AppState>, ticket_id: Option<i64>, include_resolved: bool) -> AppResult<Vec<crate::sync_queue::LocalConflict>> {
    crate::sync_queue::conflicts(&state.db.lock().unwrap(), ticket_id, include_resolved)
}

/// Keep the shared value. The conflict stays visible ("resolving") until the shared database accepts it.
#[tauri::command]
pub async fn conflict_accept_shared(state: State<'_, AppState>, conflict_id: String) -> AppResult<()> {
    let a = actor(&state);
    crate::sync_queue::accept_shared(&state.db.lock().unwrap(), &conflict_id, &a)?;
    sync::nudge_shared(&state);
    Ok(())
}

#[tauri::command]
pub async fn diag_recent(state: State<'_, AppState>, min_severity: Option<String>, limit: Option<i64>) -> AppResult<Vec<crate::diag::DiagEvent>> {
    Ok(crate::diag::recent(&state.db.lock().unwrap(), min_severity.as_deref().unwrap_or("info"), limit.unwrap_or(500))?)
}

#[tauri::command]
pub async fn overview_get(state: State<'_, AppState>, done_from: Option<String>, done_to: Option<String>) -> AppResult<crate::overview::Overview> {
    let s = settings(&state);
    crate::overview::overview(&state.db.lock().unwrap(), &s, &done_from.unwrap_or_default(), &done_to.unwrap_or_default())
}
