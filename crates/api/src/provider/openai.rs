use crate::client::AuthSource;
use crate::credentials::StoredProviderConfig;
use crate::error::ApiError;

use super::ProviderClient;

pub fn build_client(
    config: &StoredProviderConfig,
    model: &str,
) -> Result<ProviderClient, ApiError> {
    let auth = credential_to_auth(config);
    let mut client = crate::OpenAiResponsesClient::new(auth, model);
    if config.auth_method == "oauth" {
        let account_id = config.oauth.as_ref().and_then(|o| o.account_id.clone());
        client = client.with_codex_endpoint(account_id);
    }
    Ok(ProviderClient::OpenAi(client))
}

fn credential_to_auth(config: &StoredProviderConfig) -> AuthSource {
    if config.auth_method == "oauth" {
        if let Some(oauth) = &config.oauth {
            return AuthSource::BearerToken(oauth.access_token.clone());
        }
    }
    if let Some(key) = &config.api_key {
        if !key.is_empty() {
            return AuthSource::BearerToken(key.clone());
        }
    }
    AuthSource::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{StoredOAuthTokens, StoredProviderConfig};

    #[test]
    fn api_key_builds_openai_client() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("sk-test".into()),
            ..Default::default()
        };
        let client = build_client(&config, "gpt-4o");
        assert!(client.is_ok());
    }

    #[test]
    fn oauth_config_builds_codex_client() {
        let config = StoredProviderConfig {
            auth_method: "oauth".into(),
            oauth: Some(StoredOAuthTokens {
                access_token: "access_tok".into(),
                account_id: Some("acct_123".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let client = build_client(&config, "codex-mini-latest");
        assert!(client.is_ok());
    }
}
