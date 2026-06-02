//! WebSocket bridge server for the Chrome extension.
//!
//! Exposes a local WebSocket endpoint that the acrawl Chrome extension connects to,
//! allowing the extension to act as a browser backend for the crawler agent.

mod auth;
mod http;
mod session;

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use acrawl_core::config_home_dir;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};

use tokio_tungstenite::tungstenite::http::{self as ws_http};

pub use auth::generate_bridge_token;
use auth::{validate_ws_upgrade, RateEntry};
use http::{send_raw_http_error, serve_health};
use session::{run_ws_session, CommandRx};

/// A command sent from the crawler to the Chrome extension via WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeCommand {
    pub id: u64,
    pub action: String,
    pub payload: serde_json::Value,
}

/// A response from the Chrome extension back to the crawler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Errors from starting the bridge server.
#[derive(Debug)]
pub enum WsBridgeError {
    Bind(std::io::Error),
    BridgeFileWrite(std::io::Error),
}

impl fmt::Display for WsBridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(e) => write!(f, "failed to bind WebSocket server: {e}"),
            Self::BridgeFileWrite(e) => write!(f, "failed to write bridge.json: {e}"),
        }
    }
}

impl std::error::Error for WsBridgeError {}

/// Handle to the running WebSocket bridge server.
///
/// Dropping signals shutdown and removes `~/.acrawl/bridge.json`.
pub struct WsBridgeServer {
    port: u16,
    command_tx: mpsc::Sender<(BridgeCommand, oneshot::Sender<BridgeResponse>)>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    bridge_file_path: PathBuf,
    client_connected_rx: watch::Receiver<bool>,
    _task: tokio::task::JoinHandle<()>,
}

impl WsBridgeServer {
    /// Start the bridge server on `127.0.0.1:<port>`.
    ///
    /// Writes `~/.acrawl/bridge.json` with `{"port": N, "pid": N}` for discovery.
    ///
    /// # Errors
    ///
    /// Returns `WsBridgeError::Bind` if the TCP listener cannot bind, or
    /// `WsBridgeError::BridgeFileWrite` if the discovery file cannot be written.
    #[allow(clippy::unused_async)]
    pub async fn start(port: u16, token: String) -> Result<Self, WsBridgeError> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = try_bind_with_retry(addr).map_err(WsBridgeError::Bind)?;
        let actual_port = listener.local_addr().map_err(WsBridgeError::Bind)?.port();

        let bridge_file_path = config_home_dir().join("bridge.json");
        let bridge_info = serde_json::json!({
            "port": actual_port,
            "pid": std::process::id(),
        });
        if let Some(parent) = bridge_file_path.parent() {
            std::fs::create_dir_all(parent).map_err(WsBridgeError::BridgeFileWrite)?;
        }
        std::fs::write(&bridge_file_path, bridge_info.to_string())
            .map_err(WsBridgeError::BridgeFileWrite)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&bridge_file_path, std::fs::Permissions::from_mode(0o600));
        }
        #[cfg(windows)]
        {
            if let (Some(path_str), Ok(username)) =
                (bridge_file_path.to_str(), std::env::var("USERNAME"))
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("icacls")
                    .args([
                        path_str,
                        "/inheritance:r",
                        "/grant:r",
                        &format!("{username}:(R,W)"),
                    ])
                    .creation_flags(0x0800_0000)
                    .output();
            }
        }

        let (command_tx, command_rx) =
            mpsc::channel::<(BridgeCommand, oneshot::Sender<BridgeResponse>)>(32);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (client_connected_tx, client_connected_rx) = watch::channel(false);

        let state = Arc::new(ServerState {
            token,
            port: actual_port,
            has_active_client: AtomicBool::new(false),
            client_connected_tx,
            rate_limiter: tokio::sync::Mutex::new(HashMap::new()),
        });

        let task = tokio::spawn(run_server(listener, state, command_rx, shutdown_rx));

        Ok(Self {
            port: actual_port,
            command_tx,
            shutdown_tx: Some(shutdown_tx),
            bridge_file_path,
            client_connected_rx,
            _task: task,
        })
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn command_sender(&self) -> mpsc::Sender<(BridgeCommand, oneshot::Sender<BridgeResponse>)> {
        self.command_tx.clone()
    }

    #[must_use]
    pub fn is_client_connected(&self) -> bool {
        *self.client_connected_rx.borrow()
    }

    #[must_use]
    pub fn connection_watcher(&self) -> watch::Receiver<bool> {
        self.client_connected_rx.clone()
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = std::fs::remove_file(&self.bridge_file_path);
    }

    pub async fn wait_for_connection(&mut self, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if *self.client_connected_rx.borrow() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            if tokio::time::timeout(remaining, self.client_connected_rx.changed())
                .await
                .is_err()
            {
                return *self.client_connected_rx.borrow();
            }
        }
    }
}

