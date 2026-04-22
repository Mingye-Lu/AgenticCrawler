pub mod anthropic;
pub mod bedrock;
pub mod catalog;
pub mod custom;
pub mod openai;
pub mod preset;
pub mod transform;

use serde::{Deserialize, Serialize};

use crate::credentials::{CredentialStore, StoredProviderConfig};
use crate::error::ApiError;
use crate::provider::preset::ProviderProtocol;
use crate::types::{MessageRequest, MessageResponse, ReasoningEffort, StreamEvent};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub provider_id: String,
    pub max_output_tokens: u32,
    pub context_window: u32,
    pub capabilities: ModelCapabilities,
    pub pricing: Option<ModelPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct ModelCapabilities {
    pub reasoning: bool,
    pub tool_use: bool,
    pub vision: bool,
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_efforts: Vec<ReasoningEffort>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
}

pub enum ProviderStream {
    Anthropic(crate::client::MessageStream),
    Bedrock(crate::bedrock::BedrockMessageStream),
    OpenAi(crate::responses::ResponsesMessageStream),
    Custom(crate::openai::OpenAiMessageStream),
    Gemini(crate::gemini::GeminiMessageStream),
}

impl ProviderStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Anthropic(s) => s.next_event().await,
            Self::Bedrock(s) => s.next_event().await,
            Self::OpenAi(s) => s.next_event().await,
            Self::Custom(s) => s.next_event().await,
            Self::Gemini(s) => s.next_event().await,
        }
    }
}

pub enum ProviderClient {
    Anthropic(crate::AnthropicClient),
    Bedrock(crate::bedrock::BedrockClient),
    OpenAi(crate::OpenAiResponsesClient),
    Custom(crate::ChatCompletionsClient),
    Gemini(crate::gemini::GeminiClient),
}

impl ProviderClient {
    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<ProviderStream, ApiError> {
        match self {
            Self::Anthropic(c) => c
                .stream_message(request)
                .await
                .map(ProviderStream::Anthropic),
            Self::Bedrock(c) => c.stream_message(request).await.map(ProviderStream::Bedrock),
            Self::OpenAi(c) => c.stream_message(request).await.map(ProviderStream::OpenAi),
            Self::Custom(c) => c.stream_message(request).await.map(ProviderStream::Custom),
            Self::Gemini(c) => c.stream_message(request).await.map(ProviderStream::Gemini),
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        match self {
            Self::Anthropic(c) => c.send_message(request).await,
            Self::Bedrock(_) => Err(ApiError::Auth(
                "send_message not supported for Bedrock streaming client".into(),
            )),
            Self::Gemini(_) => Err(ApiError::Auth(
                "send_message not supported for Gemini".into(),
            )),
            _ => Err(ApiError::Auth(
                "send_message only supported for Anthropic".into(),
            )),
        }
    }

    #[must_use]
    pub fn is_anthropic(&self) -> bool {
        matches!(self, Self::Anthropic(_) | Self::Bedrock(_))
    }

    pub fn from_stored_config(
        provider_id: &str,
        config: &StoredProviderConfig,
        model: &str,
    ) -> Result<Self, ApiError> {
        if let Some(preset) = preset::find_preset(provider_id) {
            match preset.protocol {
                ProviderProtocol::Anthropic => return anthropic::build_client(config),
                ProviderProtocol::Bedrock => return bedrock::build_client(config, model),
                ProviderProtocol::OpenAiResponses => return openai::build_client(config, model),
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
                        let deployment = config.deployment_name.as_deref().unwrap_or("gpt-4o");
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

        custom::build_client(config, model)
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

    let transform: Box<dyn transform::ProviderTransform> = match transform_id {
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

pub struct ProviderRegistry {
    catalog: Vec<ModelInfo>,
    active_provider_id: Option<String>,
    configured_providers: Vec<String>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn from_credentials(store: &CredentialStore) -> Self {
        let configured_providers: Vec<String> = store.providers.keys().cloned().collect();
        let active_provider_id = store.active_provider.clone();
        let catalog = catalog::builtin_models();
        Self {
            catalog,
            active_provider_id,
            configured_providers,
        }
    }

    #[must_use]
    pub fn resolve_model(&self, model: &str) -> Option<&ModelInfo> {
        self.catalog.iter().find(|m| m.id == model)
    }

    #[must_use]
    pub fn max_tokens(&self, model: &str) -> u32 {
        self.resolve_model(model).map_or_else(
            || catalog::default_max_tokens(model),
            |m| m.max_output_tokens,
        )
    }

    #[must_use]
    pub fn provider_for_model<'a>(&'a self, model: &'a str) -> &'a str {
        self.resolve_model(model).map_or_else(
            || catalog::infer_provider(model),
            |m| m.provider_id.as_str(),
        )
    }

    #[must_use]
    pub fn active_provider_id(&self) -> Option<&str> {
        self.active_provider_id.as_deref()
    }

    pub fn build_client(
        &self,
        model: &str,
        store: &CredentialStore,
    ) -> Result<ProviderClient, ApiError> {
        if store.providers.is_empty() {
            return Ok(ProviderClient::no_auth_placeholder());
        }

        if self
            .active_provider_id()
            .is_none_or(|id| !store.providers.contains_key(id))
        {
            let matching = self.ambiguous_provider_matches(model);
            if matching.len() > 1 {
                return Err(ApiError::Auth(format!(
                    "Model '{model}' matches multiple configured providers ({}). Set active provider with `acrawl auth <provider>`.",
                    matching.join(", ")
                )));
            }
        }

        let provider_id = self
            .active_provider_id()
            .filter(|id| store.providers.contains_key(*id))
            .unwrap_or_else(|| self.provider_for_model(model));

        let config = store.providers.get(provider_id).ok_or_else(|| {
            ApiError::Auth(format!(
                "No {provider_id} credentials found. Run `acrawl auth`."
            ))
        })?;

        ProviderClient::from_stored_config(provider_id, config, model)
    }

    #[must_use]
    pub fn configured_providers(&self) -> &[String] {
        &self.configured_providers
    }

    fn ambiguous_provider_matches(&self, model: &str) -> Vec<&'static str> {
        Self::matching_provider_ids(model, &self.configured_providers)
    }

    fn matching_provider_ids(model: &str, configured_providers: &[String]) -> Vec<&'static str> {
        let mut matching = Vec::new();
        for preset in preset::builtin_presets() {
            if preset.model_prefixes.is_empty()
                || !configured_providers.iter().any(|id| id == preset.id)
                || !preset
                    .model_prefixes
                    .iter()
                    .any(|prefix| model.starts_with(prefix))
            {
                continue;
            }
            matching.push(preset.id);
        }
        matching
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{CredentialStore, StoredProviderConfig};

    #[test]
    fn registry_resolves_model_by_id() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert!(registry.resolve_model("claude-sonnet-4-6").is_some());
        assert!(registry.resolve_model("gpt-4o").is_some());
        assert!(registry.resolve_model("unknown-model").is_none());
    }

    #[test]
    fn registry_max_tokens_from_catalog() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert_eq!(registry.max_tokens("claude-sonnet-4-6"), 64_000);
        assert_eq!(registry.max_tokens("claude-opus-4-6"), 32_000);
        assert_eq!(registry.max_tokens("gpt-4o"), 16_384);
    }

    #[test]
    fn registry_max_tokens_falls_back_for_unknown() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert_eq!(registry.max_tokens("llama3.2"), 8_192);
    }

    #[test]
    fn registry_provider_for_model_uses_catalog() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert_eq!(
            registry.provider_for_model("claude-sonnet-4-6"),
            "anthropic"
        );
        assert_eq!(registry.provider_for_model("gpt-4o"), "openai");
        assert_eq!(registry.provider_for_model("codex-mini-latest"), "openai");
        assert_eq!(registry.provider_for_model("llama3.2"), "other");
    }

    #[test]
    fn registry_build_client_returns_placeholder_when_no_creds() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        let client = registry.build_client("claude-sonnet-4-6", &store);
        assert!(client.is_ok());
        assert!(client.unwrap().is_anthropic());
    }

