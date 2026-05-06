// 9Router DNS Redirect Manager for Antigravity (Google Cloud Code / Gemini API)
//
<<<<<<< Updated upstream
// Spawns 9router's Node.js MITM server (src/mitm/server.cjs) which:
//  - Listens on port 443 locally with SSL termination
//  - Intercepts Gemini API requests (cloudcode-pa.googleapis.com)
//  - Forwards them to 9router at localhost:20128 for multi-provider routing
//
// DNS redirect (cloudcode-pa.googleapis.com → 127.0.0.1) is managed separately
// via hosts_redirect::add_hosts_entries("127.0.0.1").

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
=======
// Manages DNS redirect via /etc/hosts to point Google domains to the real server IP.
// No MITM proxy - requests go directly to the target server with proper SSL certificates.
>>>>>>> Stashed changes

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

<<<<<<< Updated upstream
/// Find the node binary on the system PATH.
fn find_node() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        for name in &["node.exe", "node"] {
            if let Ok(out) = std::process::Command::new("where").arg(name).output() {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout);
                    if let Some(line) = s.lines().next() {
                        return Some(PathBuf::from(line.trim()));
                    }
                }
            }
        }
    } else {
        for name in &["node", "nodejs"] {
            if let Ok(out) = std::process::Command::new("which").arg(name).output() {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout);
                    if let Some(line) = s.lines().next() {
                        let p = PathBuf::from(line.trim());
                        if p.exists() {
                            return Some(p);
                        }
                    }
                }
            }
        }
        // Common NVM / system paths
        let home = dirs::home_dir().unwrap_or_default();
        for candidate in &[
            home.join(".nvm/versions/node").as_path().to_path_buf(), // resolved below
            PathBuf::from("/usr/local/bin/node"),
            PathBuf::from("/usr/bin/node"),
            PathBuf::from("/opt/homebrew/bin/node"),
        ] {
            if candidate.exists() {
                return Some(candidate.clone());
            }
        }
    }
    None
}

/// Resolve the path to antisw's bundled MITM server.js.
///
/// Search order:
///  1. `MITM_SERVER_PATH` env var (explicit override)
///  2. Dev build: path embedded at compile time via CARGO_MANIFEST_DIR
///  3. Production: `<exe>/../mitm/server.cjs` bundled alongside executable
pub fn resolve_server_path() -> Option<PathBuf> {
    // 1. Explicit override
    if let Ok(p) = std::env::var("MITM_SERVER_PATH") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }

    // 2. Dev build: CARGO_MANIFEST_DIR is src-tauri/, mitm/ is sibling
    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("mitm/server.cjs");
    if dev_path.exists() {
        return Some(dev_path);
    }

    // 3. Production bundle
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let prod_path = parent.join("mitm/server.cjs");
            if prod_path.exists() {
                return Some(prod_path);
            }
        }
    }

    None
}

=======
>>>>>>> Stashed changes
impl NineRouterMitmManager {
    /// Mark the DNS redirect as active
    pub async fn set_active(&self, active: bool) {
        let mut state = self.state.write().await;
<<<<<<< Updated upstream

        // Already running?
        if let Some(pid) = state.pid {
            if is_pid_alive(pid) {
                return Err(format!("9Router MITM already running (PID {})", pid));
            }
            // Stale — clear state
            state.process = None;
            state.pid = None;
        }

        let server_path = resolve_server_path()
            .ok_or("MITM server.cjs not found. Set MITM_SERVER_PATH env var to point to src-tauri/mitm/server.cjs")?;

        let node = find_node()
            .ok_or("node binary not found — install Node.js to use 9Router MITM")?;

        tracing::info!(
            "[9ROUTER-MITM] Starting: {} {}",
            node.display(),
            server_path.display()
        );

        // Ensure Root CA + leaf certs exist before spawning
        crate::modules::mitm_ca::ensure_all_certs()
            .map_err(|e| format!("Failed to generate MITM certs: {}", e))?;

        let mut cmd = Command::new(&node);
        cmd.arg(&server_path)
            .env("MITM_ROUTER_BASE", &self.router_base)
            .env("ROUTER_API_KEY", api_key)
            .env("MITM_SERVER_PATH", &server_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn MITM server: {}", e))?;

        let pid = child.id().ok_or("Failed to get PID of spawned process")?;

        // Pipe stdout/stderr to tracing
        if let Some(stdout) = child.stdout.take() {
            let prefix = "[9ROUTER-MITM-OUT]";
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::info!("{} {}", prefix, line);
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let prefix = "[9ROUTER-MITM-ERR]";
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::warn!("{} {}", prefix, line);
                }
            });
        }

        state.pid = Some(pid);
        state.server_path = Some(server_path);
        state.process = Some(child);

        tracing::info!("[9ROUTER-MITM] Started with PID {}", pid);
        Ok(pid)
=======
        state.active = active;
>>>>>>> Stashed changes
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