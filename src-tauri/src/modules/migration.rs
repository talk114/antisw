use std::fs;
use std::path::PathBuf;
use serde_json::Value;
use base64::{Engine as _, engine::general_purpose};
use crate::models::{TokenData, Account, AccountIndex, AccountSummary};
use crate::modules::{account, db};
use crate::utils::protobuf;

/// Sync legacy data from `~/.antigravity_sw/accounts/*.json` (plaintext, used by
/// older builds) into the new `~/.antisw/accounts/` layout that stores each
/// account file encrypted. Idempotent: only copies accounts whose target file
/// does not already exist in the new directory, so it is safe to call on every
/// startup.
pub fn sync_legacy_antigravity_sw_accounts() -> Result<usize, String> {
    let home = dirs::home_dir().ok_or("Failed to get home directory")?;
    let legacy_dir = home.join(".antigravity_sw");
    let legacy_accounts_dir = legacy_dir.join("accounts");

    if !legacy_accounts_dir.exists() {
        return Ok(0);
    }

    let new_accounts_dir = account::get_accounts_dir()?;

    crate::modules::logger::log_info(&format!(
        "[Migration] Detected legacy data directory {:?}, syncing accounts to new encrypted layout",
        legacy_accounts_dir
    ));

    let entries = match fs::read_dir(&legacy_accounts_dir) {
        Ok(e) => e,
        Err(e) => {
            return Err(format!("Failed to read legacy accounts dir: {}", e));
        }
    };

    // Build a set of existing emails (case-insensitive) so we can skip legacy
    // accounts whose email already exists under a different ID — otherwise the
    // same user would be migrated again and appear twice in the new layout.
    let existing_index = account::load_account_index().unwrap_or_else(|_| AccountIndex::new());
    let mut existing_emails: std::collections::HashSet<String> = existing_index
        .accounts
        .iter()
        .map(|s| s.email.to_lowercase())
        .collect();

    let mut migrated: Vec<Account> = Vec::new();
    let mut skipped = 0usize;

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !path.extension().map_or(false, |ext| ext == "json") {
            continue;
        }

        let account_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        let new_account_path = new_accounts_dir.join(format!("{}.json", account_id));
        if new_account_path.exists() {
            skipped += 1;
            continue;
        }

        let raw = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                crate::modules::logger::log_warn(&format!(
                    "[Migration] Failed to read legacy account file {:?}: {}",
                    path, e
                ));
                continue;
            }
        };

        // Older builds may have written plaintext JSON, but be defensive and
        // also handle the case where the file is already in the new encrypted
        // envelope (decrypt_file_content returns the raw input when no envelope
        // is detected).
        let content = crate::utils::crypto::decrypt_file_content(&raw);

        let account: Account = match serde_json::from_str(&content) {
            Ok(acc) => acc,
            Err(e) => {
                crate::modules::logger::log_warn(&format!(
                    "[Migration] Failed to parse legacy account {:?}: {}",
                    path, e
                ));
                continue;
            }
        };

        let email_key = account.email.to_lowercase();
        if existing_emails.contains(&email_key) {
            crate::modules::logger::log_info(&format!(
                "[Migration] Skipping legacy account {} ({}): email already present under a different ID",
                account.email, account.id
            ));
            skipped += 1;
            continue;
        }

        if let Err(e) = account::save_account(&account) {
            crate::modules::logger::log_warn(&format!(
                "[Migration] Failed to save migrated account {} ({}): {}",
                account.email, account.id, e
            ));
            continue;
        }

        crate::modules::logger::log_info(&format!(
            "[Migration] Migrated account: {} ({})",
            account.email, account.id
        ));
        existing_emails.insert(email_key);
        migrated.push(account);
    }

    if migrated.is_empty() {
        crate::modules::logger::log_info(&format!(
            "[Migration] No new legacy accounts to sync ({} already present)",
            skipped
        ));
        return Ok(0);
    }

    // Merge migrated summaries into the new index (build a fresh index if the
    // new directory had none previously).
    let mut new_index = account::load_account_index().unwrap_or_else(|_| AccountIndex::new());

    for acc in &migrated {
        if new_index.accounts.iter().any(|s| s.id == acc.id) {
            continue;
        }
        new_index.accounts.push(AccountSummary {
            id: acc.id.clone(),
            email: acc.email.clone(),
            name: acc.name.clone(),
            disabled: acc.disabled,
            proxy_disabled: acc.proxy_disabled,
            protected_models: acc.protected_models.clone(),
            created_at: acc.created_at,
            last_used: acc.last_used,
        });
    }

    // Preserve current_account_id from the legacy index if the new one has none.
    if new_index.current_account_id.is_none() {
        let legacy_index_path = legacy_dir.join("accounts.json");
        if legacy_index_path.exists() {
            if let Ok(raw) = fs::read_to_string(&legacy_index_path) {
                let content = crate::utils::crypto::decrypt_file_content(&raw);
                if let Ok(legacy_index) = serde_json::from_str::<AccountIndex>(&content) {
                    if let Some(id) = legacy_index.current_account_id {
                        if new_index.accounts.iter().any(|a| a.id == id) {
                            new_index.current_account_id = Some(id);
                        }
                    }
                }
            }
        }
    }

    if new_index.current_account_id.is_none() {
        new_index.current_account_id = new_index.accounts.first().map(|s| s.id.clone());
    }

    if let Err(e) = account::save_account_index(&new_index) {
        crate::modules::logger::log_warn(&format!(
            "[Migration] Failed to save new account index after sync: {}",
            e
        ));
    }

    crate::modules::logger::log_info(&format!(
        "[Migration] Legacy sync complete: {} migrated, {} already present",
        migrated.len(),
        skipped
    ));

    Ok(migrated.len())
}

