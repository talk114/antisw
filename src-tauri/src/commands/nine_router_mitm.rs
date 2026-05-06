// Tauri commands for 9NICE DNS redirect management (antigravity / Google Cloud Code)

use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock;

use crate::modules::nine_router_mitm::{
    NineRouterMitmManager, NineRouterMitmStatus, DEFAULT_TARGET_IP, ANTIGRAVITY_HOSTS,
};

/// Tauri-managed state wrapping the DNS redirect manager.
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

/// Get current status of the 9NICE DNS redirect.
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
    tracing::info!("[9NICE] nine_router_mitm_hosts_active: result={}", result);
    result
}

/// Check if the certificate is already installed in the system trust store.
#[tauri::command]
pub fn nine_router_mitm_cert_installed() -> bool {
    crate::modules::cert_install::is_cert_installed()
}

/// Enable DNS redirect and install SSL certificate for antigravity.
///
/// Sets /etc/hosts to redirect Google domains to the target server IP,
/// and installs the SSL certificate into the OS trust store.
#[tauri::command]
pub async fn nine_router_mitm_start(
    state: State<'_, NineRouterMitmState>,
    target_ip: Option<String>,
    enableDns: Option<bool>,
    installCert: Option<bool>,
    sudoPassword: Option<String>,
) -> Result<NineRouterMitmStatus, String> {
    let ip = target_ip.unwrap_or_else(|| DEFAULT_TARGET_IP.to_string());

    // Re-create manager with custom target IP if provided
    {
        let mut mgr = state.manager.write().await;
        *mgr = NineRouterMitmManager::new(&ip);
    }

    // Install certificate if requested (default: true)
    if installCert.unwrap_or(true) {
        match crate::modules::cert_install::install_cert(sudoPassword.as_deref()) {
            Ok(_) => tracing::info!("[9NICE] SSL certificate installed to system trust store"),
            Err(e) => tracing::warn!("[9NICE] Failed to install SSL certificate: {}. SSL errors may occur.", e),
        }
    }

    // Set DNS redirect entries if requested (default: true)
    if enableDns.unwrap_or(true) {
        match crate::modules::hosts_redirect::add_hosts_entries(&ip, sudoPassword.as_deref()) {
            Ok(_) => {
                tracing::info!(
                    "[9NICE] DNS redirect active: {:?} → {}",
                    ANTIGRAVITY_HOSTS,
                    ip
                );
                // Mark as active
                let mgr = state.manager.read().await;
                mgr.set_active(true).await;
            }
            Err(e) => {
                tracing::warn!(
                    "[9NICE] Failed to set DNS redirect (hosts file): {}. Run as admin to enable system-wide interception.",
                    e
                );
            }
        }
    }

    let mgr = state.manager.read().await;
    Ok(mgr.get_status().await)
}

/// Disable DNS redirect entries.
#[tauri::command]
pub async fn nine_router_mitm_stop(
    state: State<'_, NineRouterMitmState>,
    removeDns: Option<bool>,
    sudoPassword: Option<String>,
) -> Result<NineRouterMitmStatus, String> {
    if removeDns.unwrap_or(true) {
        match crate::modules::hosts_redirect::remove_hosts_entries(sudoPassword.as_deref()) {
            Ok(_) => {
                tracing::info!("[9NICE] DNS redirect removed");
                let mgr = state.manager.read().await;
                mgr.set_active(false).await;
            }
            Err(e) => tracing::warn!("[9NICE] Failed to remove DNS redirect: {}", e),
        }
    }

    let mgr = state.manager.read().await;
    Ok(mgr.get_status().await)
}
