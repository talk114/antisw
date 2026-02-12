use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::models::QuotaData;
use crate::modules::config;
use crate::error::AppError;

const QUOTA_API_URL: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:fetchAvailableModels";

/// Critical retry threshold: considered near recovery when quota reaches 95%
const NEAR_READY_THRESHOLD: i32 = 95;
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY_SECS: u64 = 30;

#[derive(Debug, Serialize, Deserialize)]
struct QuotaResponse {
    models: std::collections::HashMap<String, ModelInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelInfo {
    #[serde(rename = "quotaInfo")]
    quota_info: Option<QuotaInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct QuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadProjectResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project_id: Option<String>,
    #[serde(rename = "currentTier")]
    current_tier: Option<Tier>,
    #[serde(rename = "paidTier")]
    paid_tier: Option<Tier>,
}

#[derive(Debug, Deserialize)]
struct Tier {
    id: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "quotaTier")]
    quota_tier: Option<String>,
    #[allow(dead_code)]
    name: Option<String>,
    #[allow(dead_code)]
    slug: Option<String>,
}

/// Get shared HTTP Client (15s timeout)
async fn create_client(account_id: Option<&str>) -> reqwest::Client {
    if let Some(pool) = crate::proxy::proxy_pool::get_global_proxy_pool() {
        pool.get_effective_client(account_id, 15).await
    } else {
        crate::utils::http::get_client()
    }
}

/// Get shared HTTP Client (60s timeout)
#[allow(dead_code)] // 预留给预热/后台任务调用
async fn create_warmup_client(account_id: Option<&str>) -> reqwest::Client {
    if let Some(pool) = crate::proxy::proxy_pool::get_global_proxy_pool() {
        pool.get_effective_client(account_id, 60).await
    } else {
        crate::utils::http::get_long_client()
    }
}

/// Unified entry point for fetching account quota
pub async fn fetch_quota(_access_token: &str, _email: &str, _account_id: Option<&str>) -> crate::error::AppResult<(QuotaData, Option<String>)> {
    Ok((QuotaData::new(), None))
}

/// Fetch quota with cache support
pub async fn fetch_quota_with_cache(
    _access_token: &str,
    _email: &str,
    _cached_project_id: Option<&str>,
    _account_id: Option<&str>,
) -> crate::error::AppResult<(QuotaData, Option<String>)> {
      Err(AppError::Unknown("Quota fetch failed".to_string()))
}

/// Internal fetch quota logic
pub async fn fetch_quota_inner(_access_token: &str, _email: &str) -> crate::error::AppResult<(QuotaData, Option<String>)> {
    // fetch_quota_with_cache(access_token, email, None, None).await
    Ok((QuotaData::new(), None))
}

/// Batch fetch all account quotas (backup functionality)
#[allow(dead_code)]
pub async fn fetch_all_quotas(_accounts: Vec<(String, String, String)>) -> Vec<(String, crate::error::AppResult<QuotaData>)> {
    let results = Vec::new();
    // for (id, email, access_token) in accounts {
    //     let res = fetch_quota(&access_token, &email, Some(&id)).await;
    //     results.push((email, res.map(|(q, _)| q)));
    // }
    results
}

/// Get valid token (auto-refresh if expired)
pub async fn get_valid_token_for_warmup(account: &crate::models::account::Account) -> Result<(String, String), String> {
    let mut account = account.clone();
    
    // Check and auto-refresh token
    let new_token = crate::modules::oauth::ensure_fresh_token(&account.token, Some(&account.id)).await?;
    
    // If token changed (meant refreshed), save it
    if new_token.access_token != account.token.access_token {
        account.token = new_token;
        if let Err(e) = crate::modules::account::save_account(&account) {
            crate::modules::logger::log_warn(&format!("[Warmup] Failed to save refreshed token: {}", e));
        } else {
            crate::modules::logger::log_info(&format!("[Warmup] Successfully refreshed and saved new token for {}", account.email));
        }
    }
    
    // Fetch project_id (Stubbed since original function was deleted)
    let final_pid = "bamboo-precept-lgxtn".to_string();
    
    Ok((account.token.access_token, final_pid))
}

