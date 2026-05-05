// Tauri commands for 9Router MITM proxy management (antigravity / Google Cloud Code)

use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock;

use crate::modules::nine_router_mitm::{
    NineRouterMitmManager, NineRouterMitmStatus, ANTIGRAVITY_MITM_HOSTS, resolve_server_path,
};

/// Tauri-managed state wrapping the MITM manager.
#[derive(Clone)]
pub struct NineRouterMitmState {
    pub manager: Arc<RwLock<NineRouterMitmManager>>,
}

impl NineRouterMitmState {
    pub fn new() -> Self {
        Self {
            manager: Arc::new(RwLock::new(NineRouterMitmManager::default())),
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Get current status of the 9Router MITM server.
#[tauri::command]
pub async fn nine_router_mitm_status(
    state: State<'_, NineRouterMitmState>,
) -> Result<NineRouterMitmStatus, String> {
    let mgr = state.manager.read().await;
    Ok(mgr.get_status().await)
}

/// Check if DNS redirect entries are active (non-commented) in hosts file.
#[tauri::command]
pub fn nine_router_mitm_hosts_active() -> bool {
    let result = crate::modules::hosts_redirect::has_hosts_entries();
    tracing::info!("[9ROUTER-MITM] nine_router_mitm_hosts_active: result={}", result);
    result
}

/// Check whether the 9Router MITM server.js can be found on this machine.
///
/// Returns the resolved path (if found) so the frontend can display it.
#[tauri::command]
pub fn nine_router_mitm_find_server() -> Option<String> {
    resolve_server_path().map(|p| p.to_string_lossy().into_owned())
}

/// Start the 9Router MITM server and enable DNS redirect for antigravity.
#[tauri::command]
pub async fn nine_router_mitm_start(
    state: State<'_, NineRouterMitmState>,
    apiKey: Option<String>,
    routerUrl: Option<String>,
    enableDns: Option<bool>,
    sudoPassword: Option<String>,
) -> Result<NineRouterMitmStatus, String> {
    // Re-create manager if a custom router URL was supplied
    {
        let mut mgr = state.manager.write().await;
        if let Some(url) = routerUrl {
            *mgr = NineRouterMitmManager::new(url);
        }
    }

    let mgr = state.manager.read().await;
    let pid = mgr.start(apiKey.as_deref().unwrap_or("")).await?;
    tracing::info!("[9ROUTER-MITM] MITM server started, PID={}", pid);

    // Optionally write DNS redirect entries (127.0.0.1 → cloudcode-pa.googleapis.com)
    if enableDns.unwrap_or(true) {
        match crate::modules::hosts_redirect::add_hosts_entries(
            "127.0.0.1",
            sudoPassword.as_deref(),
        ) {
            Ok(_) => tracing::info!(
                "[9ROUTER-MITM] DNS redirect active: {:?} → 127.0.0.1",
                ANTIGRAVITY_MITM_HOSTS
            ),
            Err(e) => tracing::warn!(
                "[9ROUTER-MITM] Failed to set DNS redirect (hosts file): {}. Run as admin to enable system-wide interception.",
                e
            ),
        }
    }

    Ok(mgr.get_status().await)
}

/// Stop the 9Router MITM server and remove DNS redirect entries.
#[tauri::command]
pub async fn nine_router_mitm_stop(
    state: State<'_, NineRouterMitmState>,
    removeDns: Option<bool>,
    sudoPassword: Option<String>,
) -> Result<NineRouterMitmStatus, String> {
    let mgr = state.manager.read().await;
    mgr.stop().await?;

    if removeDns.unwrap_or(true) {
        match crate::modules::hosts_redirect::remove_hosts_entries(sudoPassword.as_deref()) {
            Ok(_) => tracing::info!("[9ROUTER-MITM] DNS redirect removed"),
            Err(e) => tracing::warn!("[9ROUTER-MITM] Failed to remove DNS redirect: {}", e),
        }
    }

    Ok(mgr.get_status().await)
}
