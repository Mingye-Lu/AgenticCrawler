use crate::credentials::StoredProviderConfig;
use crate::error::ApiError;
use crate::provider::preset::ProviderProtocol;

use super::ProviderClient;

impl ProviderClient {
    pub fn from_stored_config(
        provider_id: &str,
        config: &StoredProviderConfig,
        model: &str,
    ) -> Result<Self, ApiError> {
        if let Some(preset) = super::preset::find_preset(provider_id) {
            match preset.protocol {
                ProviderProtocol::Anthropic => return super::anthropic::build_client(config),
                ProviderProtocol::Bedrock => {
                    return super::bedrock::build_client(config, model);
                }
                ProviderProtocol::OpenAiResponses => {
                    return super::openai::build_client(config, model);
                }
                ProviderProtocol::Gemini => {
                    if preset.id == "vertex" {
                        return Ok(build_vertex_client(config, model));
                    }
                    let api_key = config.api_key.clone().unwrap_or_default();
                    let client = config.base_url.as_ref().map_or_else(
                        || crate::gemini::GeminiClient::new(api_key.clone()),
                        |base_url| {
                            crate::gemini::GeminiClient::new(api_key.clone())
                                .with_base_url(base_url.clone())
                        },
                    );
                    return Ok(Self::Gemini(client));
                }
                ProviderProtocol::ChatCompletions => {
                    if preset.id == "azure" {
                        let resource = config.resource_name.as_deref().unwrap_or("default");
                        let deployment =
                            config.deployment_name.as_deref().unwrap_or("gpt-4o");
                        let base_url = format!(
                            "https://{resource}.openai.azure.com/openai/deployments/{deployment}"
                        );
                        return Ok(Self::Custom(build_azure_chat_completions(
                            config,
                            model,
                            &base_url,
                            preset.chat_path,
                        )));
                    }
                    if preset.id == "gitlab" {
                        let base_url = config
                            .base_url
                            .as_deref()
                            .unwrap_or("https://gitlab.com/api/v4/ai/v1");
                        let gitlab_headers = vec![
                            (
                                "X-Gitlab-Authentication-Type".to_string(),
                                "oidc".to_string(),
                            ),
                            (
                                "X-Gitlab-Duo-Chat-Feature".to_string(),
                                "code_suggestions".to_string(),
                            ),
                        ];
                        return Ok(Self::Custom(
                            build_chat_completions_from_config(
                                config,
                                model,
                                base_url,
                                preset.chat_path,
                                preset.transform_id,
                            )
                            .with_extra_headers(gitlab_headers),
                        ));
                    }
                    if preset.id == "copilot" {
                        let base_url = config
                            .base_url
                            .as_deref()
                            .unwrap_or("https://api.githubcopilot.com");
                        let copilot_headers = vec![
                            ("Copilot-Integration-Id".to_string(), "acrawl".to_string()),
                            ("editor-version".to_string(), "acrawl/1.0.0".to_string()),
                        ];
                        return Ok(Self::Custom(
                            build_copilot_chat_completions(
                                config,
                                model,
                                base_url,
                                preset.chat_path,
                            )
                            .with_extra_headers(copilot_headers),
                        ));
                    }
                    let base_url = config.base_url.as_deref().unwrap_or(preset.base_url);
                    return Ok(Self::Custom(build_chat_completions_from_config(
                        config,
                        model,
                        base_url,
                        preset.chat_path,
                        preset.transform_id,
                    )));
                }
            }
        }

        super::custom::build_client(config, model)
    }

    #[must_use]
    pub fn no_auth_placeholder() -> Self {
        Self::Anthropic(crate::AnthropicClient::from_auth(crate::AuthSource::None))
    }
}

fn build_chat_completions_from_config(
    config: &StoredProviderConfig,
    model: &str,
    base_url: &str,
    chat_path: &str,
    transform_id: Option<&str>,
) -> crate::ChatCompletionsClient {
    use crate::client::AuthSource;
    use crate::provider::transform::{MistralTransform, NoOpTransform};

    let auth = config
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .map_or(AuthSource::None, |key| {
            AuthSource::BearerToken(key.to_string())
        });

    let transform: Box<dyn super::transform::ProviderTransform> = match transform_id {
        Some("mistral") => Box::new(MistralTransform),
        _ => Box::new(NoOpTransform),
    };

    crate::ChatCompletionsClient::with_no_auth(model, base_url)
        .with_optional_auth(auth)
        .with_chat_path(chat_path)
        .with_transform(transform)
}

