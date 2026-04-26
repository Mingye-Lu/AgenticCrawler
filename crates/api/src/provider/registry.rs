use crate::credentials::{CredentialStore, StoredProviderConfig};
use crate::error::ApiError;

use super::{catalog, ModelInfo, ProviderClient};

/// Extract the API model ID from a potentially provider-prefixed model string.
/// "anthropic/claude-sonnet-4-6" → "claude-sonnet-4-6"
/// "claude-sonnet-4-6" → "claude-sonnet-4-6"
#[must_use]
pub fn model_api_id(model: &str) -> &str {
    model.split_once('/').map_or(model, |(_, id)| id)
}

pub struct ProviderRegistry {
    catalog: Vec<ModelInfo>,
    configured_providers: Vec<String>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn from_credentials(store: &CredentialStore) -> Self {
        let configured_providers: Vec<String> = store.providers.keys().cloned().collect();
        let catalog = catalog::builtin_models();
        Self {
            catalog,
            configured_providers,
        }
    }

    #[must_use]
    pub fn resolve_model(&self, model: &str) -> Option<&ModelInfo> {
        let id = model_api_id(model);
        self.catalog.iter().find(|m| m.id == id)
    }

    #[must_use]
    pub fn max_tokens(&self, model: &str) -> u32 {
        self.resolve_model(model).map_or_else(
            || catalog::default_max_tokens(model_api_id(model)),
            |m| m.max_output_tokens,
        )
    }

    pub fn provider_for_model<'a>(&'a self, model: &'a str) -> Result<&'a str, ApiError> {
        model
            .split_once('/')
            .map(|(provider, _)| provider)
            .ok_or_else(|| {
                ApiError::Auth(format!(
                    "Model '{model}' must include a provider prefix (e.g. 'anthropic/{model}'). \
                     Run `acrawl auth` to configure a provider."
                ))
            })
    }

    pub fn build_client(
        &self,
        model: &str,
        store: &CredentialStore,
    ) -> Result<ProviderClient, ApiError> {
        let provider_id = self.provider_for_model(model)?;
        let api_id = model_api_id(model);
        let default_config = StoredProviderConfig::default();
        let config = store
            .providers
            .get(provider_id)
            .or_else(|| legacy_provider_key(provider_id).and_then(|k| store.providers.get(k)))
            .unwrap_or(&default_config);

        ProviderClient::from_stored_config(provider_id, config, api_id)
    }

    #[must_use]
    pub fn configured_providers(&self) -> &[String] {
        &self.configured_providers
    }
}

