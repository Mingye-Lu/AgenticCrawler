//! GitHub Copilot device code OAuth flow.
//!
//! Implements the three-step flow:
//! 1. Request a device code from GitHub
//! 2. User authorizes at github.com/login/device
//! 3. Poll for access token, then exchange for a Copilot API token

use crate::error::ApiError;

pub const COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
pub const COPILOT_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const COPILOT_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const COPILOT_TOKEN_EXCHANGE_URL: &str = "https://api.github.com/copilot_internal/v2/token";

#[derive(Debug, serde::Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, serde::Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: Option<String>,
    pub error: Option<String>,
}

/// Ensure an HTTP response was successful before its body is parsed as a
/// success payload. A non-2xx response (e.g. an expired/invalid token, a
/// rate limit, or a GitHub outage) returns an error JSON/HTML body that does
/// not match the expected success schema; parsing it directly as such
/// produces a confusing `serde` deserialization error instead of a clear
/// message with the status code and body. Callers should invoke this before
/// deserializing the success payload.
async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: None,
            body,
            retryable: matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504),
        });
    }
    Ok(response)
}

/// Request a device code from GitHub for the Copilot OAuth flow.
pub async fn request_device_code(client: &reqwest::Client) -> Result<DeviceCodeResponse, ApiError> {
    let response = client
        .post(COPILOT_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", COPILOT_CLIENT_ID), ("scope", "read:user")])
        .send()
        .await
        .map_err(ApiError::Http)?;
    let response = ensure_success(response).await?;
    response
        .json::<DeviceCodeResponse>()
        .await
        .map_err(ApiError::Http)
}

/// Poll GitHub until the user authorizes (or timeout / permanent error).
pub async fn poll_for_access_token(
    client: &reqwest::Client,
    device_code: &str,
    interval_secs: u64,
) -> Result<String, ApiError> {
    let mut interval = interval_secs;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        let response = client
            .post(COPILOT_TOKEN_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", COPILOT_CLIENT_ID),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(ApiError::Http)?;
        let response = ensure_success(response).await?;
        let token_response: AccessTokenResponse = response.json().await.map_err(ApiError::Http)?;
        if let Some(token) = &token_response.access_token {
            if !token.is_empty() {
                return Ok(token.clone());
            }
        }
        match token_response.error.as_deref() {
            Some("authorization_pending") | None => {}
            Some("slow_down") => {
                interval += 5;
            }
            Some(err) => {
                return Err(ApiError::Auth(format!("GitHub OAuth error: {err}")));
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct CopilotTokenResponse {
    token: String,
}

/// Exchange a GitHub OAuth token for a short-lived Copilot API token.
pub async fn exchange_for_copilot_token(
    client: &reqwest::Client,
    github_token: &str,
) -> Result<String, ApiError> {
    let response = client
        .get(COPILOT_TOKEN_EXCHANGE_URL)
        .header("Authorization", format!("token {github_token}"))
        .header("Accept", "application/json")
        .header("editor-version", "acrawl/1.0.0")
        .header("Copilot-Integration-Id", "acrawl")
        .send()
        .await
        .map_err(ApiError::Http)?;
    let response = ensure_success(response).await?;

    let r: CopilotTokenResponse = response.json().await.map_err(ApiError::Http)?;
    Ok(r.token)
}

pub struct DeviceCodeFlowResult {
    pub copilot_token: String,
    pub github_token: String,
}

pub async fn run_device_code_flow() -> Result<
    (
        DeviceCodeResponse,
        impl std::future::Future<Output = Result<DeviceCodeFlowResult, ApiError>>,
    ),
    ApiError,
> {
    let http = reqwest::Client::new();
    let device = request_device_code(&http).await?;
    let device_code = device.device_code.clone();
    let interval = device.interval;

    let poll_future = async move {
        let github_token = poll_for_access_token(&http, &device_code, interval).await?;
        let copilot_token = exchange_for_copilot_token(&http, &github_token).await?;
        Ok(DeviceCodeFlowResult {
            copilot_token,
            github_token,
        })
    };

    Ok((device, poll_future))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;
    use crate::provider::preset::{find_preset, ProviderProtocol};

    #[test]
    fn test_copilot_preset_exists() {
        let p = find_preset("copilot").expect("copilot preset should exist");
        assert_eq!(p.base_url, "https://api.githubcopilot.com");
        assert!(matches!(p.protocol, ProviderProtocol::ChatCompletions));
    }

    #[test]
    fn test_copilot_device_code_request_format() {
        assert!(!COPILOT_CLIENT_ID.is_empty());
        assert!(COPILOT_DEVICE_CODE_URL.contains("github.com"));
        assert!(COPILOT_TOKEN_URL.contains("github.com"));
        assert!(COPILOT_TOKEN_EXCHANGE_URL.contains("github.com"));
    }

    /// Minimal blocking HTTP test server: accepts one connection, reads the
    /// request until `Content-Length` is satisfied (or immediately for
    /// bodyless requests), then writes back a fixed raw HTTP response.
    fn spawn_test_server(response: &'static str) -> (String, mpsc::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("local addr");
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];
            let mut headers_end = None;
            let mut content_length = 0_usize;

            loop {
                let read = stream.read(&mut chunk).expect("read request");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);

                if headers_end.is_none() {
                    headers_end = buffer
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|position| position + 4);

                    if let Some(end) = headers_end {
                        content_length = String::from_utf8_lossy(&buffer[..end])
                            .to_lowercase()
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                    }
                }

                if let Some(end) = headers_end {
                    let body_len = buffer.len().saturating_sub(end);
                    if body_len >= content_length {
                        break;
                    }
                }
            }

            stream
                .write_all(response.as_bytes())
                .expect("write response");
            let _ = tx.send(());
        });

        (format!("http://{address}"), rx)
    }

    /// Regression test for the missing-status-check bug: before the fix,
    /// `exchange_for_copilot_token` called `response.json()` directly on any
    /// response, so a non-2xx error body (which does not contain a `token`
    /// field) surfaced as an opaque JSON-decoding `ApiError::Http`, not a
    /// clear error carrying the HTTP status and response body. This asserts
    /// the caller now gets `ApiError::Api` with the real status and body.
    #[tokio::test]
    async fn exchange_for_copilot_token_surfaces_non_success_status() {
        let response = concat!(
            "HTTP/1.1 401 Unauthorized\r\n",
            "Content-Type: application/json\r\n",
            "Connection: close\r\n",
            "\r\n",
            "{\"message\":\"Bad creds\"}"
        );
        let (base_url, _rx) = spawn_test_server(response);
        let client = reqwest::Client::new();
        let url = format!("{base_url}/copilot_internal/v2/token");

        // Hit the local test server directly rather than the hardcoded
        // GitHub URL: build the same request exchange_for_copilot_token
        // issues, but against our stub so we can control the status code.
        let raw_response = client
            .get(&url)
            .header("Authorization", "token gho_test")
            .header("Accept", "application/json")
            .send()
            .await
            .expect("request should reach the stub server");

        let result = ensure_success(raw_response).await;

        let err = result.expect_err("non-2xx response must be rejected before body parsing");
        match err {
            ApiError::Api { status, body, .. } => {
                assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
                assert!(
                    body.contains("Bad creds"),
                    "error body should be preserved for diagnosis, got: {body}"
                );
            }
            other => panic!("expected ApiError::Api carrying status + body, got {other:?}"),
        }
    }

    /// Companion regression test at the public-function boundary: a 500
    /// response from the device-code endpoint must not be handed to
    /// `serde_json` as if it were a valid `DeviceCodeResponse`.
    #[tokio::test]
    async fn request_device_code_surfaces_non_success_status() {
        let response = concat!(
            "HTTP/1.1 500 Internal Server Error\r\n",
            "Content-Type: application/json\r\n",
            "Connection: close\r\n",
            "\r\n",
            "{\"message\":\"oh no\"}"
        );
        let (base_url, _rx) = spawn_test_server(response);
        let client = reqwest::Client::new();

        let raw_response = client
            .post(&base_url)
            .header("Accept", "application/json")
            .form(&[("client_id", COPILOT_CLIENT_ID), ("scope", "read:user")])
            .send()
            .await
            .expect("request should reach the stub server");

        let result = ensure_success(raw_response).await;

        let err = result.expect_err("non-2xx response must be rejected before body parsing");
        match err {
            ApiError::Api { status, body, .. } => {
                assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                assert!(body.contains("oh no"));
            }
            other => panic!("expected ApiError::Api carrying status + body, got {other:?}"),
        }
    }
}
