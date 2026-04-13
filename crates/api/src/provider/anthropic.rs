use crate::client::AuthSource;
use crate::credentials::StoredProviderConfig;
use crate::error::ApiError;

use super::ProviderClient;

pub fn build_client(config: &StoredProviderConfig) -> Result<ProviderClient, ApiError> {
    let auth = credential_to_auth(config);
    Ok(ProviderClient::Anthropic(
        crate::AnthropicClient::from_auth(auth),
    ))
}

fn credential_to_auth(config: &StoredProviderConfig) -> AuthSource {
    if config.auth_method == "oauth" {
        if let Some(oauth) = &config.oauth {
            return AuthSource::BearerToken(oauth.access_token.clone());
        }
    }
    if let Some(key) = &config.api_key {
        if !key.is_empty() {
            return AuthSource::ApiKey(key.clone());
        }
    }
    AuthSource::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::StoredProviderConfig;

    #[test]
    fn api_key_produces_api_key_auth() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("sk-ant-test".into()),
            ..Default::default()
        };
        let client = build_client(&config);
        assert!(client.is_ok());
    }

    #[test]
    fn empty_config_produces_no_auth() {
        let config = StoredProviderConfig::default();
        let client = build_client(&config);
        assert!(client.is_ok());
    }
}
