use std::collections::HashMap;
use std::sync::OnceLock;

use api::provider::ProviderRegistry;

pub(super) fn model_supports_reasoning(model: &str) -> bool {
    let api_model = api::provider::model_api_id(model);
    let store = api::load_credentials().unwrap_or_default();
    let registry = ProviderRegistry::from_credentials(&store);
    if let Some(info) = registry.resolve_model(api_model) {
        return info.capabilities.reasoning;
    }
    models_dev_reasoning_cache()
        .get(api_model)
        .copied()
        .unwrap_or(false)
}

pub(super) fn model_reasoning_efforts(model: &str) -> Vec<api::ReasoningEffort> {
    let api_model = api::provider::model_api_id(model);
    let store = api::load_credentials().unwrap_or_default();
    let registry = ProviderRegistry::from_credentials(&store);
    if let Some(info) = registry.resolve_model(api_model) {
        return info.capabilities.reasoning_efforts.clone();
    }
    if model_supports_reasoning(model) {
        api::ReasoningEffort::OPENAI.to_vec()
    } else {
        vec![]
    }
}

pub(super) fn models_dev_reasoning_cache() -> &'static HashMap<String, bool> {
    static CACHE: OnceLock<HashMap<String, bool>> = OnceLock::new();
    CACHE.get_or_init(|| {
        if let Some(rt) = crate::TOKIO_RUNTIME.get() {
            rt.block_on(api::provider::catalog::fetch_models_dev_reasoning())
                .ok()
        } else {
            tokio::runtime::Runtime::new().ok().and_then(|rt| {
                rt.block_on(api::provider::catalog::fetch_models_dev_reasoning())
                    .ok()
            })
        }
        .unwrap_or_default()
    })
}