impl Drop for WsBridgeServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Server internals
// ---------------------------------------------------------------------------

struct ServerState {
    token: String,
    port: u16,
    has_active_client: AtomicBool,
    client_connected_tx: watch::Sender<bool>,
    rate_limiter: tokio::sync::Mutex<HashMap<IpAddr, RateEntry>>,
}

async fn run_server(
    listener: TcpListener,
    state: Arc<ServerState>,
    command_rx: mpsc::Receiver<(BridgeCommand, oneshot::Sender<BridgeResponse>)>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let command_rx: CommandRx = Arc::new(tokio::sync::Mutex::new(command_rx));

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                if let Ok((stream, peer_addr)) = accept_result {
                    let st = Arc::clone(&state);
                    let rx = Arc::clone(&command_rx);
                    tokio::spawn(handle_incoming(stream, st, rx, peer_addr));
                }
            }
            _ = &mut shutdown_rx => break,
        }
    }
}

async fn handle_incoming(
    stream: TcpStream,
    state: Arc<ServerState>,
    command_rx: CommandRx,
    peer_addr: SocketAddr,
) {
    let mut buf = [0u8; 1024];
    let n = match stream.peek(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let preview = String::from_utf8_lossy(&buf[..n]);

    if preview.starts_with("GET /health") && !preview.contains("Upgrade:") {
        serve_health(stream, state.port).await;
        return;
    }

    {
        let client_ip = peer_addr.ip();
        let mut limiter = state.rate_limiter.lock().await;
        let entry = limiter.entry(client_ip).or_insert(RateEntry {
            failures: 0,
            window_start: Instant::now(),
        });

        if entry.window_start.elapsed().as_secs() >= 60 {
            entry.failures = 0;
            entry.window_start = Instant::now();
        }

        if entry.failures >= 5 {
            drop(limiter);
            send_raw_http_error(stream, 429, "Too Many Requests: rate limit exceeded").await;
            return;
        }
    }

    if state
        .has_active_client
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        send_raw_http_error(stream, 409, "Conflict: another client is already connected").await;
        return;
    }

    let token_clone = state.token.clone();
    #[allow(clippy::result_large_err)]
    let ws_result = tokio_tungstenite::accept_hdr_async(
        stream,
        move |req: &ws_http::Request<()>, resp: ws_http::Response<()>| {
            validate_ws_upgrade(&token_clone, req, resp)
        },
    )
    .await;

    if let Ok(ws) = ws_result {
        let _ = state.client_connected_tx.send(true);
        run_ws_session(ws, command_rx).await;
        let _ = state.client_connected_tx.send(false);
    } else {
        let client_ip = peer_addr.ip();
        let mut limiter = state.rate_limiter.lock().await;
        if let Some(entry) = limiter.get_mut(&client_ip) {
            entry.failures = entry.failures.saturating_add(1);
        }
    }

    state.has_active_client.store(false, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Binding helpers
// ---------------------------------------------------------------------------

fn bind_with_reuse(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    #[cfg(not(target_os = "windows"))]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    TcpListener::from_std(socket.into())
}

fn try_bind_with_retry(addr: SocketAddr) -> std::io::Result<TcpListener> {
    match bind_with_reuse(addr) {
        Ok(listener) => Ok(listener),
        Err(e) if is_port_conflict(&e) => {
            clean_stale_bridge_file(addr.port());
            std::thread::sleep(std::time::Duration::from_millis(200));
            bind_with_reuse(addr)
        }
        Err(e) => Err(e),
    }
}

fn is_port_conflict(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(10048 | 10013 | 98 | 48))
}

fn clean_stale_bridge_file(expected_port: u16) {
    let bridge_file = config_home_dir().join("bridge.json");
    let Ok(content) = std::fs::read_to_string(&bridge_file) else {
        return;
    };
    let Ok(info) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    if info["port"].as_u64() == Some(u64::from(expected_port)) {
        let _ = std::fs::remove_file(&bridge_file);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::http::StatusCode;

    #[test]
    fn constant_time_eq_same() {
        assert!(auth::generate_bridge_token().len() == 64);
    }

    #[test]
    fn generate_bridge_token_is_64_hex_chars() {
        let token = generate_bridge_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn bridge_command_roundtrip() {
        let cmd = BridgeCommand {
            id: 42,
            action: "navigate".into(),
            payload: serde_json::json!({"url": "https://example.com"}),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: BridgeCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, 42);
        assert_eq!(parsed.action, "navigate");
    }

    #[test]
    fn bridge_response_skips_none_fields() {
        let resp = BridgeResponse {
            id: 1,
            ok: true,
            result: Some(serde_json::json!({"title": "Example"})),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("error"));

        let resp2 = BridgeResponse {
            id: 2,
            ok: false,
            result: None,
            error: Some("timeout".into()),
        };
        let json2 = serde_json::to_string(&resp2).unwrap();
        assert!(!json2.contains("result"));
        assert!(json2.contains("timeout"));
    }

    #[test]
    fn validate_ws_upgrade_rejects_wrong_path() {
        let req = ws_http::Request::builder()
            .uri("http://localhost/wrong")
            .body(())
            .unwrap();
        let resp = ws_http::Response::builder().body(()).unwrap();
        let result = validate_ws_upgrade("secret", &req, resp);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn validate_ws_upgrade_rejects_bad_origin() {
        let req = ws_http::Request::builder()
            .uri("http://localhost/bridge?token=secret")
            .header("origin", "https://evil.com")
            .body(())
            .unwrap();
        let resp = ws_http::Response::builder().body(()).unwrap();
        let result = validate_ws_upgrade("secret", &req, resp);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn validate_ws_upgrade_rejects_missing_token() {
        let req = ws_http::Request::builder()
            .uri("http://localhost/bridge")
            .body(())
            .unwrap();
        let resp = ws_http::Response::builder().body(()).unwrap();
        let result = validate_ws_upgrade("secret", &req, resp);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn validate_ws_upgrade_rejects_wrong_token() {
        let req = ws_http::Request::builder()
            .uri("http://localhost/bridge?token=wrong")
            .body(())
            .unwrap();
        let resp = ws_http::Response::builder().body(()).unwrap();
        let result = validate_ws_upgrade("secret", &req, resp);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn validate_ws_upgrade_accepts_valid_request() {
        let req = ws_http::Request::builder()
            .uri("http://localhost/bridge?token=secret")
            .header(
                "origin",
                "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaabb",
            )
            .body(())
            .unwrap();
        let resp = ws_http::Response::builder().body(()).unwrap();
        let result = validate_ws_upgrade("secret", &req, resp);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_ws_upgrade_accepts_edge_extension_origin() {
        let req = ws_http::Request::builder()
            .uri("http://localhost/bridge?token=secret")
            .header(
                "origin",
                "edge-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaabb",
            )
            .body(())
            .unwrap();
        let resp = ws_http::Response::builder().body(()).unwrap();
        let result = validate_ws_upgrade("secret", &req, resp);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_ws_upgrade_accepts_absent_origin() {
        let req = ws_http::Request::builder()
            .uri("http://localhost/bridge?token=mytoken")
            .body(())
            .unwrap();
        let resp = ws_http::Response::builder().body(()).unwrap();
        let result = validate_ws_upgrade("mytoken", &req, resp);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rate_limiter_per_ip_isolation() {
        let limiter: tokio::sync::Mutex<HashMap<IpAddr, RateEntry>> =
            tokio::sync::Mutex::new(HashMap::new());

        let ip1: IpAddr = "127.0.0.1".parse().unwrap();
        let ip2: IpAddr = "127.0.0.2".parse().unwrap();

        {
            let mut map = limiter.lock().await;
            map.insert(
                ip1,
                RateEntry {
                    failures: 5,
                    window_start: Instant::now(),
                },
            );
            map.insert(
                ip2,
                RateEntry {
                    failures: 2,
                    window_start: Instant::now(),
                },
            );
        }

        let map = limiter.lock().await;
        assert!(map[&ip1].failures >= 5);
        assert!(map[&ip2].failures < 5);
    }
}
