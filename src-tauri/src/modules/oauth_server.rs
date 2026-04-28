use crate::modules::oauth;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Mutex, OnceLock};
use tauri::Url;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::watch;

/// Derive a 32-byte AES key from passphrase using SHA-256 (mirrors Go's deriveKey).
fn derive_key(passphrase: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(passphrase.as_bytes());
    hasher.finalize().into()
}

/// Decrypt AES-256-GCM ciphertext produced by the Go server.
/// Format: base64( nonce(12 bytes) || ciphertext+tag )
fn aes_gcm_decrypt(encrypted_b64: &str, passphrase: &str) -> Result<Vec<u8>, String> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(encrypted_b64.trim())
        .map_err(|e| format!("base64_decode_error: {}", e))?;

    let nonce_size = 12; // GCM standard nonce size
    if data.len() < nonce_size {
        return Err("encrypted_data_too_short".to_string());
    }

    let (nonce_bytes, ciphertext) = data.split_at(nonce_size);
    let key_bytes = derive_key(passphrase);
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("aes_gcm_decrypt_error: {}", e))
}

/// Extract the `payload` query parameter from an HTTP GET request string.
/// The request line looks like: `GET /sso-callback?payload=<url-encoded-data> HTTP/1.1`
fn extract_get_payload(request: &str) -> String {
    // Get the first line of the HTTP request
    let first_line = request.lines().next().unwrap_or("");

    // Find ?payload=
    if let Some(qs_start) = first_line.find("?payload=") {
        let after = &first_line[qs_start + "?payload=".len()..];
        // Trim to end of query (next & or space)
        let raw = after
            .split(|c| c == '&' || c == ' ')
            .next()
            .unwrap_or("")
            .trim();
        // URL decode: replace + with space then decode %XX
        url_decode(raw)
    } else {
        String::new()
    }
}

/// Minimal URL percent-decoder (handles %XX and + → space).
fn url_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(s) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                    if let Ok(b) = u8::from_str_radix(s, 16) {
                        out.push(b as char);
                        i += 3;
                        continue;
                    }
                }
                out.push('%');
                i += 1;
            }
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

struct OAuthFlowState {
    auth_url: String,
    #[allow(dead_code)]
    redirect_uri: String,
    state: String,
    cancel_tx: watch::Sender<bool>,
    code_tx: mpsc::Sender<Result<String, String>>,
    code_rx: Option<mpsc::Receiver<Result<String, String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VnpayAccount {
    pub email: String,
    pub refresh_token: String,
    #[serde(rename = "type", default)]
    pub account_type: String,
    #[serde(rename = "token", default)]
    pub anthropic_auth_token: Option<String>,
    #[serde(rename = "base_url", default)]
    pub anthropic_base_url: Option<String>,
}

struct VnpaySsoState {
    port: u16,
    cancel_tx: watch::Sender<bool>,
    #[allow(dead_code)]
    accounts_tx: mpsc::Sender<Vec<VnpayAccount>>,
}

struct VnpayJwtState {
    #[allow(dead_code)]
    port: u16,
    cancel_tx: watch::Sender<bool>,
}

static OAUTH_FLOW_STATE: OnceLock<Mutex<Option<OAuthFlowState>>> = OnceLock::new();
static VNPAY_SSO_STATE: OnceLock<Mutex<Option<VnpaySsoState>>> = OnceLock::new();
static VNPAY_JWT_STATE: OnceLock<Mutex<Option<VnpayJwtState>>> = OnceLock::new();

fn get_oauth_flow_state() -> &'static Mutex<Option<OAuthFlowState>> {
    OAUTH_FLOW_STATE.get_or_init(|| Mutex::new(None))
}

fn get_vnpay_sso_state() -> &'static Mutex<Option<VnpaySsoState>> {
    VNPAY_SSO_STATE.get_or_init(|| Mutex::new(None))
}

fn get_vnpay_jwt_state() -> &'static Mutex<Option<VnpayJwtState>> {
    VNPAY_JWT_STATE.get_or_init(|| Mutex::new(None))
}

/// Extract `token` query parameter from an HTTP GET request line.
fn extract_get_token(request: &str) -> String {
    let first_line = request.lines().next().unwrap_or("");
    let qs_idx = match first_line.find('?') {
        Some(i) => i + 1,
        None => return String::new(),
    };
    let query = first_line[qs_idx..]
        .split(' ')
        .next()
        .unwrap_or("");
    for pair in query.split('&') {
        if let Some(rest) = pair.strip_prefix("token=") {
            return url_decode(rest);
        }
    }
    String::new()
}

fn oauth_success_html() -> &'static str {
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
    <html>\
    <body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
    <h1 style='color: green;'>✅ Authorization Successful!</h1>\
    <p>You can close this window and return to the application.</p>\
    <script>setTimeout(function() { window.close(); }, 2000);</script>\
    </body>\
    </html>"
}

