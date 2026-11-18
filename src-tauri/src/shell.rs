//! Tauri desktop shell: state, tray, window handling, command registration.

use crate::{catalog, commands, db, settings, sync, templates_store};
use rusqlite::Connection;
use settings::Settings;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, RwLock};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

pub struct AppState {
    pub db: Mutex<Connection>,
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub data_dir: PathBuf,
    pub images_dir: PathBuf,
    pub resource_dir: Option<PathBuf>,
    /// Source-checkout worker folder; development builds only (release builds never depend on the source tree).
    pub dev_worker_dir: Option<PathBuf>,
    pub sync: sync::SyncController,
    pub startup_ms: AtomicU64,
    /// Tickets with a Mark Done run in progress (blocks repeated clicks).
    pub completing: Mutex<std::collections::HashSet<i64>>,
    /// Shared (Access) database synchronization: status, single-flight guard, wake signal.
    pub shared: crate::shared_sync::SharedController,
    pub app_version: String,
    _log_guard: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
}

fn init_logging(log_dir: &std::path::Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(log_dir).ok()?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("support-desk")
        .filename_suffix("log")
        .max_log_files(14)
        .build(log_dir)
        .ok()?;
    let (nb, guard) = tracing_appender::non_blocking(appender);
    let filter = tracing_subscriber::EnvFilter::try_from_env("ASR_DESK_LOG").unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(nb).with_ansi(false).try_init().ok()?;
    Some(guard)
}

