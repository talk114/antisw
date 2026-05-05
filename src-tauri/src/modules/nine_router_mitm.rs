// 9Router MITM Process Manager for Antigravity (Google Cloud Code / Gemini API)
//
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

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const DEFAULT_ROUTER_BASE: &str = "http://localhost:20128";

// Antigravity domains intercepted by the MITM server
pub const ANTIGRAVITY_MITM_HOSTS: &[&str] = &[
    "daily-cloudcode-pa.googleapis.com",
    "cloudcode-pa.googleapis.com",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NineRouterMitmStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub server_path: Option<String>,
    pub router_base: String,
}

struct MitmState {
    process: Option<Child>,
    pid: Option<u32>,
    server_path: Option<PathBuf>,
}

pub struct NineRouterMitmManager {
    state: Arc<RwLock<MitmState>>,
    router_base: String,
}

impl NineRouterMitmManager {
    pub fn new(router_base: impl Into<String>) -> Self {
        Self {
            state: Arc::new(RwLock::new(MitmState {
                process: None,
                pid: None,
                server_path: None,
            })),
            router_base: router_base.into(),
        }
    }
}

impl Default for NineRouterMitmManager {
    fn default() -> Self {
        Self::new(DEFAULT_ROUTER_BASE)
    }
}

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

impl NineRouterMitmManager {
    /// Start the 9router MITM server process.
    ///
    /// Returns the PID of the spawned process on success.
    /// Callers are responsible for setting up DNS redirect via `hosts_redirect`.
    pub async fn start(&self, api_key: &str) -> Result<u32, String> {
        let mut state = self.state.write().await;

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
    }

    /// Stop the 9router MITM server process.
    pub async fn stop(&self) -> Result<(), String> {
        let mut state = self.state.write().await;

        let pid = match state.pid {
            Some(p) => p,
            None => {
                tracing::info!("[9ROUTER-MITM] Not running, nothing to stop");
                return Ok(());
            }
        };

        if let Some(mut child) = state.process.take() {
            let _ = child.kill().await;
        }

        // Best-effort SIGKILL by PID if still alive
        if is_pid_alive(pid) {
            kill_pid(pid);
        }

        state.pid = None;
        state.server_path = None;
        tracing::info!("[9ROUTER-MITM] Stopped");
        Ok(())
    }

    /// Get current status of the MITM manager.
    pub async fn get_status(&self) -> NineRouterMitmStatus {
        let state = self.state.read().await;
        let alive = state.pid.map(is_pid_alive).unwrap_or(false);
        NineRouterMitmStatus {
            running: alive,
            pid: if alive { state.pid } else { None },
            server_path: state.server_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            router_base: self.router_base.clone(),
        }
    }

    /// Return true when the MITM server process is alive.
    pub async fn is_running(&self) -> bool {
        let state = self.state.read().await;
        state.pid.map(is_pid_alive).unwrap_or(false)
    }
}

// ── Platform helpers ──────────────────────────────────────────────────────────

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill -0 checks process existence without sending a signal
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_router_base() {
        let mgr = NineRouterMitmManager::default();
        assert_eq!(mgr.router_base, DEFAULT_ROUTER_BASE);
    }

    #[test]
    fn test_resolve_server_path_does_not_panic() {
        // Just ensure it doesn't panic; path may or may not exist in CI
        let _ = resolve_server_path();
    }
}