fn build_azure_chat_completions(
    config: &StoredProviderConfig,
    model: &str,
    base_url: &str,
    chat_path: &str,
) -> crate::ChatCompletionsClient {
    let extra_headers: Vec<(String, String)> = config
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .map(|key| vec![("api-key".to_string(), key.to_string())])
        .unwrap_or_default();

    crate::ChatCompletionsClient::with_no_auth(model, base_url)
        .with_chat_path(chat_path)
        .with_extra_headers(extra_headers)
}

fn build_copilot_chat_completions(
    config: &StoredProviderConfig,
    model: &str,
    base_url: &str,
    chat_path: &str,
) -> crate::ChatCompletionsClient {
    use crate::client::AuthSource;

    let auth = config
        .oauth
        .as_ref()
        .filter(|o| !o.access_token.is_empty())
        .map_or(AuthSource::None, |o| {
            AuthSource::BearerToken(o.access_token.clone())
        });

    crate::ChatCompletionsClient::with_no_auth(model, base_url)
        .with_optional_auth(auth)
        .with_chat_path(chat_path)
}

fn build_vertex_client(config: &StoredProviderConfig, model: &str) -> ProviderClient {
    let project = config.gcp_project_id.as_deref().unwrap_or("my-project");
    let region = config.gcp_region.as_deref().unwrap_or("us-central1");
    let api_key = config.api_key.clone().unwrap_or_default();

    if model.starts_with("claude") {
        let vertex_base = format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic"
        );
        return ProviderClient::Anthropic(
            crate::AnthropicClient::from_auth(crate::client::AuthSource::BearerToken(api_key))
                .with_base_url(vertex_base),
        );
    }

    let vertex_base = format!(
        "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google"
    );
    ProviderClient::Gemini(
        crate::gemini::GeminiClient::new(api_key)
            .with_base_url(vertex_base)
            .with_bearer_auth(),
    )
}

#[cfg(test)]
mod tests {
    use crate::credentials::StoredProviderConfig;
    use crate::provider::ProviderClient;

