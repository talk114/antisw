// 9NICE DNS Redirect Manager for Antigravity (Google Cloud Code / Gemini API)
//
// Manages DNS redirect via /etc/hosts to point Google domains to the real server IP.
// No MITM proxy - requests go directly to the target server with proper SSL certificates.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Target server IP for DNS redirect
pub const DEFAULT_TARGET_IP: &str = "103.67.184.135";

/// Antigravity domains to redirect via hosts file
pub const ANTIGRAVITY_HOSTS: &[&str] = &[
    "daily-cloudcode-pa.googleapis.com",
    "cloudcode-pa.googleapis.com",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NineRouterMitmStatus {
    pub running: bool,
    pub target_ip: String,
    pub domains: Vec<String>,
}

pub struct NineRouterMitmManager {
    state: Arc<RwLock<ManagerState>>,
}

struct ManagerState {
    active: bool,
    target_ip: String,
}

impl NineRouterMitmManager {
    pub fn new(target_ip: impl Into<String>) -> Self {
        Self {
            state: Arc::new(RwLock::new(ManagerState {
                active: false,
                target_ip: target_ip.into(),
            })),
        }
    }
}

impl Default for NineRouterMitmManager {
    fn default() -> Self {
        Self::new(DEFAULT_TARGET_IP)
    }
}

impl NineRouterMitmManager {
    /// Mark the DNS redirect as active
    pub async fn set_active(&self, active: bool) {
        let mut state = self.state.write().await;
        state.active = active;
    }

    /// Check if DNS redirect is active
    pub async fn is_active(&self) -> bool {
        let state = self.state.read().await;
        state.active
    }

    /// Get current status
    pub async fn get_status(&self) -> NineRouterMitmStatus {
        let state = self.state.read().await;
        let active = crate::modules::hosts_redirect::has_hosts_entries();
        NineRouterMitmStatus {
            running: active,
            target_ip: state.target_ip.clone(),
            domains: ANTIGRAVITY_HOSTS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Get the target IP
    pub async fn get_target_ip(&self) -> String {
        let state = self.state.read().await;
        state.target_ip.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_target_ip() {
        let mgr = NineRouterMitmManager::default();
        assert_eq!(mgr.state.try_read().unwrap().target_ip, DEFAULT_TARGET_IP);
    }
}