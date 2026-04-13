pub mod anthropic;
pub mod catalog;
pub mod custom;
pub mod openai;

use serde::{Deserialize, Serialize};

use crate::credentials::{CredentialStore, StoredProviderConfig};
use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

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
    OpenAi(crate::responses::ResponsesMessageStream),
    Custom(crate::openai::OpenAiMessageStream),
}

impl ProviderStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Anthropic(s) => s.next_event().await,
            Self::OpenAi(s) => s.next_event().await,
            Self::Custom(s) => s.next_event().await,
        }
    }
}

pub enum ProviderClient {
    Anthropic(crate::AnthropicClient),
    OpenAi(crate::OpenAiResponsesClient),
    Custom(crate::ChatCompletionsClient),
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
            Self::OpenAi(c) => c.stream_message(request).await.map(ProviderStream::OpenAi),
            Self::Custom(c) => c.stream_message(request).await.map(ProviderStream::Custom),
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        match self {
            Self::Anthropic(c) => c.send_message(request).await,
            _ => Err(ApiError::Auth(
                "send_message only supported for Anthropic".into(),
            )),
        }
    }

    #[must_use]
    pub fn is_anthropic(&self) -> bool {
        matches!(self, Self::Anthropic(_))
    }

    pub fn from_stored_config(
        provider_id: &str,
        config: &StoredProviderConfig,
        model: &str,
    ) -> Result<Self, ApiError> {
        match provider_id {
            "anthropic" => anthropic::build_client(config),
            "openai" => openai::build_client(config, model),
            _ => custom::build_client(config, model),
        }
    }

    #[must_use]
    pub fn no_auth_placeholder() -> Self {
        Self::Anthropic(crate::AnthropicClient::from_auth(crate::AuthSource::None))
    }
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
    pub fn resolve_model(&self, model_or_alias: &str) -> Option<&ModelInfo> {
        self.catalog
            .iter()
            .find(|m| m.id == model_or_alias || m.aliases.iter().any(|a| a == model_or_alias))
    }

    #[must_use]
    pub fn resolve_alias<'a>(&'a self, model: &'a str) -> &'a str {
        self.resolve_model(model).map_or(model, |m| m.id.as_str())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::CredentialStore;

    #[test]
    fn registry_resolves_alias_to_canonical_id() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert_eq!(registry.resolve_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(registry.resolve_alias("opus"), "claude-opus-4-6");
        assert_eq!(registry.resolve_alias("4o"), "gpt-4o");
        assert_eq!(registry.resolve_alias("codex"), "codex-mini-latest");
    }

    #[test]
    fn registry_resolves_unknown_alias_to_self() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert_eq!(registry.resolve_alias("unknown-model"), "unknown-model");
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
}
