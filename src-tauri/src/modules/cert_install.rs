// Certificate Installation Module
// Installs SSL certificates into the OS trust store for system-wide SSL trust

use std::path::Path;
use std::process::Command;

/// Path to the bundled certificate file
pub fn get_cert_path() -> Option<std::path::PathBuf> {
    // Dev build: CARGO_MANIFEST_DIR is src-tauri/, assets/ is sibling
    let dev_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/googleapis.crt");
    if dev_path.exists() {
        return Some(dev_path);
    }

    // Production bundle: <exe>/../assets/googleapis.crt
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let prod_path = parent.join("assets/googleapis.crt");
            if prod_path.exists() {
                return Some(prod_path);
            }
        }
    }

    None
}

/// Install certificate into macOS Keychain (system trust)
#[cfg(target_os = "macos")]
pub fn install_cert_to_keychain(cert_path: &Path, password: Option<&str>) -> Result<(), String> {
    if !cert_path.exists() {
        return Err(format!("Certificate not found: {}", cert_path.display()));
    }

    // Password is required - collected from frontend dialog
    let pwd = password.ok_or("Sudo password required. Please enter your computer password.")?;

    let cert_str = cert_path.to_string_lossy().to_string();

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "printf '%s\\n' '{}' | sudo -S security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain '{}' 2>&1",
            pwd.replace('\'', "'\\''"),
            cert_str.replace('\'', "'\\''")
        ))
        .output()
        .map_err(|e| format!("Failed to run sudo: {}", e))?;

    if status.status.success() {
        tracing::info!("[CERT] Certificate installed to System.keychain via sudo -S");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&status.stderr);
    // Check if already exists (not an error)
    if stderr.contains("already exists") || stderr.contains("CSSM_ERR_ADD_WILL_BE_CAUTION") {
        tracing::info!("[CERT] Certificate already trusted");
        return Ok(());
    }

    tracing::warn!("[CERT] sudo -S failed: {}", stderr);
    Err(format!("Failed to install certificate: {}", stderr))
}

/// Install certificate into Linux ca-certificates
#[cfg(target_os = "linux")]
pub fn install_cert_to_ca(cert_path: &Path, password: Option<&str>) -> Result<(), String> {
    if !cert_path.exists() {
        return Err(format!("Certificate not found: {}", cert_path.display()));
    }

    // Password is required - collected from frontend dialog
    let pwd = password.ok_or("Sudo password required. Please enter your computer password.")?;

    let cert_name = "googleapis.crt";
    let dest_dir = "/usr/local/share/ca-certificates";
    let src_str = cert_path.to_string_lossy().to_string();
    let dst_str = format!("{}/{}", dest_dir, cert_name);
    let cmd = format!("cp '{}' '{}' && update-ca-certificates", src_str, dst_str);

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "printf '%s\\n' '{}' | sudo -S sh -c '{}'",
            pwd.replace('\'', "'\\''"),
            cmd.replace('\'', "'\\''")
        ))
        .output()
        .map_err(|e| format!("Failed to run sudo: {}", e))?;

    if status.status.success() {
        tracing::info!("[CERT] Certificate installed via sudo -S");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&status.stderr);
    tracing::warn!("[CERT] sudo -S failed: {}", stderr);
    Err(format!("Failed to install certificate: {}", stderr))
}

