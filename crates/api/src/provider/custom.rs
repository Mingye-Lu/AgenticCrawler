use crate::client::AuthSource;
use crate::credentials::StoredProviderConfig;
use crate::error::ApiError;

use super::ProviderClient;

const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";

pub fn build_client(
    config: &StoredProviderConfig,
    model: &str,
) -> Result<ProviderClient, ApiError> {
    let auth = credential_to_auth(config);
    let base_url = config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
    Ok(ProviderClient::Custom(
        crate::ChatCompletionsClient::with_no_auth(model, base_url).with_optional_auth(auth),
    ))
}

fn credential_to_auth(config: &StoredProviderConfig) -> AuthSource {
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
    use crate::credentials::StoredProviderConfig;

    #[test]
    fn builds_with_custom_base_url() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("key".into()),
            base_url: Some("http://localhost:8080/v1".into()),
            ..Default::default()
        };
        let client = build_client(&config, "llama3.2");
        assert!(client.is_ok());
    }

    #[test]
    fn no_auth_builds_successfully() {
        let config = StoredProviderConfig::default();
        let client = build_client(&config, "llama3.2");
        assert!(client.is_ok());
    }
}
