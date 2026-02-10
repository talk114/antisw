use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use crate::modules::logger;
use chrono::Utc;

const VERSION_SERVER_URL: &str = "http://gravityland.vnoffice.io.vn/app.version";
const DOWNLOAD_URL: &str = "https://gravityland.vnoffice.io.vn";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_CHECK_INTERVAL_HOURS: u64 = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum VersionStatus {
    BelowMinimum,
    UpdateAvailable,
    UpToDate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub min_version: String,
    pub has_update: bool,
    pub download_url: String,
    pub version_status: VersionStatus,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSettings {
    pub auto_check: bool,
    pub last_check_time: u64,
    #[serde(default = "default_check_interval")]
    pub check_interval_hours: u64,
}

fn default_check_interval() -> u64 {
    DEFAULT_CHECK_INTERVAL_HOURS
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            last_check_time: 0,
            check_interval_hours: DEFAULT_CHECK_INTERVAL_HOURS,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ServerVersionInfo {
    min: String,
    latest: String,
}

/// Check for updates from custom server endpoint
pub async fn check_for_updates() -> Result<UpdateInfo, String> {
    let client = create_client().await?;

    logger::log_info("Checking for updates from custom server...");

    let response = client
        .get(VERSION_SERVER_URL)
        .send()
        .await
        .map_err(|e| format!("Request to version server failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Version server returned status: {}", response.status()));
    }

    let server_info: ServerVersionInfo = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse version info: {}", e))?;

    let current_version = CURRENT_VERSION.to_string();
    let min_version = server_info.min;
    let latest_version = server_info.latest;

    // Determine version status
    let (version_status, has_update) = if compare_versions(&min_version, &current_version) {
        // current < min: Force update required
        logger::log_warn(&format!(
            "Current version {} is below minimum required version {}",
            current_version, min_version
        ));
        (VersionStatus::BelowMinimum, true)
    } else if compare_versions(&latest_version, &current_version) {
        // min <= current < latest: Update available
        logger::log_info(&format!(
            "Update available: {} (Current: {})",
            latest_version, current_version
        ));
        (VersionStatus::UpdateAvailable, true)
    } else {
        // current >= latest: Up to date
        logger::log_info(&format!(
            "Up to date: {} (Latest: {})",
            current_version, latest_version
        ));
        (VersionStatus::UpToDate, false)
    };

    Ok(UpdateInfo {
        current_version,
        latest_version,
        min_version,
        has_update,
        download_url: DOWNLOAD_URL.to_string(),
        version_status,
        source: Some("Custom Server".to_string()),
    })
}

async fn create_client() -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent("Antigravity Switcher")
        .timeout(std::time::Duration::from_secs(10));

    // Load config to check for upstream proxy
    if let Ok(config) = crate::modules::config::load_app_config() {
        if config.proxy.upstream_proxy.enabled && !config.proxy.upstream_proxy.url.is_empty() {
            logger::log_info(&format!("Update checker using upstream proxy: {}", config.proxy.upstream_proxy.url));
            match reqwest::Proxy::all(&config.proxy.upstream_proxy.url) {
                Ok(proxy) => {
                    builder = builder.proxy(proxy);
                },
                Err(e) => {
                    logger::log_warn(&format!("Failed to parse proxy URL '{}': {}", config.proxy.upstream_proxy.url, e));
                }
            }
        }
    }

    builder.build().map_err(|e| format!("Failed to create HTTP client: {}", e))
}



/// Compare two semantic versions (e.g., "3.3.30" vs "3.3.29")
fn compare_versions(latest: &str, current: &str) -> bool {
    let parse_version = |v: &str| -> Vec<u32> {
        v.split('.')
            .filter_map(|s| s.parse::<u32>().ok())
            .collect()
    };

    let latest_parts = parse_version(latest);
    let current_parts = parse_version(current);

    for i in 0..latest_parts.len().max(current_parts.len()) {
        let latest_part = latest_parts.get(i).unwrap_or(&0);
        let current_part = current_parts.get(i).unwrap_or(&0);

        if latest_part > current_part {
            return true;
        } else if latest_part < current_part {
            return false; // e.g. local: 3.3.30, remote: 3.3.30 => false
        }
    }

    false
}

/// Check if enough time has passed since last check
pub fn should_check_for_updates(settings: &UpdateSettings) -> bool {
    if !settings.auto_check {
        return false;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let elapsed_hours = (now - settings.last_check_time) / 3600;
    let interval = if settings.check_interval_hours > 0 {
        settings.check_interval_hours
    } else {
        DEFAULT_CHECK_INTERVAL_HOURS
    };
    elapsed_hours >= interval
}

/// Load update settings from config file
pub fn load_update_settings() -> Result<UpdateSettings, String> {
    let data_dir = crate::modules::account::get_data_dir()
        .map_err(|e| format!("Failed to get data dir: {}", e))?;
    let settings_path = data_dir.join("update_settings.json");

    if !settings_path.exists() {
        return Ok(UpdateSettings::default());
    }

    let content = std::fs::read_to_string(&settings_path)
        .map_err(|e| format!("Failed to read settings file: {}", e))?;

    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse settings: {}", e))
}

/// Save update settings to config file
pub fn save_update_settings(settings: &UpdateSettings) -> Result<(), String> {
    let data_dir = crate::modules::account::get_data_dir()
        .map_err(|e| format!("Failed to get data dir: {}", e))?;
    let settings_path = data_dir.join("update_settings.json");

    let content = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    std::fs::write(&settings_path, content)
        .map_err(|e| format!("Failed to write settings file: {}", e))
}

/// Update last check time
pub fn update_last_check_time() -> Result<(), String> {
    let mut settings = load_update_settings()?;
    settings.last_check_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    save_update_settings(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_versions() {
        // Test that newer version is detected
        assert!(compare_versions("3.3.36", "3.3.35"));
        assert!(compare_versions("3.4.0", "3.3.35"));
        assert!(compare_versions("4.0.3", "3.3.35"));
        
        // Test that older or equal versions are not detected as updates
        assert!(!compare_versions("3.3.34", "3.3.35"));
        assert!(!compare_versions("3.3.35", "3.3.35"));
    }

    #[test]
    fn test_version_status_logic() {
        // Simulate version comparisons
        let current = "4.1.5";
        let min = "4.1.2";
        let latest = "4.1.12";

        // Test: current >= min (should not be BelowMinimum)
        assert!(!compare_versions(min, current));
        
        // Test: current < latest (should be UpdateAvailable)
        assert!(compare_versions(latest, current));

        // Test: current < min (should be BelowMinimum)
        let current_old = "4.1.1";
        assert!(compare_versions(min, current_old));

        // Test: current >= latest (should be UpToDate)
        let current_new = "4.1.12";
        assert!(!compare_versions(latest, current_new));
    }

    #[test]
    fn test_should_check_for_updates() {
        let mut settings = UpdateSettings::default();
        assert!(should_check_for_updates(&settings));

        settings.last_check_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(!should_check_for_updates(&settings));

        settings.auto_check = false;
        assert!(!should_check_for_updates(&settings));
    }
}
