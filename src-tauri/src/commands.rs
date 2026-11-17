//! Tauri command handlers (thin wrappers; logic lives in the domain modules).

use crate::catalog::{catalog, Catalog};
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
    tickets::create_manual(&state.db.lock().unwrap(), &input, &s, &actor(&state))
}

#[tauri::command]
pub async fn ticket_update(state: State<'_, AppState>, id: i64, patch: TicketPatch) -> AppResult<tickets::Ticket> {
    let a = actor(&state);
    tickets::update(&state.db.lock().unwrap(), id, &patch, &a)
}

#[tauri::command]
pub async fn ticket_set_field(state: State<'_, AppState>, id: i64, key: String, value: Option<String>, verified: bool) -> AppResult<TicketField> {
    let a = actor(&state);
    tickets::set_field(&state.db.lock().unwrap(), id, &key, value, verified, &a)
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
    let data = sync::call_with_retry(&cfg, "get_message", serde_json::json!({"entry_id": entry, "store_id": store}), Duration::from_secs(60), 2).await?;
    let body = data.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
    state.db.lock().unwrap().execute(
        "UPDATE email_messages SET body_text=?2, body_truncated=0 WHERE id=?1",
        rusqlite::params![message_id, body],
    )?;
    Ok(body)
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
pub async fn outlook_list_folders(state: State<'_, AppState>, store_id: Option<String>, shared_mailbox: Option<String>) -> AppResult<serde_json::Value> {
    let cfg = sync::worker_config(&state)?;
    sync::call_with_retry(
        &cfg,
        "list_folders",
        serde_json::json!({"store_id": store_id.unwrap_or_default(), "shared_mailbox": shared_mailbox.unwrap_or_default(), "max_depth": 6}),
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
    s.folder_display = state.settings.read().unwrap().folder.as_ref().map(|f| f.path.clone());
    Ok(s)
}

// ------------------------------------------------------------------ settings

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> AppResult<Settings> {
    Ok(settings(&state))
}

#[tauri::command]
pub async fn settings_save(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> AppResult<Settings> {
    settings.validate()?;
    let old = self::settings(&state);
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
pub async fn export_csv(state: State<'_, AppState>, path: String, group: Option<String>) -> AppResult<usize> {
    export::export_csv(&state.db.lock().unwrap(), &PathBuf::from(path), group.as_deref().unwrap_or("all"))
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
}

#[tauri::command]
pub async fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppResult<AppInfo> {
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
    })
}
