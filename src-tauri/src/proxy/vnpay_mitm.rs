// VNPAY Transparent MITM Proxy
// Listens on localhost and forwards intercepted requests to genai.vnpay.vn

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// VNPAY Transparent Proxy State
pub struct VnpayMitmProxy {
    running: Arc<RwLock<bool>>,
    port: u16,
}

impl VnpayMitmProxy {
    pub fn new(port: u16) -> Self {
        Self {
            running: Arc::new(RwLock::new(false)),
            port,
        }
    }

    pub async fn start(&self) -> Result<(), String> {
        let addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind port {}: {}", self.port, e))?;

        {
            let mut running = self.running.write().await;
            *running = true;
        }

        tracing::info!("[VNPAY-MITM] Transparent proxy listening on {}", addr);

        loop {
            match listener.accept().await {
                Ok((stream, client_addr)) => {
                    let running = self.running.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, client_addr).await {
                            tracing::error!("[VNPAY-MITM] Connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("[VNPAY-MITM] Accept error: {}", e);
                }
            }

            // Check if stopped
            let running = self.running.read().await;
            if !*running {
                tracing::info!("[VNPAY-MITM] Proxy stopped");
                break;
            }
        }

        Ok(())
    }

    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
        tracing::info!("[VNPAY-MITM] Stop signal sent");
    }

    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}

/// Handle a single connection
async fn handle_connection(mut client_stream: TcpStream, client_addr: SocketAddr) -> Result<(), io::Error> {
    // Read the HTTP request
    let mut buffer = vec![0u8; 8192];
    let n = client_stream.read(&mut buffer).await?;
    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buffer[..n]);
    tracing::debug!("[VNPAY-MITM] Request from {}: {}...", client_addr, &request[..100.min(n)]);

    // Parse the request to extract Host and path
    let (host, path, method) = parse_http_request(&request);

    // Connect to VNPAY
    let vnpay_addr = "genai.vnpay.vn:443";
    let mut upstream = TcpStream::connect(vnpay_addr).await?;

    // Transform request to VNPAY
    let transformed = transform_request(&request, &path);

    // Forward to upstream
    upstream.write_all(transformed.as_bytes()).await?;

    // Read response from upstream
    let mut response_buf = vec![0u8; 32768];
    let mut total_read = 0;

    // Read and forward response
    loop {
        let n = upstream.read(&mut response_buf[..]).await?;
        if n == 0 {
            break;
        }
        client_stream.write_all(&response_buf[..n]).await?;
        total_read += n;

        if total_read > 1024 * 1024 {
            // Limit response size
            break;
        }
    }

    tracing::debug!("[VNPAY-MITM] Forwarded {} bytes to {}", total_read, client_addr);
    Ok(())
}

/// Parse HTTP request to extract host, path and method
fn parse_http_request(request: &str) -> (String, String, String) {
    let lines: Vec<&str> = request.lines().collect();
    if lines.is_empty() {
        return (String::new(), String::new(), String::new());
    }

    // Parse first line: GET /path HTTP/1.1
    let parts: Vec<&str> = lines[0].split_whitespace().collect();
    let method = parts.first().map(|s| s.to_string()).unwrap_or_default();
    let path = parts.get(1).map(|s| s.to_string()).unwrap_or_default();

    // Parse Host header
    let host = lines
        .iter()
        .find(|l| l.to_lowercase().starts_with("host:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    (host, path, method)
}

/// Transform HTTP request for VNPAY endpoint
fn transform_request(request: &str, _path: &str) -> String {
    // For HTTPS, we need to handle CONNECT tunneling
    // For HTTP, we can directly forward with modified Host header

    // For now, just forward as-is since the /etc/hosts redirect handles DNS
    // The proxy just needs to forward the bytes
    request.to_string()
}