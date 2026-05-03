// DNS Redirect Module - Modify /etc/hosts to redirect Google domains to VNPAY
// This enables system-wide MITM for all applications (browsers, IDEs, CLI)

use std::fs;
use std::path::Path;
use std::process::Command;

/// Hosts file path (platform-specific)
#[cfg(target_os = "macos")]
const HOSTS_PATH: &str = "/etc/hosts";

#[cfg(target_os = "linux")]
const HOSTS_PATH: &str = "/etc/hosts";

#[cfg(target_os = "windows")]
const HOSTS_PATH: &str = "C:\\Windows\\System32\\drivers\\etc\\hosts";

/// Marker comment to identify our entries
const MARKER_START: &str = "# ANTIGRAVITY-VNPAY-MITM-START";
const MARKER_END: &str = "# ANTIGRAVITY-VNPAY-MITM-END";

/// Target IP (VNPAY fixed IP)
const VNPAY_TARGET_IP: &str = "103.67.184.135";

/// Domains to redirect from Google to VNPAY
const REDIRECT_DOMAINS: &[&str] = &[
    "daily-cloudcode-pa.googleapis.com",
    "daily-cloudcode-pa.sandbox.googleapis.com",
    "cloudcode-pa.googleapis.com",
    "generativelanguage.googleapis.com",
    "generative-ai.googleapis.com",
];

/// Get the IP address of genai.vnpay.vn
pub async fn resolve_vnpay_ip() -> String {
    // Use fixed IP instead of DNS resolution
    VNPAY_TARGET_IP.to_string()
}

/// Generate hosts file entries
fn generate_hosts_entries(target_ip: &str) -> String {
    let mut entries = String::new();
    entries.push_str(&format!("{}\n", MARKER_START));
    for domain in REDIRECT_DOMAINS {
        entries.push_str(&format!("{} {}\n", target_ip, domain));
    }
    entries.push_str(&format!("{}\n", MARKER_END));
    entries
}

/// Check if our entries exist in hosts file
pub fn has_hosts_entries() -> bool {
    if !Path::new(HOSTS_PATH).exists() {
        return false;
    }
    match fs::read_to_string(HOSTS_PATH) {
        Ok(content) => content.contains(MARKER_START) && content.contains(MARKER_END),
        Err(_) => false,
    }
}

/// Build new hosts file content with our entries removed
fn strip_existing_entries(content: &str) -> String {
    let mut new_content = String::new();
    let mut inside_block = false;

    for line in content.lines() {
        if line.trim() == MARKER_START {
            inside_block = true;
            continue;
        }
        if line.trim() == MARKER_END {
            inside_block = false;
            continue;
        }
        if !inside_block {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }
    new_content
}

/// Build the complete hosts file content (existing + our entries)
fn build_new_hosts_content(target_ip: &str) -> Result<String, String> {
    let existing = if Path::new(HOSTS_PATH).exists() {
        fs::read_to_string(HOSTS_PATH)
            .map_err(|e| format!("Failed to read hosts file: {}", e))?
    } else {
        String::new()
    };

    let stripped = strip_existing_entries(&existing);
    let entries = generate_hosts_entries(target_ip);

    let mut result = stripped;
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&entries);
    Ok(result)
}

/// Build hosts content with our entries removed (for disable)
fn build_clean_hosts_content() -> Result<String, String> {
    if !Path::new(HOSTS_PATH).exists() {
        return Ok(String::new());
    }
    let existing = fs::read_to_string(HOSTS_PATH)
        .map_err(|e| format!("Failed to read hosts file: {}", e))?;
    Ok(strip_existing_entries(&existing))
}

/// Write hosts file with elevated privileges (cross-platform)
fn write_hosts_with_privileges(new_content: &str) -> Result<(), String> {
    // Write the new content to a temp file first
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join("antigravity_hosts_new.tmp");

    fs::write(&temp_path, new_content)
        .map_err(|e| format!("Failed to write temp hosts file: {}", e))?;

    let temp_str = temp_path.to_string_lossy().to_string();

    // Try direct write first (in case running as root/admin)
    if fs::write(HOSTS_PATH, new_content).is_ok() {
        let _ = fs::remove_file(&temp_path);
        tracing::info!("[VNPAY-HOSTS] Hosts file written directly (already had permissions)");
        return Ok(());
    }

    // Need elevated privileges
    let result = elevate_and_copy(&temp_str, HOSTS_PATH);
    let _ = fs::remove_file(&temp_path);
    result
}

