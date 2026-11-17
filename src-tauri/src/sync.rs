//! Tauri-side scheduling: runs the sync job (logic in `sync_core`) and emits UI events.

pub use crate::sync_core::*;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::outlook::WorkerConfig;
use crate::AppState;
use chrono::{Duration as ChronoDuration, Utc};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub fn worker_config(state: &AppState) -> AppResult<WorkerConfig> {
    let s = state.settings.read().unwrap().clone();
    WorkerConfig::resolve(&s.python_path, state.resource_dir.as_deref(), state.dev_worker_dir.as_deref())
}

pub async fn run_once(app: &AppHandle, trigger: &str) -> AppResult<SyncOutcome> {
    let state = app.state::<AppState>();
    let Some(guard) = state.sync.try_start() else {
        return Err(AppError::coded("SYNC_RUNNING", "A synchronization is already running."));
    };
    let _ = app.emit("sync-status", state.sync.snapshot());
    let started = std::time::Instant::now();
    let result = run_inner(app, &state).await;
    {
        let mut st = state.sync.status.lock().unwrap();
        st.last_run_at = Some(now());
        match &result {
            Ok(o) => {
                st.last_success_at = st.last_run_at.clone();
                st.last_error = None;
                st.last_error_code = None;
                st.last_report = Some(o.report.clone());
                st.warnings = o.warnings.clone();
            }
            Err(e) => {
                st.last_error = Some(e.to_string());
                st.last_error_code = Some(e.code().to_string());
            }
        }
    }
    drop(guard);
    tracing::info!(trigger, ok = result.is_ok(), elapsed_ms = started.elapsed().as_millis() as u64, "sync finished");
    let _ = app.emit("sync-status", state.sync.snapshot());
    // the scan may have changed membership (verification) even without new emails
    let _ = app.emit("tickets-changed", ());
    result
}

async fn run_inner(app: &AppHandle, state: &AppState) -> AppResult<SyncOutcome> {
    let settings = state.settings.read().unwrap().clone();
    let (reqs, fulls) = build_requests(&state.db.lock().unwrap(), &settings, Utc::now())?;
    let cfg = worker_config(state)?;
    let data = call_with_retry(
        &cfg,
        "sync_messages",
        serde_json::json!({ "folders": reqs, "max_items": settings.max_messages_per_sync, "body_max_chars": settings.body_max_chars }),
        Duration::from_secs(300),
        3,
    )
    .await?;
    let results: Vec<FolderResult> = serde_json::from_value(data.get("folders").cloned().unwrap_or_default())?;
    let app2 = app.clone();
    let settings2 = settings.clone();
    // database work off the async executor
    let (report, warnings, changed) = tauri::async_runtime::spawn_blocking(move || {
        let state = app2.state::<AppState>();
        let conn = state.db.lock().unwrap();
        apply_results(&conn, &settings2, &fulls, &results)
    })
    .await
    .map_err(|e| AppError::msg(format!("sync task failed: {e}")))??;
    if let Some(ns) = changed {
        ns.save(&state.settings_path)?;
        *state.settings.write().unwrap() = ns;
    }
    let mut warnings = warnings;
    // verification of known emails is separate from importing (no date window / unread filter)
    match reconcile(state).await {
        Ok(r) => {
            if let Some(d) = r.discarded {
                warnings.push(d);
            } else if !r.complete {
                warnings.push("Some folders could not be checked completely; tickets were not moved to Ticket History. Check Now again later.".into());
            }
        }
        Err(e) => warnings.push(format!("Current Tickets could not be verified: {e}")),
    }
    Ok(SyncOutcome { report, warnings, folders: reqs.len() })
}

/// Candidate folders for finding an email whose EntryID changed: its last folder, the completion folder,
/// the monitored folders, every staff folder (with subfolders) and the whole tree under the top folder.
pub fn candidate_folders(s: &crate::settings::Settings, staff: &[crate::staff::Staff], own_store: &str, own_folder: &str) -> Vec<serde_json::Value> {
    let mut v = vec![serde_json::json!({"store_id": own_store, "entry_id": own_folder})];
    if let Some(a) = &s.archive_folder {
        v.push(serde_json::json!({"store_id": a.store_id, "entry_id": a.entry_id, "path": a.path}));
    }
    for (f, subs) in s.monitored() {
        v.push(serde_json::json!({"store_id": f.store_id, "entry_id": f.entry_id, "path": f.path, "recursive": subs}));
    }
    for st in staff {
        v.push(serde_json::json!({"store_id": st.store_id, "entry_id": st.folder_entry_id, "path": st.folder_path, "recursive": true}));
    }
    if let Some(top) = &s.folder {
        v.push(serde_json::json!({"store_id": top.store_id, "entry_id": top.entry_id, "path": top.path, "recursive": true}));
    }
    v
}