    #[test]
    fn test_auth_other_still_works() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("test-key".into()),
            base_url: Some("https://api.example.com/v1".into()),
            ..Default::default()
        };

        let client = ProviderClient::from_stored_config("other", &config, "llama3");

        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Custom(_)));
    }

    #[test]
    fn test_unknown_provider_falls_back_to_custom() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("test-key".into()),
            base_url: Some("https://some-unknown-provider.com/v1".into()),
            ..Default::default()
        };

        let client = ProviderClient::from_stored_config("unknown-provider", &config, "some-model");

        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Custom(_)));
    }

    #[test]
    fn test_bedrock_preset_routes_to_bedrock_client() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("AKIDEXAMPLE".into()),
            aws_secret_access_key: Some("secret".into()),
            region: Some("us-east-1".into()),
            ..Default::default()
        };

        let client = ProviderClient::from_stored_config(
            "amazon-bedrock",
            &config,
            "anthropic.claude-sonnet-4-6-20250514-v1:0",
        );

        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Bedrock(_)));
    }

    #[test]
    fn test_gitlab_custom_headers() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("glpat-test".into()),
            ..Default::default()
        };
        let client =
            ProviderClient::from_stored_config("gitlab", &config, "gitlab-default").unwrap();
        let ProviderClient::Custom(c) = client else {
            panic!("expected Custom variant");
        };
        let has_auth_type = c
            .extra_headers
            .iter()
            .any(|(k, _)| k == "X-Gitlab-Authentication-Type");
        assert!(
            has_auth_type,
            "GitLab client should have X-Gitlab-Authentication-Type header"
        );
        let has_feature = c
            .extra_headers
            .iter()
            .any(|(k, _)| k == "X-Gitlab-Duo-Chat-Feature");
        assert!(
            has_feature,
            "GitLab client should have X-Gitlab-Duo-Chat-Feature header"
        );
    }

    #[test]
    fn test_gitlab_self_hosted_url() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("token".into()),
            base_url: Some("https://my-gitlab.example.com/api/v4/ai/v1".into()),
            ..Default::default()
        };
        let client =
            ProviderClient::from_stored_config("gitlab", &config, "gitlab-default").unwrap();
        let ProviderClient::Custom(c) = client else {
            panic!("expected Custom variant for gitlab");
        };
        assert!(c.base_url.contains("my-gitlab.example.com"));
    }

    #[test]
    fn test_azure_url_template() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("azure-key-123".into()),
            resource_name: Some("myresource".into()),
            deployment_name: Some("gpt4".into()),
            ..Default::default()
        };

        let client = ProviderClient::from_stored_config("azure", &config, "gpt-4o");
        assert!(client.is_ok());

        let ProviderClient::Custom(cc) = client.unwrap() else {
            panic!("expected Custom variant");
        };
        assert!(cc.base_url.contains("myresource.openai.azure.com"));
        assert!(cc.base_url.contains("/deployments/gpt4"));
        assert!(cc.chat_path.contains("api-version=2024-02-01"));
    }

    #[test]
    fn test_azure_api_key_header() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("azure-secret".into()),
            resource_name: Some("res".into()),
            deployment_name: Some("dep".into()),
            ..Default::default()
        };

        let client = ProviderClient::from_stored_config("azure", &config, "gpt-4o");
        let ProviderClient::Custom(cc) = client.unwrap() else {
            panic!("expected Custom variant");
        };
        assert_eq!(
            cc.extra_headers,
            vec![("api-key".to_string(), "azure-secret".to_string())]
        );
    }

    #[test]
    fn test_copilot_routes_to_chat_completions_with_oauth() {
        let config = StoredProviderConfig {
            auth_method: "oauth".into(),
            oauth: Some(crate::credentials::StoredOAuthTokens {
                access_token: "copilot-token-xyz".into(),
                ..Default::default()
            }),
            base_url: Some("https://api.githubcopilot.com".into()),
            ..Default::default()
        };

        let client = ProviderClient::from_stored_config("copilot", &config, "gpt-4o");
        assert!(client.is_ok());

        let ProviderClient::Custom(cc) = client.unwrap() else {
            panic!("expected Custom variant");
        };
        assert_eq!(cc.base_url, "https://api.githubcopilot.com");
        assert!(cc
            .extra_headers
            .contains(&("Copilot-Integration-Id".to_string(), "acrawl".to_string())));
        assert!(cc
            .extra_headers
            .contains(&("editor-version".to_string(), "acrawl/1.0.0".to_string())));
    }

    #[test]
    fn test_vertex_gemini_url() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("token".into()),
            gcp_project_id: Some("my-project".into()),
            gcp_region: Some("us-central1".into()),
            ..Default::default()
        };
        let client =
            ProviderClient::from_stored_config("vertex", &config, "gemini-2.0-flash").unwrap();
        assert!(matches!(client, ProviderClient::Gemini(_)));
    }

    #[test]
    fn test_vertex_anthropic_url() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("token".into()),
            gcp_project_id: Some("my-project".into()),
            gcp_region: Some("us-central1".into()),
            ..Default::default()
        };
        let client =
            ProviderClient::from_stored_config("vertex", &config, "claude-sonnet-4-6@20250514")
                .unwrap();
        assert!(matches!(client, ProviderClient::Anthropic(_)));
    }

    #[test]
    fn test_vertex_routes_by_model() {
        let config = StoredProviderConfig {
            api_key: Some("tok".into()),
            ..Default::default()
        };
        let gemini =
            ProviderClient::from_stored_config("vertex", &config, "gemini-1.5-pro").unwrap();
        let claude =
            ProviderClient::from_stored_config("vertex", &config, "claude-sonnet-4-6").unwrap();
        assert!(matches!(gemini, ProviderClient::Gemini(_)));
        assert!(matches!(claude, ProviderClient::Anthropic(_)));
    }

    #[test]
    fn test_every_chat_completions_preset_builds_client() {
        use crate::provider::preset::{builtin_presets, ProviderProtocol};

        for preset in builtin_presets() {
            if !matches!(preset.protocol, ProviderProtocol::ChatCompletions) {
                continue;
            }
            if preset.base_url.contains('{') {
                continue;
            }
            let config = StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("test-key".into()),
                base_url: Some(preset.base_url.to_string()),
                ..Default::default()
            };
            let result = ProviderClient::from_stored_config(preset.id, &config, "test-model");
            assert!(
                result.is_ok(),
                "ChatCompletions preset '{}' failed to build client: {:?}",
                preset.id,
                result.err()
            );
        }
    }

    #[test]
    fn no_auth_placeholder_is_anthropic() {
        let client = ProviderClient::no_auth_placeholder();
        assert!(client.is_anthropic());
    }

    #[test]
    fn gemini_preset_builds_gemini_client() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("gemini-key".into()),
            ..Default::default()
        };
        let client =
            ProviderClient::from_stored_config("google", &config, "gemini-2.0-flash").unwrap();
        assert!(matches!(client, ProviderClient::Gemini(_)));
    }

    #[test]
    fn anthropic_preset_builds_anthropic_client() {
        let config = StoredProviderConfig {
            auth_method: "api_key".into(),
            api_key: Some("sk-ant-test".into()),
            ..Default::default()
        };
        let client =
            ProviderClient::from_stored_config("anthropic", &config, "claude-sonnet-4-6").unwrap();
        assert!(matches!(client, ProviderClient::Anthropic(_)));
    }
}