fn oauth_fail_html() -> &'static str {
    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
    <html>\
    <body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
    <h1 style='color: red;'>❌ Authorization Failed</h1>\
    <p>Failed to obtain Authorization Code. Please return to the app and try again.</p>\
    </body>\
    </html>"
}

async fn ensure_oauth_flow_prepared(
    app_handle: Option<tauri::AppHandle>,
) -> Result<String, String> {
    // Return URL if flow already exists and is still "fresh" (receiver hasn't been taken)
    if let Ok(mut state) = get_oauth_flow_state().lock() {
        if let Some(s) = state.as_mut() {
            if s.code_rx.is_some() {
                return Ok(s.auth_url.clone());
            } else {
                // Flow is already "in progress" (rx taken), but user requested a NEW one.
                // Force cancel the old one to allow a new attempt.
                let _ = s.cancel_tx.send(true);
                *state = None;
            }
        }
    }

    // Create loopback listeners.
    // Some browsers resolve `localhost` to IPv6 (::1). To avoid "localhost refused connection",
    // we try to listen on BOTH IPv6 and IPv4 with the same port when possible.
    let mut ipv4_listener: Option<TcpListener> = None;
    let mut ipv6_listener: Option<TcpListener> = None;

    // Prefer creating one listener on an ephemeral port first, then bind the other stack to same port.
    // If both are available -> use `http://localhost:<port>` as redirect URI.
    // If only one is available -> use an explicit IP to force correct stack.
    let port: u16;
    match TcpListener::bind("[::1]:0").await {
        Ok(l6) => {
            port = l6
                .local_addr()
                .map_err(|e| format!("failed_to_get_local_port: {}", e))?
                .port();
            ipv6_listener = Some(l6);

            match TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                Ok(l4) => ipv4_listener = Some(l4),
                Err(e) => {
                    crate::modules::logger::log_warn(&format!(
                        "failed_to_bind_ipv4_callback_port_127_0_0_1:{} (will only listen on IPv6): {}",
                        port, e
                    ));
                }
            }
        }
        Err(_) => {
            let l4 = TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|e| format!("failed_to_bind_local_port: {}", e))?;
            port = l4
                .local_addr()
                .map_err(|e| format!("failed_to_get_local_port: {}", e))?
                .port();
            ipv4_listener = Some(l4);

            match TcpListener::bind(format!("[::1]:{}", port)).await {
                Ok(l6) => ipv6_listener = Some(l6),
                Err(e) => {
                    crate::modules::logger::log_warn(&format!(
                        "failed_to_bind_ipv6_callback_port_::1:{} (will only listen on IPv4): {}",
                        port, e
                    ));
                }
            }
        }
    }

    let has_ipv4 = ipv4_listener.is_some();
    let has_ipv6 = ipv6_listener.is_some();

    let redirect_uri = if has_ipv4 && has_ipv6 {
        format!("http://localhost:{}/oauth-callback", port)
    } else if has_ipv4 {
        format!("http://127.0.0.1:{}/oauth-callback", port)
    } else {
        format!("http://[::1]:{}/oauth-callback", port)
    };

    let state_str = uuid::Uuid::new_v4().to_string();
    let auth_url = oauth::get_auth_url(&redirect_uri, &state_str);

    // Cancellation signal (supports multiple consumers)
    let (cancel_tx, cancel_rx) = watch::channel(false);
    // Use mpsc instead of oneshot to allow multiple senders (listener OR manual input)
    let (code_tx, code_rx) = mpsc::channel::<Result<String, String>>(1);

    // Start listeners immediately: even if the user authorizes before clicking "Start OAuth",
    // the browser can still hit our callback and finish the flow.
    let app_handle_for_tasks = app_handle.clone();

    if let Some(l4) = ipv4_listener {
        let tx = code_tx.clone();
        let mut rx = cancel_rx.clone();
        let app_handle = app_handle_for_tasks.clone();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = tokio::select! {
                res = l4.accept() => res.map_err(|e| format!("failed_to_accept_connection: {}", e)),
                _ = rx.changed() => Err("OAuth cancelled".to_string()),
            } {
                // Reuse the existing parsing/response code by constructing a temporary listener task
                // that sends into the shared mpsc channel.
                let mut buffer = [0u8; 4096];
                let bytes_read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..bytes_read]);

                // [FIX #931/850/778] More robust parsing and detailed logging
                let query_params = request
                    .lines()
                    .next()
                    .and_then(|line| {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            Some(parts[1])
                        } else {
                            None
                        }
                    })
                    .and_then(|path| {
                        // Use a dummy base for parsing; redirect_uri is already set to localhost
                        Url::parse(&format!("http://localhost{}", path)).ok()
                    })
                    .map(|url| {
                        let mut code = None;
                        let mut state = None;
                        for (k, v) in url.query_pairs() {
                            if k == "code" {
                                code = Some(v.to_string());
                            } else if k == "state" {
                                state = Some(v.to_string());
                            }
                        }
                        (code, state)
                    });

                let (code, received_state) = match query_params {
                    Some((c, s)) => (c, s),
                    None => (None, None),
                };

                if code.is_none() && bytes_read > 0 {
                    crate::modules::logger::log_error(&format!(
                        "OAuth callback failed to parse code. Raw request (first 512 bytes): {}",
                        &request.chars().take(512).collect::<String>()
                    ));
                }

                // Verify state
                let state_valid = {
                    if let Ok(lock) = get_oauth_flow_state().lock() {
                        if let Some(s) = lock.as_ref() {
                            received_state.as_ref() == Some(&s.state)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                let (result, response_html) = match (code, state_valid) {
                    (Some(code), true) => {
                        crate::modules::logger::log_info(
                            "Successfully captured OAuth code from IPv4 listener",
                        );
                        (Ok(code), oauth_success_html())
                    }
                    (Some(_), false) => {
                        crate::modules::logger::log_error(
                            "OAuth callback state mismatch (CSRF protection)",
                        );
                        (Err("OAuth state mismatch".to_string()), oauth_fail_html())
                    }
                    (None, _) => (
                        Err("Failed to get Authorization Code in callback".to_string()),
                        oauth_fail_html(),
                    ),
                };

                let _ = stream.write_all(response_html.as_bytes()).await;
                let _ = stream.flush().await;

                if let Some(h) = app_handle {
                    use tauri::Emitter;
                    let _ = h.emit("oauth-callback-received", ());
                }
                let _ = tx.send(result).await;
            }
        });
    }

    if let Some(l6) = ipv6_listener {
        let tx = code_tx.clone();
        let mut rx = cancel_rx;
        let app_handle = app_handle_for_tasks;
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = tokio::select! {
                res = l6.accept() => res.map_err(|e| format!("failed_to_accept_connection: {}", e)),
                _ = rx.changed() => Err("OAuth cancelled".to_string()),
            } {
                let mut buffer = [0u8; 4096];
                let bytes_read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..bytes_read]);

                let query_params = request
                    .lines()
                    .next()
                    .and_then(|line| {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            Some(parts[1])
                        } else {
                            None
                        }
                    })
                    .and_then(|path| Url::parse(&format!("http://localhost{}", path)).ok())
                    .map(|url| {
                        let mut code = None;
                        let mut state = None;
                        for (k, v) in url.query_pairs() {
                            if k == "code" {
                                code = Some(v.to_string());
                            } else if k == "state" {
                                state = Some(v.to_string());
                            }
                        }
                        (code, state)
                    });

                let (code, received_state) = match query_params {
                    Some((c, s)) => (c, s),
                    None => (None, None),
                };

                if code.is_none() && bytes_read > 0 {
                    crate::modules::logger::log_error(&format!(
                        "OAuth callback failed to parse code (IPv6). Raw request: {}",
                        &request.chars().take(512).collect::<String>()
                    ));
                }

                // Verify state
                let state_valid = {
                    if let Ok(lock) = get_oauth_flow_state().lock() {
                        if let Some(s) = lock.as_ref() {
                            received_state.as_ref() == Some(&s.state)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                let (result, response_html) = match (code, state_valid) {
                    (Some(code), true) => {
                        crate::modules::logger::log_info(
                            "Successfully captured OAuth code from IPv6 listener",
                        );
                        (Ok(code), oauth_success_html())
                    }
                    (Some(_), false) => {
                        crate::modules::logger::log_error(
                            "OAuth callback state mismatch (IPv6 CSRF protection)",
                        );
                        (Err("OAuth state mismatch".to_string()), oauth_fail_html())
                    }
                    (None, _) => (
                        Err("Failed to get Authorization Code in callback".to_string()),
                        oauth_fail_html(),
                    ),
                };

                let _ = stream.write_all(response_html.as_bytes()).await;
                let _ = stream.flush().await;

                if let Some(h) = app_handle {
                    use tauri::Emitter;
                    let _ = h.emit("oauth-callback-received", ());
                }
                let _ = tx.send(result).await;
            }
        });
    }

    // Save state
    if let Ok(mut state) = get_oauth_flow_state().lock() {
        *state = Some(OAuthFlowState {
            auth_url: auth_url.clone(),
            redirect_uri,
            state: state_str,
            cancel_tx,
            code_tx,
            code_rx: Some(code_rx),
        });
    }

    // Send event to frontend (for display/copying link)
    if let Some(h) = app_handle {
        use tauri::Emitter;
        let _ = h.emit("oauth-url-generated", &auth_url);
    }

    Ok(auth_url)
}