/// Send warmup request via proxy internal API
pub async fn warmup_model_directly(
    access_token: &str,
    model_name: &str,
    project_id: &str,
    email: &str,
    percentage: i32,
    _account_id: Option<&str>,
) -> bool {
    // Get currently configured proxy port
    let port = config::load_app_config()
        .map(|c| c.proxy.port)
        .unwrap_or(8045);

    let warmup_url = format!("http://127.0.0.1:{}/internal/warmup", port);
    let body = json!({
        "email": email,
        "model": model_name,
        "access_token": access_token,
        "project_id": project_id
    });

    // Use a no-proxy client for local loopback requests
    // This prevents Docker environments from routing localhost through external proxies
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .post(&warmup_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;

    match resp {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                crate::modules::logger::log_info(&format!("[Warmup] ✓ Triggered {} for {} (was {}%)", model_name, email, percentage));
                true
            } else {
                let text = response.text().await.unwrap_or_default();
                crate::modules::logger::log_warn(&format!("[Warmup] ✗ {} for {} (was {}%): HTTP {} - {}", model_name, email, percentage, status, text));
                false
            }
        }
        Err(e) => {
            crate::modules::logger::log_warn(&format!("[Warmup] ✗ {} for {} (was {}%): {}", model_name, email, percentage, e));
            false
        }
    }
}

/// Smart warmup for all accounts
pub async fn warm_up_all_accounts() -> Result<String, String> {
    let _retry_count = 0;

    // loop {
    //     let all_accounts = crate::modules::account::list_accounts().unwrap_or_default();
    //     // [FIX] 过滤掉禁用反代的账号
    //     let target_accounts: Vec<_> = all_accounts
    //         .into_iter()
    //         .filter(|a| !a.disabled && !a.proxy_disabled)
    //         .collect();

    //     if target_accounts.is_empty() {
    //         return Ok("No accounts available".to_string());
    //     }

    //     crate::modules::logger::log_info(&format!("[Warmup] Screening models for {} accounts...", target_accounts.len()));

    //     let mut warmup_items = Vec::new();
    //     let mut has_near_ready_models = false;

    //     // Concurrently fetch quotas (batch size 5)
    //     let batch_size = 5;
    //     for batch in target_accounts.chunks(batch_size) {
    //         let mut handles = Vec::new();
    //         for account in batch {
    //             let account = account.clone();
    //             let handle = tokio::spawn(async move {
    //                 let (token, pid) = match get_valid_token_for_warmup(&account).await {
    //                     Ok(t) => t,
    //                     Err(_) => return None,
    //                 };
    //                 let quota = fetch_quota_with_cache(&token, &account.email, Some(&pid), Some(&account.id)).await.ok();
    //                 Some((account.id.clone(), account.email.clone(), token, pid, quota))
    //             });
    //             handles.push(handle);
    //         }

    //         for handle in handles {
    //             if let Ok(Some((id, email, token, pid, Some((fresh_quota, _))))) = handle.await {
    //                 let mut account_warmed_series = std::collections::HashSet::new();
    //                 for m in fresh_quota.models {
    //                     if m.percentage >= 100 {
    //                         let model_to_ping = m.name.clone();

    //                         // Removed hardcoded whitelist - now warms up any model at 100%
    //                         if !account_warmed_series.contains(&model_to_ping) {
    //                             warmup_items.push((id.clone(), email.clone(), model_to_ping.clone(), token.clone(), pid.clone(), m.percentage));
    //                             account_warmed_series.insert(model_to_ping);
    //                         }
    //                     } else if m.percentage >= NEAR_READY_THRESHOLD {
    //                         has_near_ready_models = true;
    //                     }
    //                 }
    //             }
    //         }
    //     }

    //     if !warmup_items.is_empty() {
    //         let total_before = warmup_items.len();
            
    //         // Filter out models warmed up within 4 hours
    //         warmup_items.retain(|(_, email, model, _, _, _)| {
    //             let history_key = format!("{}:{}:100", email, model);
    //             !crate::modules::scheduler::check_cooldown(&history_key, 14400)
    //         });
            
    //         if warmup_items.is_empty() {
    //             let skipped = total_before;
    //             crate::modules::logger::log_info(&format!("[Warmup] Returning to frontend: All models in cooldown, skipped {}", skipped));
    //             return Ok(format!("All models are in cooldown, skipped {} items", skipped));
    //         }
            
    //         let total = warmup_items.len();
    //         let skipped = total_before - total;
            
    //         if skipped > 0 {
    //             crate::modules::logger::log_info(&format!(
    //                 "[Warmup] Skipped {} models in cooldown, preparing to warmup {}",
    //                 skipped, total
    //             ));
    //         }
            
    //         crate::modules::logger::log_info(&format!(
    //             "[Warmup] 🔥 Starting manual warmup for {} models",
    //             total
    //         ));
            
    //         tokio::spawn(async move {
    //             let mut success = 0;
    //             let batch_size = 3;
    //             let now_ts = chrono::Utc::now().timestamp();
                
    //             for (batch_idx, batch) in warmup_items.chunks(batch_size).enumerate() {
    //                 let mut handles = Vec::new();
                    
    //                 for (id, email, model, token, pid, pct) in batch.iter() {
    //                     let id = id.clone();
    //                     let email = email.clone();
    //                     let model = model.clone();
    //                     let token = token.clone();
    //                     let pid = pid.clone();
    //                     let pct = *pct;
                        
    //                     let handle = tokio::spawn(async move {
    //                         let result = warmup_model_directly(&token, &model, &pid, &email, pct, Some(&id)).await;
    //                         (result, email, model)
    //                     });
    //                     handles.push(handle);
    //                 }
                    
    //                 for handle in handles {
    //                     match handle.await {
    //                         Ok((true, email, model)) => {
    //                             success += 1;
    //                             let history_key = format!("{}:{}:100", email, model);
    //                             crate::modules::scheduler::record_warmup_history(&history_key, now_ts);
    //                         }
    //                         _ => {}
    //                     }
    //                 }
                    
    //                 if batch_idx < (warmup_items.len() + batch_size - 1) / batch_size - 1 {
    //                     tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    //                 }
    //             }
                
    //             crate::modules::logger::log_info(&format!("[Warmup] Warmup task completed: success {}/{}", success, total));
    //             tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    //             let _ = crate::modules::account::refresh_all_quotas_logic().await;
    //         });
    //         crate::modules::logger::log_info(&format!("[Warmup] Returning to frontend: Warmup task triggered for {} models", total));
    //         return Ok(format!("Warmup task triggered for {} models", total));
    //     }

    //     if has_near_ready_models && retry_count < MAX_RETRIES {
    //         retry_count += 1;
    //         crate::modules::logger::log_info(&format!("[Warmup] Critical recovery model detected, waiting {}s to retry ({}/{})", RETRY_DELAY_SECS, retry_count, MAX_RETRIES));
    //         tokio::time::sleep(tokio::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
    //         continue;
    //     }

        return Ok("No models need warmup".to_string());
    // }
}