/// Scan and import V1 data
pub async fn import_from_v1() -> Result<Vec<Account>, String> {
    use crate::modules::oauth;

    let home = dirs::home_dir().ok_or("Failed to get home directory")?;
    
    // V1 data directory (confirmed cross-platform consistency from utils.py)
    let v1_dir = home.join(".antigravity-agent");
    
    let mut imported_accounts = Vec::new();
    
    // Try multiple possible filenames
    let index_files = vec![
        "antigravity_accounts.json", // Directly use string literal
        "accounts.json"
    ];
    
    let mut found_index = false;

    for index_filename in index_files {
        let v1_accounts_path = v1_dir.join(index_filename);
        
        if !v1_accounts_path.exists() {
            continue;
        }
        
        found_index = true;
        crate::modules::logger::log_info(&format!("V1 data discovered: {:?}", v1_accounts_path));
        
        let content = match fs::read_to_string(&v1_accounts_path) {
            Ok(c) => c,
            Err(e) => {
                crate::modules::logger::log_warn(&format!("Failed to read index: {}", e));
                continue;
            }
        };
        
        let v1_index: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                crate::modules::logger::log_warn(&format!("Failed to parse index JSON: {}", e));
                continue;
            }
        };
        
        // Compatible with two formats: direct map, or contains "accounts" field
        let accounts_map = if let Some(map) = v1_index.as_object() {
            if let Some(accounts) = map.get("accounts").and_then(|v| v.as_object()) {
                accounts 
            } else {
                map
            }
        } else {
            continue;
        };
        
        for (id, acc_info) in accounts_map {
            let email_placeholder = acc_info.get("email").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
            
            // Skip non-account keys (e.g. "current_account_id")
            if !acc_info.is_object() {
                continue;
            }
            
            let backup_file_str = acc_info.get("backup_file").and_then(|v| v.as_str());
            let data_file_str = acc_info.get("data_file").and_then(|v| v.as_str());
            
            // Prefer backup_file, then data_file
            let target_file = backup_file_str.or(data_file_str);
            
            if target_file.is_none() {
                crate::modules::logger::log_warn(&format!("Account {} ({}) missing data file path", id, email_placeholder));
                continue;
            }
            
            let mut backup_path = PathBuf::from(target_file.unwrap());
            
            // If relative path, try joining with v1_dir
            if !backup_path.exists() {
                 backup_path = v1_dir.join(backup_path.file_name().unwrap_or_default());
            }
            
            // Try joining data/ or backups/ subdirectories again
            if !backup_path.exists() {
                 let file_name = backup_path.file_name().unwrap_or_default();
                 let try_backups = v1_dir.join("backups").join(file_name);
                 if try_backups.exists() {
                     backup_path = try_backups;
                 } else {
                     let try_accounts = v1_dir.join("accounts").join(file_name);
                     if try_accounts.exists() {
                         backup_path = try_accounts;
                     }
                 }
            }
            
            if !backup_path.exists() {
                crate::modules::logger::log_warn(&format!("Account {} ({}) backup file not found: {:?}", id, email_placeholder, backup_path));
                continue;
            }
            
            // Read backup file
            if let Ok(backup_content) = fs::read_to_string(&backup_path) {
                if let Ok(backup_json) = serde_json::from_str::<Value>(&backup_content) {
                    
                    // Compatible with two formats:
                    // 1. V1 backup: jetskiStateSync.agentManagerInitState -> Protobuf
                    // 2. V2/Script data: JSON containing "token" field
                    
                    let mut refresh_token_opt = None;
                    
                    // Try format 2
                    if let Some(token_data) = backup_json.get("token") {
                        if let Some(rt) = token_data.get("refresh_token").and_then(|v| v.as_str()) {
                            refresh_token_opt = Some(rt.to_string());
                        }
                    }
                    
                    // Try format 1
                    if refresh_token_opt.is_none() {
                         if let Some(state_b64) = backup_json.get("jetskiStateSync.agentManagerInitState").and_then(|v| v.as_str()) {
                            // Parse Protobuf
                            if let Ok(blob) = general_purpose::STANDARD.decode(state_b64) {
                                if let Ok(Some(oauth_data)) = protobuf::find_field(&blob, 6) {
                                    if let Ok(Some(refresh_bytes)) = protobuf::find_field(&oauth_data, 3) {
                                        if let Ok(rt) = String::from_utf8(refresh_bytes) {
                                            refresh_token_opt = Some(rt);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                    if let Some(refresh_token) = refresh_token_opt {
                         crate::modules::logger::log_info(&format!("Importing account: {}", email_placeholder));
                                                  let (email, access_token, expires_in) = match oauth::refresh_access_token(&refresh_token, None).await {
                             Ok(token_resp) => {
                                 match oauth::get_user_info(&token_resp.access_token, None).await {
                                     Ok(user_info) => (user_info.email, token_resp.access_token, token_resp.expires_in),
                                     Err(_) => (email_placeholder.clone(), token_resp.access_token, token_resp.expires_in), 
                                 }
                             },
                            Err(e) => {
                                crate::modules::logger::log_warn(&format!("Token refresh failed (likely expired): {}", e));
                                (email_placeholder.clone(), "imported_access_token".to_string(), 0)
                            }, 
                        };
                        
                        let token_data = TokenData::new(
                            access_token, 
                            refresh_token,
                            expires_in,
                            Some(email.clone()),
                            None, // project_id will be fetched on demand
                            None, // session_id
                    );
                        
                        // Name already fetched in get_user_info at line 153, but outside match scope, use None to be safe
                        match account::upsert_account(email.clone(), None, token_data) {
                            Ok(acc) => {
                                crate::modules::logger::log_info(&format!("Import successful: {}", email));
                                imported_accounts.push(acc);
                            },
                            Err(e) => crate::modules::logger::log_error(&format!("Import save failed {}: {}", email, e)),
                        }

                    } else {
                        crate::modules::logger::log_warn(&format!("Account {} data file missing Refresh Token", email_placeholder));
                    }
                }
            }
        }
    }
    
    if !found_index {
        return Err("V1 account data file not found".to_string());
    }
    
    Ok(imported_accounts)
}

/// Import account from custom database path
pub async fn import_from_custom_db_path(path_str: String) -> Result<Account, String> {
    use crate::modules::oauth;

    let path = PathBuf::from(path_str);
    if !path.exists() {
        return Err(format!("File does not exist: {:?}", path));
    }

    let refresh_token = extract_refresh_token_from_file(&path)?;
        
    // 3. Use Refresh Token to get latest Access Token and user info
    crate::modules::logger::log_info("Getting user info using Refresh Token...");
    let token_resp = oauth::refresh_access_token(&refresh_token, None).await?;
    let user_info = oauth::get_user_info(&token_resp.access_token, None).await?;
    
    let email = user_info.email;
    
    crate::modules::logger::log_info(&format!("Successfully retrieved account info: {}", email));
    
    let token_data = TokenData::new(
        token_resp.access_token,
        refresh_token,
        token_resp.expires_in,
        Some(email.clone()),
        None, // project_id will be fetched on demand
        None, // session_id will be generated in token_manager
    );
    
    // 4. Add or update account
    account::upsert_account(email.clone(), user_info.name, token_data)
}

/// Import current logged-in account from default IDE database
pub async fn import_from_db() -> Result<Account, String> {
    let db_path = db::get_db_path()?;
    import_from_custom_db_path(db_path.to_string_lossy().to_string()).await
}

/// Get current Refresh Token from database (common logic)
pub fn extract_refresh_token_from_file(db_path: &PathBuf) -> Result<String, String> {
    use base64::{engine::general_purpose, Engine as _};
    
    if !db_path.exists() {
        return Err(format!("Database file not found: {:?}", db_path));
    }
    
    // Connect to database
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("Failed to open database: {}", e))?;
        
    // 1. 尝试新版格式 (>= 1.16.5)
    // 键: antigravityUnifiedStateSync.oauthToken
    // 结构: Outer(F1) -> Inner(F2) -> Inner2(F1) -> Base64 -> OAuthInfo
    let new_format_data: Option<String> = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?",
            ["antigravityUnifiedStateSync.oauthToken"],
            |row| row.get(0),
        )
        .ok();

    if let Some(outer_b64) = new_format_data {
        crate::modules::logger::log_info("Detected new format database (antigravityUnifiedStateSync.oauthToken)");
        
        // Base64 解码外层数据
        let outer_blob = general_purpose::STANDARD
            .decode(&outer_b64)
            .map_err(|e| format!("Outer Base64 decoding failed: {}", e))?;
            
        // 解析 Outer (Field 1) -> Inner1
        let inner1_blob = protobuf::find_field(&outer_blob, 1)
            .map_err(|e| format!("Parsing Outer Field 1 failed: {}", e))?
            .ok_or("Outer Field 1 not found")?;
            
        // 解析 Inner1 (Field 2) -> Inner2
        let inner2_blob = protobuf::find_field(&inner1_blob, 2)
            .map_err(|e| format!("Parsing Inner1 Field 2 failed: {}", e))?
            .ok_or("Inner1 Field 2 not found")?;
            
        // 解析 Inner2 (Field 1) -> OAuthInfo B64 String
        let oauth_info_bytes = protobuf::find_field(&inner2_blob, 1)
            .map_err(|e| format!("Parsing Inner2 Field 1 failed: {}", e))?
            .ok_or("Inner2 Field 1 not found")?;
            
        let oauth_info_b64 = String::from_utf8(oauth_info_bytes)
            .map_err(|_| "OAuth Info B64 is not UTF-8")?;
            
        // 解码 OAuthInfo
        let oauth_info_blob = general_purpose::STANDARD
            .decode(&oauth_info_b64)
            .map_err(|e| format!("Inner Base64 decoding failed: {}", e))?;
            
        // 解析 OAuthInfo (Field 3) -> Refresh Token
        let refresh_bytes = protobuf::find_field(&oauth_info_blob, 3)
            .map_err(|e| format!("Parsing OAuthInfo Field 3 failed: {}", e))?
            .ok_or("Refresh Token not found in OAuthInfo (Field 3)")?;
            
        return String::from_utf8(refresh_bytes)
            .map_err(|_| "Refresh Token is not UTF-8 encoded".to_string());
    }

    // 2. 尝试旧版格式 (< 1.16.5)
    crate::modules::logger::log_info("Falling back to old format database (jetskiStateSync.agentManagerInitState)");
    let current_data: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?",
            ["jetskiStateSync.agentManagerInitState"],
            |row| row.get(0),
        )
        .map_err(|_| "Login state data not found in either format".to_string())?;
        
    // Base64 decode
    let blob = general_purpose::STANDARD
        .decode(&current_data)
        .map_err(|e| format!("Base64 decoding failed: {}", e))?;
        
    // 1. Find oauthTokenInfo (Field 6)
    let oauth_data = protobuf::find_field(&blob, 6)
        .map_err(|e| format!("Protobuf parsing failed: {}", e))?
        .ok_or("OAuth data not found (Field 6)")?;
        
    // 2. Extract refresh_token (Field 3)
    let refresh_bytes = protobuf::find_field(&oauth_data, 3)
        .map_err(|e| format!("OAuth data parsing failed: {}", e))?
        .ok_or("Refresh Token not included in data (Field 3)")?;
        
    String::from_utf8(refresh_bytes)
        .map_err(|_| "Refresh Token is not UTF-8 encoded".to_string())
}

/// Get current Refresh Token from default database (backwards compatibility)
pub fn get_refresh_token_from_db() -> Result<String, String> {
    let db_path = db::get_db_path()?;
    extract_refresh_token_from_file(&db_path)
}