/// Pre-generate OAuth URL (does not open browser, does not block waiting for callback)
pub async fn prepare_oauth_url(app_handle: Option<tauri::AppHandle>) -> Result<String, String> {
    ensure_oauth_flow_prepared(app_handle).await
}

/// Cancel current OAuth flow
pub fn cancel_oauth_flow() {
    if let Ok(mut state) = get_oauth_flow_state().lock() {
        if let Some(s) = state.take() {
            let _ = s.cancel_tx.send(true);
            crate::modules::logger::log_info("Sent OAuth cancellation signal");
        }
    }
}

/// Start OAuth flow and wait for callback, then exchange token
pub async fn start_oauth_flow(
    app_handle: Option<tauri::AppHandle>,
) -> Result<oauth::TokenResponse, String> {
    // Ensure URL + listener are ready (this way if the user authorizes first, it won't get stuck)
    let auth_url = ensure_oauth_flow_prepared(app_handle.clone()).await?;

    if let Some(h) = app_handle {
        // Open default browser
        use tauri_plugin_opener::OpenerExt;
        h.opener()
            .open_url(&auth_url, None::<String>)
            .map_err(|e| format!("failed_to_open_browser: {}", e))?;
    }

    // Take code_rx to wait for it
    let (mut code_rx, redirect_uri) = {
        let mut lock = get_oauth_flow_state()
            .lock()
            .map_err(|_| "OAuth state lock corrupted".to_string())?;
        let Some(state) = lock.as_mut() else {
            return Err("OAuth state does not exist".to_string());
        };
        let rx = state
            .code_rx
            .take()
            .ok_or_else(|| "OAuth authorization already in progress".to_string())?;
        (rx, state.redirect_uri.clone())
    };

    // Wait for code (if user has already authorized, this returns immediately)
    // For mpsc, we use recv()
    let code = match code_rx.recv().await {
        Some(Ok(code)) => code,
        Some(Err(e)) => return Err(e),
        None => return Err("OAuth flow channel closed unexpectedly".to_string()),
    };

    // Clean up flow state (release cancel_tx, etc.)
    if let Ok(mut lock) = get_oauth_flow_state().lock() {
        *lock = None;
    }

    oauth::exchange_code(&code, &redirect_uri).await
}

