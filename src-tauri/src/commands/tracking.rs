use crate::modules::tracking;
use serde_json::json;

/// Track an event
#[tauri::command]
pub async fn track_event(
    event_name: String,
    user_id: Option<String>,
    metadata: serde_json::Value,
) -> Result<(), String> {
    let event = tracking::create_event(
        &event_name,
        user_id.as_deref(),
        metadata,
    )?;
    
    // Send tracking event asynchronously (don't block on response)
    tauri::async_runtime::spawn(async move {
        if let Err(e) = tracking::send_tracking_event(event).await {
            tracing::warn!("Failed to send tracking event: {}", e);
        }
    });
    
    Ok(())
}

/// Get device ID
#[tauri::command]
pub fn get_tracking_device_id() -> Result<String, String> {
    tracking::get_device_id()
}

/// Get machine name
#[tauri::command]
pub fn get_tracking_machine_name() -> String {
    tracking::get_machine_name()
}

/// Get OS
#[tauri::command]
pub fn get_tracking_os() -> String {
    tracking::get_os()
}
