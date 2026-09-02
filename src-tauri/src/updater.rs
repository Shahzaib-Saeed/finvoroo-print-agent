//! Signed auto-update for the tray agent.
//!
//! Checks the Tauri updater endpoint (see `tauri.conf.json` -> `plugins.updater`)
//! on startup and on a recurring interval, downloads + signature-verifies any
//! newer release in the background, waits for the printer to go idle, then
//! installs and restarts the tray process. Every failure (offline, no update,
//! bad signature) is logged and swallowed — this must never crash or block
//! the agent, which pharmacies rely on to keep printing.

use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::AppState;

/// Give the HTTP API + WebView2 print engine time to finish starting before
/// the first check competes with them for the network/CPU.
const STARTUP_DELAY: Duration = Duration::from_secs(30);
/// Long-running tray process: re-check periodically rather than only once.
const RECHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// A downloaded update is only installed once the agent has been idle (no
/// print job started) for at least this long.
const IDLE_BEFORE_INSTALL: Duration = Duration::from_secs(15);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Never postpone an already-verified update forever if the till is busy —
/// install anyway once this cap is hit.
const IDLE_MAX_WAIT: Duration = Duration::from_secs(10 * 60);

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            check_and_apply(&app).await;
            tokio::time::sleep(RECHECK_INTERVAL).await;
        }
    });
}

async fn check_and_apply(app: &AppHandle) {
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(err) => {
            tracing::warn!("updater not available: {err:#}");
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return,
        Err(err) => {
            // Offline / endpoint unreachable / no release yet — expected steady state
            // most of the time, so this stays at info level rather than error.
            tracing::info!("update check skipped: {err:#}");
            return;
        }
    };

    tracing::info!(
        current = %update.current_version,
        available = %update.version,
        "print agent update found, downloading"
    );

    // `download()` verifies the minisign signature before returning bytes —
    // nothing is ever installed unsigned.
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::error!("update download/verify failed: {err:#}");
            return;
        }
    };
    tracing::info!("print agent update downloaded and verified, waiting for an idle moment");

    if !wait_for_idle(app).await {
        tracing::warn!("printer stayed busy past the wait window; installing update anyway");
    }

    if let Err(err) = update.install(bytes) {
        tracing::error!("update install failed: {err:#}");
        return;
    }

    tracing::info!("print agent update installed, restarting");
    app.restart();
}

/// Reuses the same "when did a print job last start" clock the HTTP layer
/// updates on every `/print` request (`AppState::last_print_at`), so an
/// install never lands mid-job.
async fn wait_for_idle(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let deadline = tokio::time::Instant::now() + IDLE_MAX_WAIT;
    loop {
        let last = state.last_print_at.load(Ordering::Relaxed);
        let idle = last == 0 || now_millis().saturating_sub(last) >= IDLE_BEFORE_INSTALL.as_millis() as u64;
        if idle {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(IDLE_POLL_INTERVAL).await;
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