/// Завершить OAuth flow без открытия браузера.
/// Предполагается, что пользователь открыл ссылку вручную (или ранее была открыта),
/// а мы только ждём callback и обмениваем code на token.
pub async fn complete_oauth_flow(
    app_handle: Option<tauri::AppHandle>,
) -> Result<oauth::TokenResponse, String> {
    // Ensure URL + listeners exist
    let _ = ensure_oauth_flow_prepared(app_handle).await?;

    // Take receiver to wait for code
    let (mut code_rx, redirect_uri) = {
        let mut lock = get_oauth_flow_state()
            .lock()
            .map_err(|_| "OAuth state lock corrupted".to_string())?;
        let Some(state) = lock.as_mut() else {
            return Err("OAuth state does not exist".to_string());
        };
        let rx = state
            .code_rx
            .take()
            .ok_or_else(|| "OAuth authorization already in progress".to_string())?;
        (rx, state.redirect_uri.clone())
    };

    let code = match code_rx.recv().await {
        Some(Ok(code)) => code,
        Some(Err(e)) => return Err(e),
        None => return Err("OAuth flow channel closed unexpectedly".to_string()),
    };

    if let Ok(mut lock) = get_oauth_flow_state().lock() {
        *lock = None;
    }

    oauth::exchange_code(&code, &redirect_uri).await
}

