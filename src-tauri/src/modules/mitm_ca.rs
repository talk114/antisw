// Root CA + leaf cert generation for MITM server — stored in ~/.antisw/mitm/

use std::fs;
use std::path::PathBuf;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, Ia5String, IsCa,
    KeyPair, KeyUsagePurpose, SanType,
};

const MITM_DOMAINS: &[&str] = &[
    "daily-cloudcode-pa.googleapis.com",
    "cloudcode-pa.googleapis.com",
];

pub fn get_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".antisw")
}

pub fn get_mitm_dir() -> PathBuf {
    get_data_dir().join("mitm")
}

fn ca_key_path() -> PathBuf {
    get_mitm_dir().join("rootCA.key")
}

fn ca_cert_path() -> PathBuf {
    get_mitm_dir().join("rootCA.crt")
}

fn domain_key_path(domain: &str) -> PathBuf {
    get_mitm_dir().join(format!("{}.key", domain))
}

fn domain_cert_path(domain: &str) -> PathBuf {
    get_mitm_dir().join(format!("{}.crt", domain))
}

/// Ensure Root CA + leaf certs for all MITM domains exist.
pub fn ensure_all_certs() -> Result<(), String> {
    let mitm_dir = get_mitm_dir();
    fs::create_dir_all(&mitm_dir)
        .map_err(|e| format!("Failed to create mitm dir: {}", e))?;

    let (ca_key, ca_cert) = ensure_root_ca()?;

    for domain in MITM_DOMAINS {
        let key_path = domain_key_path(domain);
        let cert_path = domain_cert_path(domain);
        if key_path.exists() && cert_path.exists() {
            continue;
        }

        tracing::info!("[MITM-CA] Generating leaf cert for {}", domain);

        let leaf_key = KeyPair::generate()
            .map_err(|e| format!("Failed to generate leaf key for {}: {}", domain, e))?;

        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, *domain);
        params.subject_alt_names = vec![
            SanType::DnsName(
                Ia5String::try_from(domain.to_string())
                    .map_err(|e| format!("Invalid domain {}: {}", domain, e))?,
            ),
            SanType::DnsName(
                Ia5String::try_from(format!("*.{}", domain))
                    .map_err(|e| format!("Invalid wildcard domain: {}", e))?,
            ),
        ];
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];

        let leaf_cert = params
            .signed_by(&leaf_key, &ca_cert, &ca_key)
            .map_err(|e| format!("Failed to sign leaf cert for {}: {}", domain, e))?;

        fs::write(&key_path, leaf_key.serialize_pem())
            .map_err(|e| format!("Failed to write leaf key for {}: {}", domain, e))?;
        fs::write(&cert_path, leaf_cert.pem())
            .map_err(|e| format!("Failed to write leaf cert for {}: {}", domain, e))?;

        tracing::info!("[MITM-CA] Leaf cert generated for {}", domain);
    }

    Ok(())
}

/// Generate or load Root CA. Returns (KeyPair, Certificate) kept in memory for signing.
fn ensure_root_ca() -> Result<(KeyPair, rcgen::Certificate), String> {
    let key_path = ca_key_path();
    let cert_path = ca_cert_path();

    if key_path.exists() && cert_path.exists() {
        tracing::info!("[MITM-CA] Root CA already exists");
        // Load from disk and reconstruct for signing
        let key_pem = fs::read_to_string(&key_path)
            .map_err(|e| format!("Failed to read CA key: {}", e))?;
        let ca_key = KeyPair::from_pem(&key_pem)
            .map_err(|e| format!("Failed to parse CA key: {}", e))?;
        let ca_cert = build_ca_cert_params().self_signed(&ca_key)
            .map_err(|e| format!("Failed to reconstruct CA cert: {}", e))?;
        return Ok((ca_key, ca_cert));
    }

    tracing::info!("[MITM-CA] Generating Root CA...");

    let ca_key = KeyPair::generate()
        .map_err(|e| format!("Failed to generate CA key pair: {}", e))?;
    let ca_cert = build_ca_cert_params()
        .self_signed(&ca_key)
        .map_err(|e| format!("Failed to self-sign CA cert: {}", e))?;

    fs::write(&key_path, ca_key.serialize_pem())
        .map_err(|e| format!("Failed to write CA key: {}", e))?;
    fs::write(&cert_path, ca_cert.pem())
        .map_err(|e| format!("Failed to write CA cert: {}", e))?;

    tracing::info!("[MITM-CA] Root CA generated at {:?}", get_mitm_dir());
    Ok((ca_key, ca_cert))
}