/// Platform-specific privilege elevation to copy temp file to hosts
#[cfg(target_os = "macos")]
fn elevate_and_copy(src: &str, dst: &str) -> Result<(), String> {
    // Use osascript to prompt for admin password
    let script = format!(
        "do shell script \"cp '{}' '{}' && chmod 644 '{}' && dscacheutil -flushcache && killall -HUP mDNSResponder\" with administrator privileges with prompt \"Antigravity needs to update /etc/hosts to redirect API traffic to VNPAY.\"",
        src.replace('\'', "'\\''"),
        dst.replace('\'', "'\\''"),
        dst.replace('\'', "'\\''")
    );

    tracing::info!("[VNPAY-HOSTS] Requesting admin privileges via osascript...");

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Failed to invoke osascript: {}", e))?;

    if output.status.success() {
        tracing::info!("[VNPAY-HOSTS] Hosts file updated with admin privileges");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Admin authorization failed or denied: {}", stderr))
    }
}

#[cfg(target_os = "linux")]
fn elevate_and_copy(src: &str, dst: &str) -> Result<(), String> {
    // Try pkexec first (graphical sudo prompt)
    let pkexec_result = Command::new("pkexec")
        .arg("cp")
        .arg(src)
        .arg(dst)
        .output();

    if let Ok(output) = pkexec_result {
        if output.status.success() {
            tracing::info!("[VNPAY-HOSTS] Hosts file updated via pkexec");
            return Ok(());
        }
    }

    // Fall back to sudo (terminal prompt)
    let sudo_result = Command::new("sudo")
        .arg("cp")
        .arg(src)
        .arg(dst)
        .output()
        .map_err(|e| format!("Failed to invoke sudo: {}", e))?;

    if sudo_result.status.success() {
        tracing::info!("[VNPAY-HOSTS] Hosts file updated via sudo");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&sudo_result.stderr);
        Err(format!("sudo failed: {}", stderr))
    }
}

#[cfg(target_os = "windows")]
fn elevate_and_copy(src: &str, dst: &str) -> Result<(), String> {
    // Use PowerShell with Start-Process -Verb RunAs to trigger UAC prompt
    let ps_command = format!(
        "Start-Process -FilePath cmd.exe -ArgumentList '/c copy /Y \"{}\" \"{}\"' -Verb RunAs -Wait",
        src.replace('"', "\\\""),
        dst.replace('"', "\\\"")
    );

    tracing::info!("[VNPAY-HOSTS] Requesting admin privileges via UAC...");

    let output = Command::new("powershell")
        .args(&["-NoProfile", "-Command", &ps_command])
        .output()
        .map_err(|e| format!("Failed to invoke powershell: {}", e))?;

    if output.status.success() {
        tracing::info!("[VNPAY-HOSTS] Hosts file updated with admin privileges");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("UAC elevation failed or denied: {}", stderr))
    }
}

/// Remove our entries from hosts file
pub fn remove_hosts_entries() -> Result<(), String> {
    if !Path::new(HOSTS_PATH).exists() {
        return Ok(());
    }

    let content = fs::read_to_string(HOSTS_PATH)
        .map_err(|e| format!("Failed to read hosts file: {}", e))?;

    if !content.contains(MARKER_START) {
        tracing::info!("[VNPAY-HOSTS] No entries to remove");
        return Ok(());
    }

    let new_content = build_clean_hosts_content()?;
    write_hosts_with_privileges(&new_content)?;

    tracing::info!("[VNPAY-HOSTS] Removed entries from hosts file");
    Ok(())
}

/// Add our entries to hosts file
pub fn add_hosts_entries(target_ip: &str) -> Result<(), String> {
    let new_content = build_new_hosts_content(target_ip)?;
    write_hosts_with_privileges(&new_content)?;

    tracing::info!(
        "[VNPAY-HOSTS] Added {} domains redirecting to {}",
        REDIRECT_DOMAINS.len(),
        target_ip
    );
    Ok(())
}

/// Get current status of VNPAY hosts redirect
pub fn get_hosts_status() -> (bool, Vec<String>) {
    let active = has_hosts_entries();
    let domains = if active {
        REDIRECT_DOMAINS.iter().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    };
    (active, domains)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_entries() {
        let entries = generate_hosts_entries("127.0.0.1");
        assert!(entries.contains("daily-cloudcode-pa.googleapis.com"));
        assert!(entries.contains("generativelanguage.googleapis.com"));
        assert!(entries.contains(MARKER_START));
        assert!(entries.contains(MARKER_END));
    }

    #[test]
    fn test_strip_existing_entries() {
        let content = "127.0.0.1 localhost\n# ANTIGRAVITY-VNPAY-MITM-START\n127.0.0.1 example.com\n# ANTIGRAVITY-VNPAY-MITM-END\n::1 localhost\n";
        let stripped = strip_existing_entries(content);
        assert!(stripped.contains("127.0.0.1 localhost"));
        assert!(stripped.contains("::1 localhost"));
        assert!(!stripped.contains("example.com"));
        assert!(!stripped.contains("ANTIGRAVITY-VNPAY"));
    }
}