/// Manually submit an OAuth code to complete the flow.
/// This is used when the user manually copies the code/URL from the browser
/// because the localhost callback couldn't be reached (e.g. in Docker/remote).
pub async fn submit_oauth_code(
    code_input: String,
    state_input: Option<String>,
) -> Result<(), String> {
    let tx = {
        let lock = get_oauth_flow_state().lock().map_err(|e| e.to_string())?;
        if let Some(state) = lock.as_ref() {
            // Verify state if provided
            if let Some(provided_state) = state_input {
                if provided_state != state.state {
                    return Err("OAuth state mismatch (CSRF protection)".to_string());
                }
            }
            state.code_tx.clone()
        } else {
            return Err("No active OAuth flow found".to_string());
        }
    };

    // Extract code if it's a URL
    let code = if code_input.starts_with("http") {
        if let Ok(url) = Url::parse(&code_input) {
            url.query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string())
                .unwrap_or(code_input)
        } else {
            code_input
        }
    } else {
        code_input
    };

    crate::modules::logger::log_info("Received manual OAuth code submission");

    // Send to the channel
    tx.send(Ok(code))
        .await
        .map_err(|_| "Failed to send code to OAuth flow (receiver dropped)".to_string())?;

    Ok(())
}
/// Manually prepare an OAuth flow without starting listeners.
/// Useful for Web/Docker environments where we only need manual code submission.
pub fn prepare_oauth_flow_manually(
    redirect_uri: String,
    state_str: String,
) -> Result<(String, mpsc::Receiver<Result<String, String>>), String> {
    let auth_url = oauth::get_auth_url(&redirect_uri, &state_str);

    // Check if we can reuse existing state
    if let Ok(mut lock) = get_oauth_flow_state().lock() {
        if let Some(s) = lock.as_mut() {
            // If we already have a code_rx, we can't easily "steal" it again because it's already returned.
            // But if this is a NEW request (different state), we should overwrite.
            // For now, let's just clear and restart to be safe.
            let _ = s.cancel_tx.send(true);
            *lock = None;
        }
    }

    let (cancel_tx, _cancel_rx) = watch::channel(false);
    let (code_tx, code_rx) = mpsc::channel(1);

    if let Ok(mut state) = get_oauth_flow_state().lock() {
        *state = Some(OAuthFlowState {
            auth_url: auth_url.clone(),
            redirect_uri: redirect_uri.clone(),
            state: state_str,
            cancel_tx,
            code_tx,
            code_rx: None, // We return it directly
        });
    }

    Ok((auth_url, code_rx))
}

