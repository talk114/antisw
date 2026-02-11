use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;
use chrono::Utc;

const DEVICE_ID_FILE: &str = "device_id.txt";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrackingEvent {
    pub device_id: String,
    pub machine_name: String,
    pub os: String,
    pub app_version: String,
    pub user_id: String,
    pub event_name: String,
    pub event_time: String,
    pub metadata: serde_json::Value,
}

/// Get or generate device ID (persistent across app restarts)
pub fn get_device_id() -> Result<String, String> {
    let data_dir = super::account::get_data_dir()?;
    let device_id_path = data_dir.join(DEVICE_ID_FILE);

    // Try to read existing device ID
    if device_id_path.exists() {
        if let Ok(id) = fs::read_to_string(&device_id_path) {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return Ok(id);
            }
        }
    }

    // Generate new device ID
    let new_id = Uuid::new_v4().to_string();
    fs::write(&device_id_path, &new_id)
        .map_err(|e| format!("Failed to save device ID: {}", e))?;
    
    Ok(new_id)
}

/// Get machine name (hostname)
pub fn get_machine_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Get OS identifier
pub fn get_os() -> String {
    #[cfg(target_os = "macos")]
    return "darwin".to_string();
    
    #[cfg(target_os = "windows")]
    return "win32".to_string();
    
    #[cfg(target_os = "linux")]
    return "linux".to_string();
    
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return "unknown".to_string();
}

/// Get app version
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Send tracking event to server
pub async fn send_tracking_event(event: TrackingEvent) -> Result<(), String> {
    // Check if tracking is enabled in config
    let config = super::config::load_app_config().map_err(|e| format!("Failed to load config: {}", e))?;
    
    // If tracking is disabled, return early
    if !config.tracking_enabled {
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .post("https://gravityland.vnoffice.io.vn/api/tracking")
        .header("Content-Type", "application/json")
        .json(&event)
        .send()
        .await
        .map_err(|e| format!("Failed to send tracking event: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Tracking API returned error: {}", response.status()));
    }

    Ok(())
}

/// Create a tracking event with common fields pre-filled
pub fn create_event(
    event_name: &str,
    user_id: Option<&str>,
    metadata: serde_json::Value,
) -> Result<TrackingEvent, String> {
    Ok(TrackingEvent {
        device_id: get_device_id()?,
        machine_name: get_machine_name(),
        os: get_os(),
        app_version: get_app_version(),
        user_id: user_id.unwrap_or("anonymous").to_string(),
        event_name: event_name.to_string(),
        event_time: Utc::now().to_rfc3339(),
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_id_generation() {
        let id = get_device_id();
        assert!(id.is_ok());
        let id = id.unwrap();
        assert!(!id.is_empty());
        assert_eq!(id.len(), 36); // UUID format
    }

    #[test]
    fn test_machine_name() {
        let name = get_machine_name();
        assert!(!name.is_empty());
    }

    #[test]
    fn test_os_detection() {
        let os = get_os();
        assert!(["darwin", "win32", "linux", "unknown"].contains(&os.as_str()));
    }

    #[test]
    fn test_create_event() {
        let metadata = serde_json::json!({
            "screen": "dashboard",
            "button": null
        });
        
        let event = create_event("test_event", Some("user123"), metadata);
        assert!(event.is_ok());
        
        let event = event.unwrap();
        assert_eq!(event.event_name, "test_event");
        assert_eq!(event.user_id, "user123");
        assert!(!event.device_id.is_empty());
    }
}