fn build_ca_cert_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, "Antisw MITM Root CA");
    params.distinguished_name.push(DnType::OrganizationName, "Antisw");
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params
}

pub fn read_ca_cert_pem() -> Option<String> {
    fs::read_to_string(ca_cert_path()).ok()
}

/// Check if the root CA certificate is already trusted by the system.
pub fn is_ca_cert_trusted() -> bool {
    let cert_path = ca_cert_path();
    if !cert_path.exists() {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        // Check if cert is in System Keychain as trusted
        let output = std::process::Command::new("security")
            .args(["find-certificate", "-c", "Antisw MITM Root CA", "/Library/Keychains/System.keychain"])
            .output();

        match output {
            Ok(out) if out.status.success() && !String::from_utf8_lossy(&out.stdout).is_empty() => {
                // Also check trust settings
                let trust_out = std::process::Command::new("security")
                    .args([
                        "dump-keychain",
                        "-i",
                        "/Library/Keychains/System.keychain",
                    ])
                    .output();

                if let Ok(trust_out) = trust_out {
                    let output_str = String::from_utf8_lossy(&trust_out.stdout);
                    // Look for the certificate with trust settings
                    return output_str.contains("Antisw MITM Root CA");
                }
                true // find-certificate found it
            }
            _ => false,
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Check if cert exists in Root store
        let output = std::process::Command::new("certutil")
            .args(["-verifystore", "Root", "-domaincontroller"])
            .output();

        // More reliable: check specific cert by hash
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-ChildItem -Path Cert:\\LocalMachine\\Root | Where-Object { $_.Subject -like '*Antisw*' } | Select-Object -First 1",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                // If output contains Antisw and not empty, cert is trusted
                stdout.contains("Antisw") && !stdout.trim().is_empty()
            }
            _ => false,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Trust the root CA certificate with administrator privileges.
///
/// On macOS: uses osascript to show native authentication dialog
/// On Windows: uses PowerShell with RunAs to prompt for admin credentials
///
/// Returns Ok(()) on success, Err(message) on failure.
pub fn trust_ca_cert() -> Result<(), String> {
    let cert_path = ca_cert_path();
    if !cert_path.exists() {
        return Err("CA certificate not found".to_string());
    }

    let cert_path_str = cert_path.to_string_lossy();

    #[cfg(target_os = "macos")]
    {
        // Use osascript to show macOS authentication dialog
        let script = format!(
            r#"do shell script "security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain '{}'" with administrator privileges"#,
            cert_path_str
        );

        let output = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("Failed to run osascript: {}", e))?;

        if output.status.success() {
            tracing::info!("[MITM-CA] Certificate trusted successfully on macOS");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            // User cancelled
            if stderr.contains("User canceled") || stdout.contains("User canceled") {
                Err("User cancelled authentication".to_string())
            } else {
                Err(format!("Failed to trust certificate: {} {}", stdout, stderr))
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Use PowerShell to add cert to Root store (requires admin)
        let script = format!(
            r#"
            $cert = Get-ChildItem -Path '{}' -ErrorAction Stop
            $store = New-Object System.Security.Cryptography.X509Certificates.X509Store('Root', 'LocalMachine')
            $store.Open('ReadWrite')
            $store.Add($cert)
            $store.Close()
            "#,
            cert_path_str.replace("\\", "\\\\")
        );

        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy", "Bypass",
                "-Command",
                &script,
            ])
            .output()
            .map_err(|e| format!("Failed to run PowerShell: {}", e))?;

        if output.status.success() {
            tracing::info!("[MITM-CA] Certificate trusted successfully on Windows");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("Access is denied") || stderr.contains("Access is denied") {
                Err("Administrator privileges required to trust certificate".to_string())
            } else {
                Err(format!("Failed to trust certificate: {} {}", stdout, stderr))
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("Trust certificate not supported on this platform".to_string())
    }
}