/// Prepare VNPAY SSO listener
/// Returns the port number to construct callback URL: http://localhost:{port}/sso-callback
pub async fn prepare_vnpay_sso_listener(
    app_handle: Option<tauri::AppHandle>,
) -> Result<u16, String> {
    // Cancel existing listener if any
    if let Ok(mut state) = get_vnpay_sso_state().lock() {
        if let Some(s) = state.take() {
            let _ = s.cancel_tx.send(true);
        }
    }

    // Create ephemeral listener
    let mut ipv4_listener: Option<TcpListener> = None;
    let mut ipv6_listener: Option<TcpListener> = None;

    let port: u16;
    match TcpListener::bind("[::1]:0").await {
        Ok(l6) => {
            port = l6
                .local_addr()
                .map_err(|e| format!("failed_to_get_local_port: {}", e))?
                .port();
            ipv6_listener = Some(l6);

            match TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                Ok(l4) => ipv4_listener = Some(l4),
                Err(e) => {
                    crate::modules::logger::log_warn(&format!(
                        "failed_to_bind_ipv4_sso_port_127_0_0_1:{} (will only listen on IPv6): {}",
                        port, e
                    ));
                }
            }
        }
        Err(_) => {
            let l4 = TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|e| format!("failed_to_bind_local_port: {}", e))?;
            port = l4
                .local_addr()
                .map_err(|e| format!("failed_to_get_local_port: {}", e))?
                .port();
            ipv4_listener = Some(l4);

            match TcpListener::bind(format!("[::1]:{}", port)).await {
                Ok(l6) => ipv6_listener = Some(l6),
                Err(e) => {
                    crate::modules::logger::log_warn(&format!(
                        "failed_to_bind_ipv6_sso_port_::1:{} (will only listen on IPv4): {}",
                        port, e
                    ));
                }
            }
        }
    }

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (accounts_tx, mut accounts_rx) = mpsc::channel::<Vec<VnpayAccount>>(1);

    // Start listeners for /sso-callback endpoint
    let app_handle_for_tasks = app_handle.clone();

    if let Some(l4) = ipv4_listener {
        let tx = accounts_tx.clone();
        let mut rx = cancel_rx.clone();
        let app_handle = app_handle_for_tasks.clone();

        tokio::spawn(async move {
            loop {
                let accept_result = tokio::select! {
                    res = l4.accept() => res,
                    _ = rx.changed() => break,
                };

                if let Ok((mut stream, _)) = accept_result {
                    let mut buffer = [0u8; 8192];
                    let bytes_read = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);

                    crate::modules::logger::log_info(&format!(
                        "VNPAY SSO callback received (IPv4): {} bytes",
                        bytes_read
                    ));

                    // Check if this is /sso-callback GET request
                    if request.contains("/sso-callback") {
                        // Extract payload from GET ?payload= query parameter
                        let payload = extract_get_payload(&request);

                        crate::modules::logger::log_info(&format!(
                            "VNPAY SSO payload (raw, {} bytes)",
                            payload.len()
                        ));

                        // Decrypt AES-256-GCM, fallback to raw JSON if not encrypted
                        let passphrase = crate::modules::config::get_vnpay_sso_passphrase();
                        let decrypted_body = match aes_gcm_decrypt(payload.trim(), &passphrase) {
                            Ok(plain) => {
                                crate::modules::logger::log_info(
                                    "VNPAY SSO payload decrypted successfully (IPv4)",
                                );
                                String::from_utf8(plain).unwrap_or_else(|_| payload.clone())
                            }
                            Err(e) => {
                                crate::modules::logger::log_warn(&format!(
                                    "VNPAY SSO AES decrypt failed (IPv4), trying raw JSON: {}",
                                    e
                                ));
                                payload.clone()
                            }
                        };

                        match serde_json::from_str::<Vec<VnpayAccount>>(&decrypted_body) {
                            Ok(accounts) => {
                                crate::modules::logger::log_info(&format!(
                                    "Successfully parsed {} VNPAY accounts",
                                    accounts.len()
                                ));
                                for (i, acc) in accounts.iter().enumerate() {
                                    crate::modules::logger::log_info(&format!(
                                        "  [{}] email={}, type={}",
                                        i + 1,
                                        acc.email,
                                        if acc.account_type.is_empty() {
                                            "(default)"
                                        } else {
                                            &acc.account_type
                                        }
                                    ));
                                }

                                // Send success response with JS to close browser
                                let response = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\n\r\n\
                                    <html><head><meta charset='utf-8'></head><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
                                    <h1 style='color: green;'>&#x2705; Đăng nhập thành công!</h1>\
                                    <p>Đã nhận {} tài khoản. Cửa sổ này sẽ tự đóng...</p>\
                                    <script>window.close(); setTimeout(function(){{ window.close(); }}, 500);</script>\
                                    </body></html>",
                                    accounts.len()
                                );
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.flush().await;

                                // Emit event to frontend with accounts
                                if let Some(h) = &app_handle {
                                    use tauri::Emitter;
                                    let _ = h.emit("vnpay-sso-accounts-received", accounts.clone());
                                }

                                // Send to channel
                                let _ = tx.send(accounts).await;
                                break; // Stop listener after successful callback
                            }
                            Err(e) => {
                                crate::modules::logger::log_error(&format!(
                                    "Failed to parse VNPAY accounts JSON: {}",
                                    e
                                ));
                                let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
                                    <html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
                                    <h1 style='color: red;'>&#x274C; Dữ liệu không hợp lệ</h1>\
                                    <p>Không thể phân tích dữ liệu tài khoản.</p>\
                                    </body></html>";
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.flush().await;
                            }
                        }
                    }
                }
            }
        });
    }

    if let Some(l6) = ipv6_listener {
        let tx = accounts_tx.clone();
        let mut rx = cancel_rx.clone();
        let app_handle = app_handle_for_tasks;

        tokio::spawn(async move {
            loop {
                let accept_result = tokio::select! {
                    res = l6.accept() => res,
                    _ = rx.changed() => break,
                };

                if let Ok((mut stream, _)) = accept_result {
                    let mut buffer = [0u8; 8192];
                    let bytes_read = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);

                    crate::modules::logger::log_info(&format!(
                        "VNPAY SSO callback received (IPv6): {} bytes",
                        bytes_read
                    ));

                    if request.contains("/sso-callback") {
                        // Extract payload from GET ?payload= query parameter
                        let payload = extract_get_payload(&request);

                        crate::modules::logger::log_info(&format!(
                            "VNPAY SSO payload (raw, {} bytes)",
                            payload.len()
                        ));

                        // Decrypt AES-256-GCM, fallback to raw JSON if not encrypted
                        let passphrase = crate::modules::config::get_vnpay_sso_passphrase();
                        let decrypted_body = match aes_gcm_decrypt(payload.trim(), &passphrase) {
                            Ok(plain) => {
                                crate::modules::logger::log_info(
                                    "VNPAY SSO payload decrypted successfully (IPv6)",
                                );
                                String::from_utf8(plain).unwrap_or_else(|_| payload.clone())
                            }
                            Err(e) => {
                                crate::modules::logger::log_warn(&format!(
                                    "VNPAY SSO AES decrypt failed (IPv6), trying raw JSON: {}",
                                    e
                                ));
                                payload.clone()
                            }
                        };
                        crate::modules::logger::log_info(&format!(
                            "VNPAY SSO Data Respone ({} )",
                            decrypted_body
                        ));

                        match serde_json::from_str::<Vec<VnpayAccount>>(&decrypted_body) {
                            Ok(accounts) => {
                                crate::modules::logger::log_info(&format!(
                                    "Successfully parsed {} VNPAY accounts",
                                    accounts.len()
                                ));
                                for (i, acc) in accounts.iter().enumerate() {
                                    crate::modules::logger::log_info(&format!(
                                        "  [{}] email={}, type={}",
                                        i + 1,
                                        acc.email,
                                        if acc.account_type.is_empty() {
                                            "(default)"
                                        } else {
                                            &acc.account_type
                                        }
                                    ));
                                }

                                let response = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\n\r\n\
                                    <html><head><meta charset='utf-8'></head><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
                                    <h1 style='color: green;'>&#x2705; Đăng nhập thành công!</h1>\
                                    <p>Đã nhận {} tài khoản. Cửa sổ này sẽ tự đóng...</p>\
                                    <script>window.close(); setTimeout(function(){{ window.close(); }}, 500);</script>\
                                    </body></html>",
                                    accounts.len()
                                );
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.flush().await;

                                if let Some(h) = &app_handle {
                                    use tauri::Emitter;
                                    let _ = h.emit("vnpay-sso-accounts-received", accounts.clone());
                                }

                                let _ = tx.send(accounts).await;
                                break;
                            }
                            Err(e) => {
                                crate::modules::logger::log_error(&format!(
                                    "Failed to parse VNPAY accounts JSON: {}",
                                    e
                                ));
                                let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
                                    <html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
                                    <h1 style='color: red;'>&#x274C; Dữ liệu không hợp lệ</h1>\
                                    <p>Không thể phân tích dữ liệu tài khoản.</p>\
                                    </body></html>";
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.flush().await;
                            }
                        }
                    }
                }
            }
        });
    }

    // Spawn a task to handle received accounts and save them
    if let Some(h) = app_handle {
        tokio::spawn(async move {
            if let Some(accounts) = accounts_rx.recv().await {
                crate::modules::logger::log_info(&format!(
                    "Processing {} VNPAY accounts for storage",
                    accounts.len()
                ));

                // Import accounts using AccountService
                let service = crate::modules::account_service::AccountService::new(
                    crate::modules::integration::SystemManager::Desktop(h.clone()),
                );

                for account_data in accounts {
                    // Anthropic accounts are handled separately — emit event for CLI Claude
                    if account_data.account_type == "anthropic" {
                        crate::modules::logger::log_info(&format!(
                            "Received anthropic account, emitting event for CLI Claude"
                        ));
                        #[derive(serde::Serialize, Clone)]
                        struct AnthropicPayload {
                            token: Option<String>,
                            base_url: Option<String>,
                        }
                        use tauri::Emitter;
                        let _ = h.emit(
                            "vnpay-anthropic-received",
                            AnthropicPayload {
                                token: account_data.anthropic_auth_token.clone(),
                                base_url: account_data.anthropic_base_url.clone(),
                            },
                        );
                        continue;
                    }

                    let account_type = if account_data.account_type.is_empty() {
                        None
                    } else {
                        Some(account_data.account_type.clone())
                    };

                    match service
                        .add_account(
                            &account_data.refresh_token,
                            account_type,
                            account_data.anthropic_auth_token.clone(),
                            account_data.anthropic_base_url.clone(),
                        )
                        .await
                    {
                        Ok(account) => {
                            crate::modules::logger::log_info(&format!(
                                "Successfully added VNPAY account: {}",
                                account.email
                            ));
                        }
                        Err(e) => {
                            crate::modules::logger::log_error(&format!(
                                "Failed to add VNPAY account {}: {}",
                                account_data.email, e
                            ));
                        }
                    }
                }

                // Emit completion event
                use tauri::Emitter;
                let _ = h.emit("vnpay-sso-import-completed", ());
            }
        });
    }

    // Save state
    if let Ok(mut state) = get_vnpay_sso_state().lock() {
        *state = Some(VnpaySsoState {
            port,
            cancel_tx,
            accounts_tx,
        });
    }

    crate::modules::logger::log_info(&format!("VNPAY SSO listener started on port {}", port));
    Ok(port)
}