/// Verify which known emails are in the selected scope (complete inventory), then locate the ones that
/// left it. Nothing is removed unless the inventory was complete and still matches the selected folders.
pub async fn reconcile(state: &AppState) -> AppResult<crate::scope::ReconcileReport> {
    use crate::db::set_setting;
    let settings = state.settings.read().unwrap().clone();
    if settings.folder.is_none() {
        return Err(AppError::coded("NOT_CONFIGURED", "No Outlook folder is selected."));
    }
    let sc = crate::scope::cfg(&settings);
    let paths: std::collections::HashMap<String, String> = settings.monitored().into_iter().map(|(f, _)| (f.entry_id, f.path)).collect();
    let req = serde_json::json!({
        "scope_key": sc.key,
        "folders": sc.folders.iter().map(|(st, e, subs)| serde_json::json!({"store_id": st, "entry_id": e, "include_subfolders": subs, "path": paths.get(e).cloned().unwrap_or_default()})).collect::<Vec<_>>(),
        "exclude": sc.exclude,
    });
    set_setting(&state.db.lock().unwrap(), "scope_last_attempt_at", &now())?;
    let cfg = worker_config(state)?;
    let inv = match call_with_retry(&cfg, "scope_inventory", req, Duration::from_secs(600), 2).await.and_then(|d| Ok(serde_json::from_value::<crate::scope::Inventory>(d)?)) {
        Ok(i) => i,
        Err(e) => {
            set_setting(&state.db.lock().unwrap(), "scope_last_error", &e.to_string())?;
            return Err(e);
        }
    };
    // compare with the selection that is current NOW: a change during the check discards the result
    let current = crate::scope::cfg(&state.settings.read().unwrap().clone());
    let rep = crate::scope::apply_inventory(&state.db.lock().unwrap(), &current, &inv)?;
    if rep.applied && !rep.to_locate.is_empty() {
        let staff = crate::staff::list(&state.db.lock().unwrap(), true)?;
        let s2 = state.settings.read().unwrap().clone();
        // 1. one fast table read of each mailbox's whole Inbox tree + completion and staff folders
        let mut stores: Vec<String> = rep.to_locate.iter().map(|m| m.store_id.clone()).filter(|s| !s.is_empty()).collect();
        stores.extend(s2.folder.iter().map(|f| f.store_id.clone()));
        stores.sort();
        stores.dedup();
        let mut wide_folders: Vec<serde_json::Value> = stores.iter().map(|st| serde_json::json!({"store_id": st, "default_folder": 6, "include_subfolders": true, "path": "Inbox"})).collect();
        if let Some(a) = &s2.archive_folder {
            wide_folders.push(serde_json::json!({"store_id": a.store_id, "entry_id": a.entry_id, "include_subfolders": true, "path": a.path}));
        }
        for st in &staff {
            wide_folders.push(serde_json::json!({"store_id": st.store_id, "entry_id": st.folder_entry_id, "include_subfolders": true, "path": st.folder_path}));
        }
        let mut remaining = rep.to_locate.clone();
        match call_with_retry(&cfg, "scope_inventory", serde_json::json!({"scope_key": "wide", "folders": wide_folders, "exclude": []}), Duration::from_secs(600), 1).await {
            Ok(d) => {
                let wide: crate::scope::Inventory = serde_json::from_value(d)?;
                let (found, rest) = crate::scope::locate_from_inventory(&wide, &remaining);
                crate::scope::apply_locate(&state.db.lock().unwrap(), &current, &found)?;
                remaining = rest;
            }
            Err(e) => tracing::warn!(error = %e, "wide inventory failed; looking up emails one by one"),
        }
        // 2. what is still missing: per-email lookup (EntryID, then Message-ID in candidate folders)
        for chunk in remaining.chunks(50) {
            let items: Vec<serde_json::Value> = chunk
                .iter()
                .map(|m| serde_json::json!({"message_id": m.message_id, "entry_id": m.entry_id, "store_id": m.store_id, "imid": m.imid,
                    "folders": candidate_folders(&s2, &staff, &m.store_id, &m.folder_entry_id)}))
                .collect();
            match call_with_retry(&cfg, "locate_messages", serde_json::json!({"items": items}), Duration::from_secs(600), 1).await {
                Ok(d) => {
                    let results: Vec<crate::scope::LocateResult> = serde_json::from_value(d.get("items").cloned().unwrap_or_default())?;
                    crate::scope::apply_locate(&state.db.lock().unwrap(), &current, &results)?;
                }
                Err(e) => tracing::warn!(error = %e, "locating emails that left the scope failed; their location stays unverified"),
            }
        }
    }
    {
        let conn = state.db.lock().unwrap();
        if rep.applied && rep.complete {
            set_setting(&conn, "scope_last_ok_at", &now())?;
            set_setting(&conn, "scope_last_key", &current.key)?;
            set_setting(&conn, "scope_last_error", "")?;
        } else {
            let why = rep.discarded.clone().unwrap_or_else(|| format!("Incomplete check: {}", inv.errors.join("; ")));
            set_setting(&conn, "scope_last_error", &why)?;
        }
    }
    tracing::info!(applied = rep.applied, complete = rep.complete, in_scope = rep.in_scope, newly_out = rep.newly_out, current = rep.current, history = rep.history, unverified = rep.unverified, "scope reconciliation finished");
    Ok(rep)
}

/// Background scheduler: sleeps until the next due time; `wake` re-reads settings.
/// Idle cost is a single sleeping task (no polling).
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let startup = {
            let s = state.settings.read().unwrap();
            s.sync_enabled && s.check_on_startup && s.folder.is_some()
        };
        if startup {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Err(e) = run_once(&app, "startup").await {
                tracing::warn!(error = %e, "startup sync failed");
            }
        }
        loop {
            let (minutes, enabled, folder) = {
                let s = state.settings.read().unwrap();
                (s.interval_minutes.max(1), s.sync_enabled && s.folder.is_some(), s.folder.as_ref().map(|_| s.scan_summary()))
            };
            let due = Utc::now() + ChronoDuration::minutes(minutes as i64);
            {
                let mut st = state.sync.status.lock().unwrap();
                st.next_run_at = if enabled { Some(due.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)) } else { None };
                st.folder_display = folder;
            }
            let _ = app.emit("sync-status", state.sync.snapshot());
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(minutes as u64 * 60)) => {
                    if enabled {
                        if let Err(e) = run_once(&app, "schedule").await {
                            tracing::warn!(error = %e, "scheduled sync failed");
                        }
                    }
                }
                _ = state.sync.wake.notified() => { /* settings changed: recompute schedule */ }
            }
        }
    });
}