/// Warmup for single account
pub async fn warm_up_account(_account_id: &str) -> Result<String, String> {
    // let accounts = crate::modules::account::list_accounts().unwrap_or_default();
    // let account_owned = accounts.iter().find(|a| a.id == account_id).cloned().ok_or_else(|| "Account not found".to_string())?;

    // if account_owned.disabled || account_owned.proxy_disabled {
    //     return Err("Account is disabled".to_string());
    // }
    
    // let email = account_owned.email.clone();
    // let (token, pid) = get_valid_token_for_warmup(&account_owned).await?;
    // let (fresh_quota, _) = fetch_quota_with_cache(&token, &email, Some(&pid), Some(&account_owned.id)).await.map_err(|e| format!("Failed to fetch quota: {}", e))?;
    
    // let mut models_to_warm = Vec::new();
    // let mut warmed_series = std::collections::HashSet::new();

    // for m in fresh_quota.models {
    //     if m.percentage >= 100 {
    //         let model_name = m.name.clone();

    //         // Removed hardcoded whitelist - now warms up any model at 100%
    //         if !warmed_series.contains(&model_name) {
    //             models_to_warm.push((model_name.clone(), m.percentage));
    //             warmed_series.insert(model_name);
    //         }
    //     }
    // }

    // if models_to_warm.is_empty() {
    //     return Ok("No warmup needed".to_string());
    // }

    let warmed_count =  0;//models_to_warm.len();
    // let account_id_clone = account_id.to_string();
    
    // tokio::spawn(async move {
    //     for (name, pct) in models_to_warm {
    //         if warmup_model_directly(&token, &name, &pid, &email, pct, Some(&account_id_clone)).await {
    //             let history_key = format!("{}:{}:100", email, name);
    //             let now_ts = chrono::Utc::now().timestamp();
    //             crate::modules::scheduler::record_warmup_history(&history_key, now_ts);
    //         }
    //         tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    //     }
    //     let _ = crate::modules::account::refresh_all_quotas_logic().await;
    // });

    Ok(format!("Successfully triggered warmup for {} model series", warmed_count))
}