/// Install certificate on Windows (into Root store)
/// Tries LocalMachine\Root first (requires admin), falls back to CurrentUser\Root (user-level)
#[cfg(target_os = "windows")]
pub fn install_cert_to_store(cert_path: &Path, _password: Option<&str>) -> Result<(), String> {
    if !cert_path.exists() {
        return Err(format!("Certificate not found: {}", cert_path.display()));
    }

    // Get absolute path to ensure correct path resolution
    let cert_abs_path = cert_path
        .canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cert_path.to_string_lossy().to_string());

    // Strip \\?\ prefix added by canonicalize() on Windows - causes issues with PowerShell Import-Certificate
    let cert_abs_path = cert_abs_path.strip_prefix("\\\\?\\").unwrap_or(&cert_abs_path);

    tracing::debug!("[CERT] Installing certificate from: {}", cert_abs_path);

    // First try LocalMachine\Root (requires admin privileges)
    let ps_command = format!(
        r#"Import-Certificate -FilePath "{}" -CertStoreLocation Cert:\LocalMachine\Root -Confirm:$false 2>&1 | Out-Null; if ($LASTEXITCODE -ne 0 -and $Error.Count -gt 0) {{ throw $Error[0] }}"#,
        cert_abs_path
    );

    let output = Command::new("powershell")
        .args(&["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_command])
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            tracing::debug!("[CERT] LocalMachine stdout: {}, stderr: {}", stdout, stderr);

            if out.status.success() && !stderr.to_lowercase().contains("exception") {
                tracing::info!("[CERT] Certificate installed to Windows LocalMachine\\Root (admin)");
                return Ok(());
            }
            tracing::warn!("[CERT] LocalMachine\\Root install failed (admin required): {}", stderr);
        }
        Err(e) => {
            tracing::warn!("[CERT] LocalMachine\\Root install failed: {}", e);
        }
    }

    // Fall back to CurrentUser\Root (no admin required, user-level trust)
    let ps_command = format!(
        r#"Import-Certificate -FilePath "{}" -CertStoreLocation Cert:\CurrentUser\Root -Confirm:$false 2>&1 | Out-Null; if ($LASTEXITCODE -ne 0 -and $Error.Count -gt 0) {{ throw $Error[0] }}"#,
        cert_abs_path
    );

    let output = Command::new("powershell")
        .args(&["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_command])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.to_lowercase().contains("exception") {
                tracing::info!("[CERT] Certificate installed to Windows CurrentUser\\Root (user-level)");
                return Ok(());
            }
            Err(format!("Failed to install certificate: {}", stderr))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            Err(format!("Failed to install certificate: {}", stderr))
        }
        Err(e) => Err(format!("Failed to install certificate: {}", e)),
    }
}

/// Install certificate to OS trust store
/// Returns Ok(()) if already trusted or successfully installed
pub fn install_cert(password: Option<&str>) -> Result<(), String> {
    let cert_path = get_cert_path()
        .ok_or("Certificate file not found in assets/ (dev) or bundled alongside executable (prod)")?;

    #[cfg(target_os = "macos")]
    {
        install_cert_to_keychain(&cert_path, password)
    }

    #[cfg(target_os = "linux")]
    {
        install_cert_to_ca(&cert_path, password)
    }

    #[cfg(target_os = "windows")]
    {
        install_cert_to_store(&cert_path, password)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err("Certificate installation not supported on this platform".to_string())
    }
}

/// Check if certificate is already installed (basic check)
pub fn is_cert_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        // Check if the cert's common name is in the System.keychain
        let output = Command::new("security")
            .args(&["find-certificate", "-a", "-c", "googleapis", "/Library/Keychains/System.keychain"])
            .output();

        match output {
            Ok(out) => out.status.success() && !String::from_utf8_lossy(&out.stdout).contains("does not exist"),
            Err(_) => false,
        }
    }

    #[cfg(target_os = "linux")]
    {
        Path::new("/usr/local/share/ca-certificates/googleapis.crt").exists()
            || Path::new("/etc/ssl/certs/googleapis.crt").exists()
    }

    #[cfg(target_os = "windows")]
    {
        // Check if cert exists in either LocalMachine\Root or CurrentUser\Root
        let output = Command::new("powershell")
            .args(&["-NoProfile", "-Command", "(Get-ChildItem -Path Cert:\\LocalMachine\\Root | Where-Object { $_.Subject -like '*googleapis*' } | Measure-Object).Count -gt 0 -or (Get-ChildItem -Path Cert:\\CurrentUser\\Root | Where-Object { $_.Subject -like '*googleapis*' } | Measure-Object).Count -gt 0"])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                stdout.trim().eq_ignore_ascii_case("True")
            }
            _ => false,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_cert_path() {
        let path = get_cert_path();
        if let Some(p) = path {
            // In dev, should point to assets/
            assert!(p.to_string_lossy().contains("googleapis.crt"));
        }
    }
}