/// Cancel VNPAY SSO listener
pub fn cancel_vnpay_sso_listener() {
    if let Ok(mut state) = get_vnpay_sso_state().lock() {
        if let Some(s) = state.take() {
            let _ = s.cancel_tx.send(true);
            crate::modules::logger::log_info("Cancelled VNPAY SSO listener");
        }
    }
}

/// Prepare VNPAY JWT listener for CLI VNPAY flow.
/// Listens on `/sso-callback?token=<JWT>`, writes token to ~/.claude/settings.json,
/// then emits "vnpay-cli-jwt-installed" event.
/// Returns the port number (used as `connectid` in https://genai.vnpay.vn/create-jwt-token).
pub async fn prepare_vnpay_jwt_listener(
    app_handle: Option<tauri::AppHandle>,
) -> Result<u16, String> {
    if let Ok(mut state) = get_vnpay_jwt_state().lock() {
        if let Some(s) = state.take() {
            let _ = s.cancel_tx.send(true);
        }
    }

    let mut ipv4_listener: Option<TcpListener> = None;
    let mut ipv6_listener: Option<TcpListener> = None;

    let port: u16;
    match TcpListener::bind("[::1]:0").await {
        Ok(l6) => {
            port = l6
                .local_addr()
                .map_err(|e| format!("failed_to_get_local_port: {}", e))?
                .port();
            ipv6_listener = Some(l6);

            match TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                Ok(l4) => ipv4_listener = Some(l4),
                Err(e) => {
                    crate::modules::logger::log_warn(&format!(
                        "failed_to_bind_ipv4_jwt_port_127_0_0_1:{} (will only listen on IPv6): {}",
                        port, e
                    ));
                }
            }
        }
        Err(_) => {
            let l4 = TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|e| format!("failed_to_bind_local_port: {}", e))?;
            port = l4
                .local_addr()
                .map_err(|e| format!("failed_to_get_local_port: {}", e))?
                .port();
            ipv4_listener = Some(l4);

            match TcpListener::bind(format!("[::1]:{}", port)).await {
                Ok(l6) => ipv6_listener = Some(l6),
                Err(e) => {
                    crate::modules::logger::log_warn(&format!(
                        "failed_to_bind_ipv6_jwt_port_::1:{} (will only listen on IPv4): {}",
                        port, e
                    ));
                }
            }
        }
    }

    let (cancel_tx, cancel_rx) = watch::channel(false);

    async fn handle_request(
        mut stream: tokio::net::TcpStream,
        app_handle: Option<tauri::AppHandle>,
    ) -> bool {
        let mut buffer = [0u8; 8192];
        let bytes_read = stream.read(&mut buffer).await.unwrap_or(0);
        if bytes_read == 0 {
            return false;
        }
        let request = String::from_utf8_lossy(&buffer[..bytes_read]);

        if !request.contains("/sso-callback") {
            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.flush().await;
            return false;
        }

        let token = extract_get_token(&request);
        if token.is_empty() {
            let resp = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
                <html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
                <h1 style='color: red;'>&#x274C; Thiếu JWT</h1>\
                <p>Không tìm thấy tham số token trong callback.</p>\
                </body></html>";
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.flush().await;
            return false;
        }

        match crate::modules::claude_settings::apply_vnpay_jwt(&token) {
            Ok(()) => {
                // Best-effort OTel telemetry profile setup
                let otel_added =
                    crate::modules::claude_settings::ensure_otel_telemetry().unwrap_or(false);

                let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
                    <html><head><meta charset='utf-8'></head>\
                    <body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
                    <h1 style='color: green;'>&#x2705; CLI VNPAY đã sẵn sàng!</h1>\
                    <p>Đã ghi token vào ~/.claude/settings.json. Cửa sổ này sẽ tự đóng...</p>\
                    <script>setTimeout(function(){ window.close(); }, 800);</script>\
                    </body></html>";
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.flush().await;

                if let Some(h) = app_handle {
                    use tauri::Emitter;
                    #[derive(serde::Serialize, Clone)]
                    struct Payload {
                        otel_added: bool,
                    }
                    let _ = h.emit(
                        "vnpay-cli-jwt-installed",
                        Payload { otel_added },
                    );
                }
                true
            }
            Err(e) => {
                crate::modules::logger::log_error(&format!(
                    "Failed to apply VNPAY JWT to settings: {}",
                    e
                ));
                let body = format!(
                    "<html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
                    <h1 style='color: red;'>&#x274C; Ghi cấu hình thất bại</h1>\
                    <p>{}</p></body></html>",
                    e
                );
                let resp = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.flush().await;
                false
            }
        }
    }

    if let Some(l4) = ipv4_listener {
        let mut rx = cancel_rx.clone();
        let app_handle = app_handle.clone();
        tokio::spawn(async move {
            loop {
                let accept_result = tokio::select! {
                    res = l4.accept() => res,
                    _ = rx.changed() => break,
                };
                if let Ok((stream, _)) = accept_result {
                    let app_handle = app_handle.clone();
                    let done = handle_request(stream, app_handle).await;
                    if done {
                        break;
                    }
                }
            }
        });
    }

    if let Some(l6) = ipv6_listener {
        let mut rx = cancel_rx.clone();
        let app_handle = app_handle.clone();
        tokio::spawn(async move {
            loop {
                let accept_result = tokio::select! {
                    res = l6.accept() => res,
                    _ = rx.changed() => break,
                };
                if let Ok((stream, _)) = accept_result {
                    let app_handle = app_handle.clone();
                    let done = handle_request(stream, app_handle).await;
                    if done {
                        break;
                    }
                }
            }
        });
    }

    if let Ok(mut state) = get_vnpay_jwt_state().lock() {
        *state = Some(VnpayJwtState { port, cancel_tx });
    }

    crate::modules::logger::log_info(&format!("VNPAY JWT listener started on port {}", port));
    Ok(port)
}

/// Cancel VNPAY JWT listener
pub fn cancel_vnpay_jwt_listener() {
    if let Ok(mut state) = get_vnpay_jwt_state().lock() {
        if let Some(s) = state.take() {
            let _ = s.cancel_tx.send(true);
            crate::modules::logger::log_info("Cancelled VNPAY JWT listener");
        }
    }
}