/// Write embedded SOP images to disk so Outlook can attach them inline.
fn materialize_images(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in catalog::sop_images() {
        let p = dir.join(name);
        let same = std::fs::metadata(&p).map(|m| m.len() == bytes.len() as u64).unwrap_or(false);
        if !same {
            std::fs::write(&p, bytes)?;
        }
    }
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Support Desk", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", "Check Outlook Now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &check, &quit])?;
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("ASR/Audit Support Desk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main(app),
            "check" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = sync::run_once(&app, "tray").await;
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let started = std::time::Instant::now();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let guard = init_logging(&data_dir.join("logs"));
            tracing::info!(version = %app.package_info().version, "starting ASR/Audit Support Desk");

            let settings_path = data_dir.join("settings.json");
            let settings = Settings::load(&settings_path);
            let conn = db::open(&data_dir.join("support_desk.db")).map_err(|e| e.to_string())?;
            templates_store::seed_embedded(&conn).map_err(|e| e.to_string())?;
            // core functions read the mode from SQLite (settings.json is the source)
            crate::sync_queue::set_mode(&conn, &settings.shared_mode).map_err(|e| e.to_string())?;
            let version = app.package_info().version.to_string();
            crate::diag::record(&conn, crate::diag::Severity::Info, "startup", "APP_START",
                &format!("ASR/Audit Support Desk {version} started; local schema v{}; shared mode {}", db::schema_version(&conn).unwrap_or(0), settings.shared_mode), "", "");
            // pre-select "who am I" from the Windows account when nothing is chosen yet
            if crate::staff::current_staff_id(&conn).ok().flatten().is_none() {
                if let Ok(Some(id)) = crate::staff::match_windows_account(&conn, &crate::sync_queue::windows_account()) {
                    let _ = crate::staff::set_current_staff_id(&conn, Some(id));
                }
            }
            let images_dir = data_dir.join("sop-images");
            if let Err(e) = materialize_images(&images_dir) {
                tracing::warn!(error = %e, "could not write SOP images");
            }
            let resource_dir = app.path().resource_dir().ok();
            let dev_worker_dir: Option<PathBuf> =
                if cfg!(debug_assertions) { Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../outlook_worker")) } else { None };
            match crate::outlook::WorkerConfig::resolve(&settings.python_path, resource_dir.as_deref(), dev_worker_dir.as_deref()) {
                Ok(w) => tracing::info!(app_dir = ?resource_dir, worker = %w.program.display(), args = w.args.len(), "outlook worker resolved"),
                Err(e) => tracing::warn!(app_dir = ?resource_dir, error = %e, "outlook worker not found"),
            }
            let close_to_tray = settings.close_to_tray;
            app.manage(AppState {
                db: Mutex::new(conn),
                settings: RwLock::new(settings),
                settings_path,
                data_dir,
                images_dir,
                resource_dir,
                dev_worker_dir,
                sync: sync::SyncController::default(),
                startup_ms: AtomicU64::new(0),
                completing: Mutex::new(Default::default()),
                shared: Default::default(),
                app_version: version,
                _log_guard: Mutex::new(guard),
            });
            if let Err(e) = setup_tray(app) {
                tracing::warn!(error = %e, "tray icon unavailable");
            }
            if std::env::args().any(|a| a == "--minimized") && close_to_tray {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            sync::spawn_scheduler(app.handle().clone());
            sync::spawn_shared_scheduler(app.handle().clone());
            let ms = started.elapsed().as_millis() as u64;
            app.state::<AppState>().startup_ms.store(ms, std::sync::atomic::Ordering::Relaxed);
            tracing::info!(startup_ms = ms, "backend ready");
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if state.settings.read().unwrap().close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::catalog_get,
            commands::tickets_list,
            commands::ticket_detail,
            commands::ticket_create,
            commands::ticket_update,
            commands::ticket_set_field,
            commands::ticket_set_issues,
            commands::ticket_add_school,
            commands::ticket_remove_school,
            commands::ticket_reopen,
            commands::ticket_complete,
            commands::ticket_start_work,
            commands::ticket_claim,
            commands::ticket_release,
            commands::ticket_flag_review,
            commands::ticket_resume,
            commands::ticket_join,
            commands::ticket_archive_outlook,
            commands::conflict_retry,
            commands::staff_list,
            commands::staff_save,
            commands::staff_set_active,
            commands::staff_current_get,
            commands::staff_current_set,
            commands::outlook_list_accounts,
            commands::assign_preview,
            commands::tickets_assign,
            commands::assignment_retry,
            commands::notification_retry,
            commands::notification_check,
            commands::notification_resend,
            commands::scope_status,
            commands::overview_get,
            commands::prompt_get,
            commands::prompt_default_instructions,
            commands::prompt_copied,
            commands::outlook_open_reply,
            commands::outlook_open_new,
            commands::outlook_validate_folder,
            commands::message_refresh,
            commands::templates_list,
            commands::template_get,
            commands::template_save,
            commands::template_duplicate,
            commands::template_set_enabled,
            commands::template_restore_default,
            commands::template_versions,
            commands::template_preview,
            commands::draft_get,
            commands::draft_check,
            commands::draft_load_default,
            commands::draft_reset,
            commands::draft_generate,
            commands::draft_fill,
            commands::draft_save,
            commands::draft_versions,
            commands::draft_create_outlook,
            commands::outlook_test,
            commands::outlook_list_mailboxes,
            commands::outlook_list_folders,
            commands::sync_now,
            commands::sync_status,
            commands::settings_get,
            commands::settings_save,
            commands::report_get,
            commands::export_csv,
            commands::db_backup,
            commands::db_restore,
            commands::app_info,
            commands::shared_status,
            commands::shared_test_connection,
            commands::shared_sync_now,
            commands::shared_save_config,
            commands::shared_release_held,
            commands::shared_discard_held,
            commands::shared_switch_database,
            commands::shared_import_preview,
            commands::shared_import_run,
            commands::shared_create_database,
            commands::notes_add,
            commands::notes_edit,
            commands::notes_delete,
            commands::conflicts_list,
            commands::conflict_accept_shared,
            commands::diag_recent,
            commands::diag_overview,
            commands::diag_copy_text,
            commands::diag_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ASR/Audit Support Desk");
}
