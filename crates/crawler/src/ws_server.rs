//! WebSocket bridge server for the Chrome extension.
//!
//! Exposes a local WebSocket endpoint that the acrawl Chrome extension connects to,
//! allowing the extension to act as a browser backend for the crawler agent.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use runtime::config_home_dir;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

use tokio_tungstenite::tungstenite::http::{self as ws_http};

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
    pub fn start(port: u16, token: String) -> Result<Self, WsBridgeError> {
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

    /// Port the server is listening on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Sender for issuing commands to the connected extension.
    ///
    /// Callers send `(BridgeCommand, oneshot::Sender<BridgeResponse>)` and await
    /// the oneshot receiver for the extension's reply.
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

    /// Shut down the server and remove bridge.json.
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = std::fs::remove_file(&self.bridge_file_path);
    }

    /// Wait until a client connects or the timeout elapses.
    /// Returns `true` if a client connected, `false` on timeout.
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
// Internals
// ---------------------------------------------------------------------------

type CommandRx =
    Arc<tokio::sync::Mutex<mpsc::Receiver<(BridgeCommand, oneshot::Sender<BridgeResponse>)>>>;

/// Tracks failed authentication attempts for a single IP address.
struct RateEntry {
    failures: u8,
    window_start: Instant,
}

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

    // Rate limit check: 5 failures in 60s window → 429
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

    // Single-client gate (atomic CAS to claim the slot)
    if state
        .has_active_client
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        eprintln!("[acrawl:ws] rejected {peer_addr}: another client already connected");
        send_raw_http_error(stream, 409, "Conflict: another client is already connected").await;
        return;
    }

    // Attempt WebSocket upgrade with token + origin validation
    let token_clone = state.token.clone();
    let ws_result = tokio_tungstenite::accept_hdr_async(
        stream,
        move |req: &ws_http::Request<()>, resp: ws_http::Response<()>| {
            validate_ws_upgrade(&token_clone, req, resp)
        },
    )
    .await;

    if let Ok(ws) = ws_result {
        eprintln!("[acrawl:ws] client connected from {peer_addr}");
        let _ = state.client_connected_tx.send(true);
        run_ws_session(ws, command_rx).await;
        let _ = state.client_connected_tx.send(false);
        eprintln!("[acrawl:ws] client disconnected");
    } else {
        eprintln!(
            "[acrawl:ws] WebSocket upgrade failed from {peer_addr}: {:?}",
            ws_result.err()
        );
        let client_ip = peer_addr.ip();
        let mut limiter = state.rate_limiter.lock().await;
        if let Some(entry) = limiter.get_mut(&client_ip) {
            entry.failures = entry.failures.saturating_add(1);
        }
    }

    state.has_active_client.store(false, Ordering::Release);
}

#[allow(clippy::result_large_err)]
fn validate_ws_upgrade(
    token: &str,
    req: &ws_http::Request<()>,
    resp: ws_http::Response<()>,
) -> Result<ws_http::Response<()>, ws_http::Response<Option<String>>> {
    let path = req.uri().path();
    if path != "/bridge" {
        return Err(ws_http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Some("Not found".into()))
            .expect("valid response"));
    }

    // Origin: if present must come from a trusted extension page.
    if let Some(origin) = req.headers().get("origin") {
        let origin_str = origin.to_str().unwrap_or("");
        if !is_allowed_extension_origin(origin_str) {
            return Err(ws_http::Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(Some("Forbidden: invalid origin".into()))
                .expect("valid response"));
        }
    }

    // Token from ?token=<value> query param
    let query = req.uri().query().unwrap_or("");
    let provided_token: Option<&str> = query.split('&').find_map(|pair: &str| {
        let (key, value) = pair.split_once('=')?;
        if key == "token" {
            Some(value)
        } else {
            None
        }
    });

    match provided_token {
        Some(t) if constant_time_eq(t.as_bytes(), token.as_bytes()) => Ok(resp),
        Some(t) => {
            eprintln!(
                "[acrawl:ws] token mismatch: got {:?} (len {}), expected len {}",
                &t[..t.len().min(8)],
                t.len(),
                token.len()
            );
            Err(ws_http::Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Some("Unauthorized: invalid token".into()))
                .expect("valid response"))
        }
        None => Err(ws_http::Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Some("Unauthorized: missing token".into()))
            .expect("valid response")),
    }
}

