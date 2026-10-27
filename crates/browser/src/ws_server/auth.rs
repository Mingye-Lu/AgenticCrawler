use std::time::Instant;

use tokio_tungstenite::tungstenite::http::{self as ws_http, StatusCode};

/// Tracks rejected WebSocket upgrades for a single IP address.
pub(super) struct RateEntry {
    pub failures: u8,
    pub window_start: Instant,
}

/// Accepts the upgrade only on `/bridge` and only from a browser extension.
///
/// There is no token: the extension is installed in the user's own browser,
/// and installing it is the consent. The `Origin` check is what keeps ordinary
/// web pages, which can also open `ws://127.0.0.1`, from connecting; browsers
/// always send it and pages cannot forge an extension origin.
#[allow(clippy::result_large_err)]
pub(super) fn validate_ws_upgrade(
    req: &ws_http::Request<()>,
    resp: ws_http::Response<()>,
) -> Result<ws_http::Response<()>, ws_http::Response<Option<String>>> {
    if req.uri().path() != "/bridge" {
        return Err(ws_http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Some("Not found".into()))
            .expect("valid response"));
    }

    let origin_ok = req
        .headers()
        .get("origin")
        .and_then(|o| o.to_str().ok())
        .is_some_and(is_allowed_extension_origin);
    if !origin_ok {
        return Err(ws_http::Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Some("Forbidden: invalid origin".into()))
            .expect("valid response"));
    }

    Ok(resp)
}

pub(super) fn is_allowed_extension_origin(origin: &str) -> bool {
    let id = if let Some(rest) = origin.strip_prefix("chrome-extension://") {
        rest
    } else if let Some(rest) = origin.strip_prefix("edge-extension://") {
        rest
    } else {
        return false;
    };
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_lowercase())
}