    #[test]
    fn registry_build_client_uses_active_provider() {
        let mut store = CredentialStore {
            active_provider: Some("openai".into()),
            ..Default::default()
        };
        store.providers.insert(
            "openai".into(),
            crate::credentials::StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("sk-test".into()),
                ..Default::default()
            },
        );
        let registry = ProviderRegistry::from_credentials(&store);
        let client = registry.build_client("claude-sonnet-4-6", &store);
        assert!(client.is_ok());
    }

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
    fn test_active_provider_overrides_model_inference() {
        let mut store = CredentialStore {
            active_provider: Some("other".into()),
            ..Default::default()
        };
        store.providers.insert(
            "other".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("test-key".into()),
                base_url: Some("https://api.example.com/v1".into()),
                ..Default::default()
            },
        );

        let registry = ProviderRegistry::from_credentials(&store);
        let client = registry.build_client("claude-sonnet-4-6", &store);

        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Custom(_)));
    }

    #[test]
    fn test_ambiguous_model_without_active_provider_returns_error() {
        let mut store = CredentialStore::default();
        store.providers.insert(
            "groq".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("test-key".into()),
                ..Default::default()
            },
        );
        store.providers.insert(
            "perplexity".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("test-key".into()),
                ..Default::default()
            },
        );

        let registry = ProviderRegistry::from_credentials(&store);
        let result = registry.build_client("llama-3.1-sonar-preview", &store);

        match result {
            Err(ApiError::Auth(message)) => {
                assert!(message.contains("matches multiple configured providers"));
                assert!(message.contains("groq"));
                assert!(message.contains("perplexity"));
                assert!(message.contains("acrawl auth <provider>"));
            }
            _ => panic!("expected ambiguous provider auth error"),
        }
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
            "bedrock",
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
    fn registry_configured_providers_reflects_store() {
        let mut store = CredentialStore::default();
        store.providers.insert(
            "anthropic".into(),
            crate::credentials::StoredProviderConfig::default(),
        );
        store.providers.insert(
            "openai".into(),
            crate::credentials::StoredProviderConfig::default(),
        );
        let registry = ProviderRegistry::from_credentials(&store);
        let configured = registry.configured_providers();
        assert_eq!(configured.len(), 2);
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
    fn test_model_inference_covers_prefixed_providers() {
        use crate::provider::catalog::infer_provider;

        let cases = [
            ("grok-2", "xai"),
            ("command-r-plus", "cohere"),
            ("qwen-max", "alibaba"),
            ("mistral-large-latest", "mistral"),
            ("codestral-latest", "mistral"),
            ("gemma2-9b-it", "groq"),
            ("@cf/meta/llama-3.1-70b-instruct", "cloudflare"),
            ("dolphin-2.9.2-qwen2-72b", "venice"),
            ("llama3.1-70b", "cerebras"),
        ];
        for (model, expected_provider) in cases {
            let inferred = infer_provider(model);
            assert_eq!(
                inferred, expected_provider,
                "model '{model}' should infer to '{expected_provider}', got '{inferred}'"
            );
        }
    }
}
