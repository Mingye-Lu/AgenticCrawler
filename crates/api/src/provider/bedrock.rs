use crate::credentials::StoredProviderConfig;
use crate::error::ApiError;

use super::ProviderClient;

const DEFAULT_REGION: &str = "us-east-1";

pub fn build_client(
    config: &StoredProviderConfig,
    _model: &str,
) -> Result<ProviderClient, ApiError> {
    let access_key_id = config.api_key.clone().filter(|value| !value.is_empty());
    let secret_access_key = config
        .aws_secret_access_key
        .clone()
        .filter(|value| !value.is_empty());

    let (Some(access_key_id), Some(secret_access_key)) = (access_key_id, secret_access_key) else {
        return Err(ApiError::Auth(
            "Bedrock requires api_key (access key id) and aws_secret_access_key".into(),
        ));
    };

    let region = config
        .region
        .clone()
        .unwrap_or_else(|| DEFAULT_REGION.to_string());

    let client = crate::bedrock::BedrockClient::new(access_key_id, secret_access_key, region);
    Ok(
        match config.base_url.clone().filter(|value| !value.is_empty()) {
            Some(session_token) => {
                ProviderClient::Bedrock(client.with_session_token(session_token))
            }
            None => ProviderClient::Bedrock(client),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_bedrock_client_with_region() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("access-key".into()),
            aws_secret_access_key: Some("secret-key".into()),
            region: Some("eu-west-1".into()),
            ..Default::default()
        };

        let client = build_client(&config, "model");
        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Bedrock(_)));
    }
}
