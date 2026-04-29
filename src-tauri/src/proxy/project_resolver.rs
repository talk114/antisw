use serde_json::Value;

// Google Cloud Code endpoints - DNS redirect will route to VNPAY when MITM is enabled
const GOOGLE_SANDBOX_URL: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:loadCodeAssist";
const GOOGLE_DAILY_URL: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const GOOGLE_PROD_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";

const GOOGLE_URLS: [&str; 3] = [
    GOOGLE_SANDBOX_URL,
    GOOGLE_DAILY_URL,
    GOOGLE_PROD_URL,
];

/// Sử dụng Antigravity's loadCodeAssist API để lấy project_id
/// DNS redirect sẽ tự động chuyển sang VNPAY khi MITM được bật
pub async fn fetch_project_id(access_token: &str) -> Result<String, String> {
    for url in GOOGLE_URLS {
        tracing::debug!("[Project-Resolver] Trying: {}", url);
        match try_fetch_project_id_from_url(url, access_token).await {
            Ok(project_id) => {
                tracing::info!("[Project-Resolver] Success from: {}", url);
                return Ok(project_id);
            }
            Err(err) => {
                tracing::warn!("[Project-Resolver] {} failed: {}", url, err);
                continue;
            }
        }
    }
    Err("[Project-Resolver] All endpoints failed for loadCodeAssist".to_string())
}

/// Thử fetch project_id từ một URL cụ thể
async fn try_fetch_project_id_from_url(
    url: &str,
    access_token: &str,
) -> Result<String, String> {
    let request_body = serde_json::json!({
        "metadata": {
            "ideType": "ANTIGRAVITY"
        }
    });

    let client = crate::utils::http::get_client();
    let response = client
        .post(url)
        .bearer_auth(access_token)
        .header("User-Agent", crate::constants::USER_AGENT.as_str())
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("loadCodeAssist request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("loadCodeAssist returned error {}: {}", status, body));
    }

    let data: Value = response.json()
        .await
        .map_err(|e| format!("parse response failed: {}", e))?;

    // Extract cloudaicompanionProject
    if let Some(project_id) = data.get("cloudaicompanionProject")
        .and_then(|v| v.as_str()) {
        return Ok(project_id.to_string());
    }

    Err("Account not eligible for cloudaicompanionProject".to_string())
}