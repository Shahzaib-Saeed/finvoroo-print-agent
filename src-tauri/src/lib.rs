//! Finvoroo Print Agent — local silent printing for Finvoroo ERP.
//!
//! Architecture:
//! - Native tray app (Tauri)
//! - Localhost-only HTTP API for the React SPA
//! - Windows print backends: RAW/ZPL via the spooler, PDF via the printer driver
//! - macOS/Linux compile with a stub backend so the API can be developed off Windows

pub mod auth;
pub mod config;
pub mod pairing;
pub mod print;
pub mod server;

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PORT: u16 = 17392;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<tokio::sync::RwLock<config::AgentConfig>>,
    pub config_path: std::path::PathBuf,
    pub pairing: Arc<pairing::PairingStore>,
    pub log_path: std::path::PathBuf,
}

#[tauri::command]
async fn agent_status(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let cfg = state.config.read().await;
    Ok(serde_json::json!({
        "running": true,
        "version": VERSION,
        "previous_version": cfg.previous_version,
        "installed_version": cfg.installed_version,
        "port": cfg.port,
        "autostart": true,
        "platform": std::env::consts::OS,
        "has_token": !cfg.token.is_empty(),
    }))
}

#[tauri::command]
async fn agent_settings(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let cfg = state.config.read().await;
    Ok(serde_json::json!({
        "version": VERSION,
        "previous_version": cfg.previous_version,
        "installed_version": cfg.installed_version,
        "port": cfg.port,
        "token": cfg.token,
        "default_printer_id": cfg.default_printer_id,
        "bind": "127.0.0.1",
        "platform": std::env::consts::OS,
    }))
}

#[tauri::command]
async fn list_printers() -> Result<Vec<print::PrinterInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| print::list_printers().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_default_printer(
    state: tauri::State<'_, AppState>,
    printer_id: String,
) -> Result<(), String> {
    let mut cfg = state.config.write().await;
    cfg.default_printer_id = if printer_id.trim().is_empty() {
        None
    } else {
        Some(printer_id)
    };
    cfg.save(&state.config_path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn regenerate_token(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let mut cfg = state.config.write().await;
    cfg.token = auth::generate_token();
    cfg.paired_origin = None;
    cfg.paired_at = None;
    cfg.save(&state.config_path).map_err(|e| e.to_string())?;
    Ok(cfg.token.clone())
}

#[tauri::command]
async fn issue_pairing_code(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let code = state.pairing.issue();
    Ok(serde_json::json!({
        "code": code,
        "ttl_seconds": 60,
    }))
}

#[tauri::command]
async fn pairing_status(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let active = state.pairing.active();
    Ok(serde_json::json!({
        "code": active.as_ref().map(|(c, _)| c),
        "ttl_seconds": active.as_ref().map(|(_, t)| t),
    }))
}

#[tauri::command]
async fn test_print(state: tauri::State<'_, AppState>, printer_id: String) -> Result<(), String> {
    let fallback = state.config.read().await.default_printer_id.clone();
    let id = if printer_id.trim().is_empty() {
        fallback.unwrap_or_default()
    } else {
        printer_id
    };
    if id.trim().is_empty() {
        return Err("Select a printer first".into());
    }
    tauri::async_runtime::spawn_blocking(move || print::test_print(&id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

fn show_settings(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_settings(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .setup(|app| {
            let config_path = config::config_file_path(app.handle())?;
            let mut cfg = config::AgentConfig::load_or_create(&config_path)?;
            if cfg.apply_version_tracking(VERSION) {
                cfg.save(&config_path)?;
            }
            let first_run = cfg.first_run;
            let port = cfg.port;
            let log_path = app
                .path()
                .app_log_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("finvoroo-print-agent"))
                .join("print-agent.log");
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            let state = AppState {
                config: Arc::new(tokio::sync::RwLock::new(cfg)),
                config_path,
                pairing: Arc::new(pairing::PairingStore::default()),
                log_path,
            };
            app.manage(state.clone());

            #[cfg(windows)]
            if let Err(err) = print::init_html_engine() {
                tracing::error!("html print engine failed to start: {err:#}");
            }

            match app.handle().autolaunch().enable() {
                Ok(()) => tracing::info!("autostart enabled"),
                Err(err) => tracing::warn!("autostart could not be enabled: {err}"),
            }

            let open = MenuItem::with_id(app, "open", "Open settings", true, None::<&str>)?;
            let test = MenuItem::with_id(app, "test", "Test print", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Finvoroo Print Agent", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &test, &quit])?;

            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_settings(app),
                    "quit" => app.exit(0),
                    "test" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app.state::<AppState>();
                            let id = state
                                .config
                                .read()
                                .await
                                .default_printer_id
                                .clone()
                                .unwrap_or_default();
                            match tauri::async_runtime::spawn_blocking(move || print::test_print(&id))
                                .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(err)) => tracing::error!("tray test print failed: {err}"),
                                Err(err) => tracing::error!("tray test print failed: {err}"),
                            }
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_settings(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            let _tray = tray.build(app)?;

            if first_run {
                show_settings(app.handle());
            }

            let http_state = state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = server::serve(http_state, port).await {
                    tracing::error!("print agent HTTP API failed: {err:#}");
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            agent_status,
            agent_settings,
            list_printers,
            set_default_printer,
            regenerate_token,
            issue_pairing_code,
            pairing_status,
            test_print
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Finvoroo Print Agent");
}