fn legacy_provider_key(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "amazon-bedrock" => Some("bedrock"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{CredentialStore, StoredProviderConfig};
    use crate::error::ApiError;
    use crate::provider::ProviderClient;

    #[test]
    fn registry_resolves_model_by_id() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert!(registry.resolve_model("claude-sonnet-4-6").is_some());
        assert!(registry
            .resolve_model("anthropic/claude-sonnet-4-6")
            .is_some());
        assert!(registry.resolve_model("gpt-4o").is_some());
        assert!(registry.resolve_model("unknown-model").is_none());
    }

    #[test]
    fn model_api_id_strips_prefix() {
        assert_eq!(
            model_api_id("anthropic/claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
        assert_eq!(
            model_api_id("bedrock/anthropic.claude-sonnet-4-6-20250514-v1:0"),
            "anthropic.claude-sonnet-4-6-20250514-v1:0"
        );
        assert_eq!(model_api_id("claude-sonnet-4-6"), "claude-sonnet-4-6");
    }

    #[test]
    fn registry_max_tokens_from_catalog() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert_eq!(registry.max_tokens("anthropic/claude-sonnet-4-6"), 64_000);
        assert_eq!(registry.max_tokens("anthropic/claude-opus-4-6"), 32_000);
        assert_eq!(registry.max_tokens("openai/gpt-4o"), 16_384);
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
            registry
                .provider_for_model("anthropic/claude-sonnet-4-6")
                .unwrap(),
            "anthropic"
        );
        assert_eq!(
            registry
                .provider_for_model("anthropic/claude-sonnet-4-6")
                .unwrap(),
            "anthropic"
        );
        assert_eq!(
            registry.provider_for_model("openai/gpt-4o").unwrap(),
            "openai"
        );
        assert_eq!(
            registry
                .provider_for_model("openai/codex-mini-latest")
                .unwrap(),
            "openai"
        );
        assert_eq!(
            registry.provider_for_model("other/llama3.2").unwrap(),
            "other"
        );
    }

    #[test]
    fn bare_model_name_returns_error() {
        let mut store = CredentialStore::default();
        store.providers.insert(
            "anthropic".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("anthropic-key".into()),
                ..Default::default()
            },
        );

        let registry = ProviderRegistry::from_credentials(&store);
        let result = registry.build_client("claude-sonnet-4-6", &store);

        match result {
            Err(ApiError::Auth(message)) => assert_eq!(
                message,
                "Model 'claude-sonnet-4-6' must include a provider prefix \
                 (e.g. 'anthropic/claude-sonnet-4-6'). \
                 Run `acrawl auth` to configure a provider."
            ),
            _ => panic!("expected provider prefix auth error"),
        }
    }

    #[test]
    fn registry_build_client_falls_back_to_default_config() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        let result = registry.build_client("anthropic/claude-sonnet-4-6", &store);
        assert!(result.is_ok());
    }

    #[test]
    fn build_client_routes_by_model_provider() {
        let mut store = CredentialStore::default();
        store.providers.insert(
            "anthropic".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("anthropic-key".into()),
                ..Default::default()
            },
        );
        store.providers.insert(
            "amazon-bedrock".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("AKIDEXAMPLE".into()),
                aws_secret_access_key: Some("secret".into()),
                region: Some("us-east-1".into()),
                ..Default::default()
            },
        );

        let registry = ProviderRegistry::from_credentials(&store);
        let anthropic_client = registry.build_client("anthropic/claude-sonnet-4-6", &store);
        let bedrock_client = registry.build_client(
            "amazon-bedrock/anthropic.claude-sonnet-4-6-20250514-v1:0",
            &store,
        );

        assert!(anthropic_client.is_ok());
        assert!(matches!(
            anthropic_client.unwrap(),
            ProviderClient::Anthropic(_)
        ));
        assert!(bedrock_client.is_ok());
        assert!(matches!(
            bedrock_client.unwrap(),
            ProviderClient::Bedrock(_)
        ));
    }

    #[test]
    fn build_client_with_explicit_provider_prefix() {
        let mut store = CredentialStore::default();
        store.providers.insert(
            "anthropic".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("anthropic-key".into()),
                ..Default::default()
            },
        );
        store.providers.insert(
            "amazon-bedrock".into(),
            StoredProviderConfig {
                auth_method: "api_key".into(),
                api_key: Some("AKIDEXAMPLE".into()),
                aws_secret_access_key: Some("secret".into()),
                region: Some("us-east-1".into()),
                ..Default::default()
            },
        );

        let registry = ProviderRegistry::from_credentials(&store);
        let client = registry.build_client(
            "amazon-bedrock/anthropic.claude-sonnet-4-6-20250514-v1:0",
            &store,
        );

        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Bedrock(_)));
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
    fn resolve_model_returns_none_for_unknown() {
        let store = CredentialStore::default();
        let registry = ProviderRegistry::from_credentials(&store);
        assert!(registry.resolve_model("totally-fake-model-xyz").is_none());
    }

    #[test]
    fn model_api_id_passes_through_bare_name() {
        assert_eq!(model_api_id("llama3.2"), "llama3.2");
        assert_eq!(model_api_id("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn legacy_bedrock_key_resolves() {
        assert_eq!(legacy_provider_key("amazon-bedrock"), Some("bedrock"));
        assert_eq!(legacy_provider_key("anthropic"), None);
        assert_eq!(legacy_provider_key("openai"), None);
    }
}
