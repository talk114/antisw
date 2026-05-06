// Tauri commands for 9Router DNS redirect management (antigravity / Google Cloud Code)

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

/// Get current status of the 9Router DNS redirect.
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
    tracing::info!("[9ROUTER] nine_router_mitm_hosts_active: result={}", result);
    result
}

/// Check if the certificate is already installed in the system trust store.
#[tauri::command]
pub fn nine_router_mitm_cert_installed() -> bool {
    crate::modules::cert_install::is_cert_installed()
}

<<<<<<< Updated upstream
/// Start the 9Router MITM server and enable DNS redirect for antigravity.
/// Automatically trusts the MITM Root CA certificate if not already trusted.
=======
/// Enable DNS redirect and install SSL certificate for antigravity.
///
/// Sets /etc/hosts to redirect Google domains to the target server IP,
/// and installs the SSL certificate into the OS trust store.
>>>>>>> Stashed changes
#[tauri::command]
pub async fn nine_router_mitm_start(
    state: State<'_, NineRouterMitmState>,
    target_ip: Option<String>,
    enableDns: Option<bool>,
    installCert: Option<bool>,
    sudoPassword: Option<String>,
) -> Result<NineRouterMitmStatus, String> {
<<<<<<< Updated upstream
    // Step 1: Ensure CA certificate is trusted (auto-trust if not already)
    if !crate::modules::mitm_ca::is_ca_cert_trusted() {
        tracing::info!("[9ROUTER-MITM] CA certificate not trusted, attempting to trust...");
        match crate::modules::mitm_ca::trust_ca_cert() {
            Ok(_) => {
                tracing::info!("[9ROUTER-MITM] CA certificate trusted successfully");
            }
            Err(e) => {
                tracing::warn!("[9ROUTER-MITM] Failed to trust CA certificate: {}", e);
                // Return error so frontend can show message to user
                return Err(format!(
                    "需要管理员权限信任 MITM CA 证书: {}\n\n请在弹出的认证对话框中输入管理员密码。",
                    e
                ));
            }
        }
    } else {
        tracing::info!("[9ROUTER-MITM] CA certificate already trusted");
    }

    // Step 2: Re-create manager if a custom router URL was supplied
=======
    let ip = target_ip.unwrap_or_else(|| DEFAULT_TARGET_IP.to_string());

    // Re-create manager with custom target IP if provided
>>>>>>> Stashed changes
    {
        let mut mgr = state.manager.write().await;
        *mgr = NineRouterMitmManager::new(&ip);
    }

    // Install certificate if requested (default: true)
    if installCert.unwrap_or(true) {
        match crate::modules::cert_install::install_cert(sudoPassword.as_deref()) {
            Ok(_) => tracing::info!("[9ROUTER] SSL certificate installed to system trust store"),
            Err(e) => tracing::warn!("[9ROUTER] Failed to install SSL certificate: {}. SSL errors may occur.", e),
        }
    }

    // Set DNS redirect entries if requested (default: true)
    if enableDns.unwrap_or(true) {
        match crate::modules::hosts_redirect::add_hosts_entries(&ip, sudoPassword.as_deref()) {
            Ok(_) => {
                tracing::info!(
                    "[9ROUTER] DNS redirect active: {:?} → {}",
                    ANTIGRAVITY_HOSTS,
                    ip
                );
                // Mark as active
                let mgr = state.manager.read().await;
                mgr.set_active(true).await;
            }
            Err(e) => {
                tracing::warn!(
                    "[9ROUTER] Failed to set DNS redirect (hosts file): {}. Run as admin to enable system-wide interception.",
                    e
                );
            }
        }
    }

    // Step 3: Start MITM server
    let mgr = state.manager.read().await;
<<<<<<< Updated upstream
    let pid = mgr.start(apiKey.as_deref().unwrap_or("")).await?;
    tracing::info!("[9ROUTER-MITM] MITM server started, PID={}", pid);

    // Step 4: Optionally write DNS redirect entries (127.0.0.1 → cloudcode-pa.googleapis.com)
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

=======
>>>>>>> Stashed changes
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
                tracing::info!("[9ROUTER] DNS redirect removed");
                let mgr = state.manager.read().await;
                mgr.set_active(false).await;
            }
            Err(e) => tracing::warn!("[9ROUTER] Failed to remove DNS redirect: {}", e),
        }
    }

    let mgr = state.manager.read().await;
    Ok(mgr.get_status().await)
<<<<<<< Updated upstream
}

/// Check if the MITM Root CA certificate is trusted by the system.
/// Returns true if the certificate is already trusted.
#[tauri::command]
pub fn nine_router_mitm_is_ca_cert_trusted() -> bool {
    crate::modules::mitm_ca::is_ca_cert_trusted()
}

/// Trust the MITM Root CA certificate by adding it to the system keychain.
/// This requires administrator privileges and will prompt the user for auth.
///
/// On macOS: shows native macOS authentication dialog
/// On Windows: shows UAC prompt for elevation
///
/// Returns Ok(true) on success, Err(message) on failure.
#[tauri::command]
pub fn nine_router_mitm_trust_ca_cert() -> Result<bool, String> {
    match crate::modules::mitm_ca::trust_ca_cert() {
        Ok(_) => {
            tracing::info!("[9ROUTER-MITM] CA certificate trusted successfully");
            Ok(true)
        }
        Err(e) => {
            tracing::warn!("[9ROUTER-MITM] Failed to trust CA certificate: {}", e);
            Err(e)
        }
    }
}
=======
}
>>>>>>> Stashed changes