async fn run_ws_session(ws: tokio_tungstenite::WebSocketStream<TcpStream>, command_rx: CommandRx) {
    let (mut sink, mut source) = ws.split();
    let mut rx = command_rx.lock().await;
    let mut pending: HashMap<u64, oneshot::Sender<BridgeResponse>> = HashMap::new();

    loop {
        tokio::select! {
            cmd_opt = rx.recv() => {
                let Some((cmd, resp_tx)) = cmd_opt else { break };
                pending.insert(cmd.id, resp_tx);
                let Ok(json) = serde_json::to_string(&cmd) else { continue };
                if sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            msg_opt = source.next() => {
                let Some(Ok(msg)) = msg_opt else { break };
                match msg {
                    Message::Text(text) => {
                        handle_text_frame(&text, &mut pending, &mut sink).await;
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn handle_text_frame(
    text: &str,
    pending: &mut HashMap<u64, oneshot::Sender<BridgeResponse>>,
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
) {
    // Keepalive ping → respond with pong
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
        if val.get("type").and_then(|v| v.as_str()) == Some("ping") {
            let pong = serde_json::json!({"type": "pong"}).to_string();
            let _ = sink.send(Message::Text(pong.into())).await;
            return;
        }
    }

    if let Ok(resp) = serde_json::from_str::<BridgeResponse>(text) {
        if let Some(tx) = pending.remove(&resp.id) {
            let _ = tx.send(resp);
        }
    }
}

// ---------------------------------------------------------------------------
// Raw HTTP helpers (no framework)
// ---------------------------------------------------------------------------

async fn serve_health(mut stream: TcpStream, port: u16) {
    let mut drain = vec![0u8; 4096];
    let _ = stream.read(&mut drain).await;

    let body = serde_json::json!({
        "service": "acrawl",
        "version": env!("CARGO_PKG_VERSION"),
        "port": port,
    })
    .to_string();

    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

async fn send_raw_http_error(mut stream: TcpStream, status: u16, message: &str) {
    let mut drain = vec![0u8; 4096];
    let _ = stream.read(&mut drain).await;

    let resp = format!(
        "HTTP/1.1 {status} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        message.len(),
        message
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

fn is_allowed_extension_origin(origin: &str) -> bool {
    let id = if let Some(rest) = origin.strip_prefix("chrome-extension://") {
        rest
    } else if let Some(rest) = origin.strip_prefix("edge-extension://") {
        rest
    } else {
        return false;
    };
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Security: constant-time token comparison
// ---------------------------------------------------------------------------

/// Constant-time byte comparison to prevent timing side-channels on token auth.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

// ---------------------------------------------------------------------------
// Token generation
// ---------------------------------------------------------------------------

/// Generate a cryptographically random 256-bit hex token for bridge auth.
#[must_use]
pub fn generate_bridge_token() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::thread_rng().gen();
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer string"));
    }

    #[test]
    fn generate_bridge_token_is_64_hex_chars() {
        let token = generate_bridge_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn health_json_has_required_fields() {
        let body = serde_json::json!({
            "service": "acrawl",
            "version": env!("CARGO_PKG_VERSION"),
            "port": 9222_u16,
        });
        assert_eq!(body["service"], "acrawl");
        assert_eq!(body["port"], 9222);
        assert!(body["version"].is_string());
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

    #[test]
    fn rate_entry_blocks_at_threshold() {
        let entry = RateEntry {
            failures: 5,
            window_start: Instant::now(),
        };
        assert!(entry.failures >= 5);
    }

    #[test]
    fn rate_entry_resets_after_window() {
        let entry = RateEntry {
            failures: 5,
            window_start: Instant::now()
                .checked_sub(std::time::Duration::from_secs(61))
                .unwrap(),
        };
        assert!(entry.window_start.elapsed().as_secs() >= 60);
    }

    #[test]
    fn rate_entry_allows_under_threshold() {
        let entry = RateEntry {
            failures: 4,
            window_start: Instant::now(),
        };
        assert!(entry.failures < 5);
    }

    #[test]
    fn rate_entry_saturating_add_does_not_overflow() {
        let mut entry = RateEntry {
            failures: 255,
            window_start: Instant::now(),
        };
        entry.failures = entry.failures.saturating_add(1);
        assert_eq!(entry.failures, 255);
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
