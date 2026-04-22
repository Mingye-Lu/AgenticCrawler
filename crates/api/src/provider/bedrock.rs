use crate::credentials::StoredProviderConfig;
use crate::error::ApiError;

use super::ProviderClient;

const DEFAULT_REGION: &str = "us-east-1";

pub fn build_client(
    config: &StoredProviderConfig,
    _model: &str,
) -> Result<ProviderClient, ApiError> {
    let region = resolve_region(config);

    let access_key_id = config
        .api_key
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("AWS_ACCESS_KEY_ID").ok().filter(|v| !v.is_empty()));

    let secret_access_key = config
        .aws_secret_access_key
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("AWS_SECRET_ACCESS_KEY")
                .ok()
                .filter(|v| !v.is_empty())
        });

    let bearer_token = std::env::var("AWS_BEARER_TOKEN_BEDROCK")
        .ok()
        .filter(|v| !v.is_empty());

    match (access_key_id, secret_access_key) {
        (Some(key_id), Some(secret)) => {
            let session_token = config
                .base_url
                .clone()
                .filter(|v| !v.is_empty())
                .or_else(|| {
                    std::env::var("AWS_SESSION_TOKEN")
                        .ok()
                        .filter(|v| !v.is_empty())
                });
            let client = crate::bedrock::BedrockClient::new(key_id, secret, region);
            Ok(match session_token {
                Some(token) => ProviderClient::Bedrock(client.with_session_token(token)),
                None => ProviderClient::Bedrock(client),
            })
        }
        (Some(token), None) => {
            Ok(ProviderClient::Bedrock(
                crate::bedrock::BedrockClient::from_bearer_token(token, region),
            ))
        }
        (None, _) if bearer_token.is_some() => {
            #[allow(clippy::unwrap_used)]
            Ok(ProviderClient::Bedrock(
                crate::bedrock::BedrockClient::from_bearer_token(
                    bearer_token.unwrap(),
                    region,
                ),
            ))
        }
        _ => Err(ApiError::Auth(
            "Bedrock requires AWS credentials. Provide an API key (bearer token), \
             set AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY, or set AWS_BEARER_TOKEN_BEDROCK."
                .into(),
        )),
    }
}

fn resolve_region(config: &StoredProviderConfig) -> String {
    config
        .region
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("AWS_REGION").ok().filter(|v| !v.is_empty()))
        .or_else(|| {
            std::env::var("AWS_DEFAULT_REGION")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_sigv4_client_with_both_keys() {
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

    #[test]
    fn builds_bearer_client_with_api_key_only() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("bearer-token-value".into()),
            region: Some("us-east-1".into()),
            ..Default::default()
        };

        let client = build_client(&config, "model");
        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Bedrock(_)));
    }
}
