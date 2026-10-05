use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::auth::is_allowed_extension_origin;
use super::pairing::RedeemError;
use super::ServerState;

const MAX_PAIR_REQUEST_BYTES: usize = 4096;

pub(super) async fn serve_health(mut stream: TcpStream, port: u16) {
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

pub(super) async fn send_raw_http_error(mut stream: TcpStream, status: u16, message: &str) {
    let mut drain = vec![0u8; 4096];
    let _ = stream.read(&mut drain).await;

    let resp = format!(
        "HTTP/1.1 {status} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        message.len(),
        message
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// `GET /pair/info` and `POST /pair`: the extension popup's side of pairing.
///
/// Only a browser-extension `Origin` is served. A local process can spoof that
/// header, which is why the token is released only against the code the user
/// read from the acrawl side.
pub(super) async fn serve_pair(mut stream: TcpStream, state: &ServerState) {
    let Some(req) = read_request(&mut stream).await else {
        send_json(
            &mut stream,
            400,
            &serde_json::json!({"error": "bad request"}),
        )
        .await;
        return;
    };

    let origin_ok = match req.origin.as_deref() {
        Some(o) => is_allowed_extension_origin(o),
        None => req.method == "GET",
    };
    if !origin_ok {
        send_json(
            &mut stream,
            403,
            &serde_json::json!({"error": "forbidden origin"}),
        )
        .await;
        return;
    }

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/pair/info") => {
            let body = match state.pairing.info() {
                Some((host, expires_in_secs)) => serde_json::json!({
                    "offer": {"host": host, "expires_in_secs": expires_in_secs}
                }),
                None => serde_json::json!({"offer": null}),
            };
            send_json(&mut stream, 200, &body).await;
        }
        ("POST", "/pair") => {
            let code = serde_json::from_str::<serde_json::Value>(&req.body)
                .ok()
                .and_then(|v| v["code"].as_str().map(str::to_owned))
                .unwrap_or_default();
            match state.pairing.redeem(&code) {
                Ok(()) => {
                    send_json(&mut stream, 200, &serde_json::json!({"token": state.token})).await;
                }
                Err(RedeemError::Wrong) => {
                    send_json(
                        &mut stream,
                        403,
                        &serde_json::json!({"error": "wrong code"}),
                    )
                    .await;
                }
                Err(RedeemError::NoOffer) => {
                    send_json(
                        &mut stream,
                        404,
                        &serde_json::json!({"error": "no pairing offer"}),
                    )
                    .await;
                }
            }
        }
        _ => send_json(&mut stream, 404, &serde_json::json!({"error": "not found"})).await,
    }
}

struct PairRequest {
    method: String,
    path: String,
    origin: Option<String>,
    body: String,
}

async fn read_request(stream: &mut TcpStream) -> Option<PairRequest> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
        if data.len() > MAX_PAIR_REQUEST_BYTES {
            return None;
        }
        let Some(head_end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next()?.split(' ');
        let method = request_line.next()?.to_owned();
        let path = request_line.next()?.to_owned();
        let mut origin = None;
        let mut content_length = 0usize;
        for line in lines {
            let (name, value) = line.split_once(':')?;
            match name.trim().to_ascii_lowercase().as_str() {
                "origin" => origin = Some(value.trim().to_owned()),
                "content-length" => content_length = value.trim().parse().ok()?,
                _ => {}
            }
        }
        let body_start = head_end + 4;
        if data.len() - body_start < content_length {
            continue;
        }
        let body =
            String::from_utf8_lossy(&data[body_start..body_start + content_length]).into_owned();
        return Some(PairRequest {
            method,
            path,
            origin,
            body,
        });
    }
}

async fn send_json(stream: &mut TcpStream, status: u16, body: &serde_json::Value) {
    let body = body.to_string();
    let resp = format!(
        "HTTP/1.1 {status} Status\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}
