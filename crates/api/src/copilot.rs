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

/// Request a device code from GitHub for the Copilot OAuth flow.
pub async fn request_device_code(client: &reqwest::Client) -> Result<DeviceCodeResponse, ApiError> {
    let response = client
        .post(COPILOT_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", COPILOT_CLIENT_ID), ("scope", "read:user")])
        .send()
        .await
        .map_err(ApiError::Http)?;
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
        .send()
        .await
        .map_err(ApiError::Http)?;

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
}
