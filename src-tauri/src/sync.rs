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
    WorkerConfig::resolve(&s.python_path, state.resource_dir.as_deref(), &state.dev_worker_dir)
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
    if let Ok(o) = &result {
        if o.report.new_messages > 0 || o.report.moved > 0 {
            let _ = app.emit("tickets-changed", ());
        }
    }
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
    Ok(SyncOutcome { report, warnings, folders: reqs.len() })
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
                (s.interval_minutes.max(1), s.sync_enabled && s.folder.is_some(), s.folder.as_ref().map(|f| f.path.clone()))
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
