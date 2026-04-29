use std::path::PathBuf;

pub const VNPAY_BASE_URL: &str = "https://genai.vnpay.vn/aicoding";

pub fn get_settings_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    Ok(home.join(".claude").join("settings.json"))
}

pub fn has_vnpay_config() -> Result<bool, String> {
    let path = get_settings_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let json: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
    Ok(json
        .get("env")
        .and_then(|v| v.as_object())
        .map(|e| e.contains_key("ANTHROPIC_AUTH_TOKEN") && e.contains_key("ANTHROPIC_BASE_URL"))
        .unwrap_or(false))
}

pub fn apply_vnpay_jwt(jwt: &str) -> Result<(), String> {
    let path = get_settings_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create ~/.claude directory: {}", e))?;
    }

    let mut json: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read settings: {}", e))?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !json.is_object() {
        json = serde_json::json!({});
    }

    {
        let obj = json.as_object_mut().unwrap();
        let env_entry = obj
            .entry("env".to_string())
            .or_insert(serde_json::json!({}));
        if !env_entry.is_object() {
            *env_entry = serde_json::json!({});
        }
        let env_obj = env_entry.as_object_mut().unwrap();
        env_obj.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            serde_json::Value::String(jwt.to_string()),
        );
        env_obj.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            serde_json::Value::String(VNPAY_BASE_URL.to_string()),
        );
    }

    let json_str = serde_json::to_string_pretty(&json)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&path, json_str)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;

    crate::modules::logger::log_info(&format!("Login CLI settings applied: {}", path.display()));
    Ok(())
}

pub fn remove_vnpay_config() -> Result<(), String> {
    let path = get_settings_path()?;
    if !path.exists() {
        return Ok(());
    }

    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read settings: {}", e))?;
    let mut json: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(obj) = json.as_object_mut() {
        if let Some(env) = obj.get_mut("env") {
            if let Some(env_obj) = env.as_object_mut() {
                env_obj.remove("ANTHROPIC_AUTH_TOKEN");
                env_obj.remove("ANTHROPIC_BASE_URL");
            }
        }
    }

    let json_str = serde_json::to_string_pretty(&json)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&path, json_str)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;

    crate::modules::logger::log_info(&format!(
        "Claude CLI VNPAY settings removed: {}",
        path.display()
    ));
    Ok(())
}

const OTEL_EXPORTS: &[(&str, &str)] = &[
    ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
    ("OTEL_SERVICE_NAME", "claude-code"),
    ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
    (
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "https://claude.io.vnoffice.vn/",
    ),
    ("OTEL_METRICS_EXPORTER", "otlp"),
    ("OTEL_LOGS_EXPORTER", "otlp"),
    ("OTEL_TRACES_EXPORTER", "otlp"),
];

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn ensure_otel_telemetry() -> Result<bool, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let profile = if shell.ends_with("zsh") {
        home.join(".zshrc")
    } else if shell.ends_with("bash") {
        home.join(".bashrc")
    } else {
        home.join(".profile")
    };

    let existing = std::fs::read_to_string(&profile).unwrap_or_default();
    let mut to_append = String::new();
    for (k, v) in OTEL_EXPORTS {
        let needle = format!("export {}=", k);
        if !existing.contains(&needle) {
            to_append.push_str(&format!("export {}={}\n", k, v));
        }
    }

    if to_append.is_empty() {
        return Ok(false);
    }

    let mut block = String::from("\n# Claude Code OpenTelemetry (added by anti-sw)\n");
    block.push_str(&to_append);

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&profile)
        .map_err(|e| format!("Failed to open {}: {}", profile.display(), e))?;
    file.write_all(block.as_bytes())
        .map_err(|e| format!("Failed to write profile: {}", e))?;

    crate::modules::logger::log_info(&format!(
        "OTel telemetry env appended to {}",
        profile.display()
    ));
    Ok(true)
}

#[cfg(target_os = "windows")]
pub fn ensure_otel_telemetry() -> Result<bool, String> {
    let mut added = false;
    for (k, v) in OTEL_EXPORTS {
        let current = std::env::var(k).unwrap_or_default();
        if current == *v {
            continue;
        }
        let output = std::process::Command::new("setx")
            .arg(k)
            .arg(v)
            .output()
            .map_err(|e| format!("setx {}: {}", k, e))?;
        if output.status.success() {
            added = true;
            crate::modules::logger::log_info(&format!("setx {}={} ok", k, v));
        } else {
            crate::modules::logger::log_warn(&format!(
                "setx {} failed: {}",
                k,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    Ok(added)
}
