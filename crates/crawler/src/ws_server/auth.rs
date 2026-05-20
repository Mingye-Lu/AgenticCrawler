use std::time::Instant;

use tokio_tungstenite::tungstenite::http::{self as ws_http, StatusCode};

/// Tracks failed authentication attempts for a single IP address.
pub(super) struct RateEntry {
    pub failures: u8,
    pub window_start: Instant,
}

#[allow(clippy::result_large_err)]
pub(super) fn validate_ws_upgrade(
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

    if let Some(origin) = req.headers().get("origin") {
        let origin_str = origin.to_str().unwrap_or("");
        if !is_allowed_extension_origin(origin_str) {
            return Err(ws_http::Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(Some("Forbidden: invalid origin".into()))
                .expect("valid response"));
        }
    }

    let query = req.uri().query().unwrap_or("");
    let provided_token: Option<String> = query.split('&').find_map(|pair: &str| {
        let (key, value) = pair.split_once('=')?;
        if key == "token" {
            Some(percent_decode(value))
        } else {
            None
        }
    });

    match provided_token {
        Some(ref t) if constant_time_eq(t.as_bytes(), token.as_bytes()) => Ok(resp),
        Some(ref t) => {
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

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

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
