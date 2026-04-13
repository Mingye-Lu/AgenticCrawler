//! Built-in model catalog and `models.dev` integration.
//!
//! Provides model metadata (aliases, token limits, capabilities, pricing)
//! so the rest of the codebase never needs to hardcode model-specific values.

use super::{ModelCapabilities, ModelInfo, ModelPricing};

/// Return the built-in model catalog (well-known models shipped with the binary).
#[must_use]
pub fn builtin_models() -> Vec<ModelInfo> {
    let mut models = Vec::new();
    models.extend(anthropic_models());
    models.extend(openai_models());
    models
}

/// Infer provider ID from a model name when the model is not in the catalog.
///
/// This is the fallback for user-supplied model IDs that aren't registered.
#[must_use]
pub fn infer_provider(model: &str) -> &'static str {
    if model.starts_with("claude") {
        "anthropic"
    } else if model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("codex-")
        || model.starts_with("chatgpt-")
    {
        "openai"
    } else {
        "other"
    }
}

/// Default max output tokens for unknown models (fallback when not in catalog).
#[must_use]
pub fn default_max_tokens(model: &str) -> u32 {
    match infer_provider(model) {
        "anthropic" => {
            if model.contains("opus") {
                32_000
            } else {
                64_000
            }
        }
        "openai" => 16_384,
        _ => 8_192,
    }
}

fn anthropic_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-opus-4-6".into(),
            display_name: "Claude Opus 4.6".into(),
            aliases: vec!["opus".into()],
            provider_id: "anthropic".into(),
            max_output_tokens: 32_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 15.0,
                output_per_mtok: 75.0,
                cache_read_per_mtok: Some(1.5),
                cache_write_per_mtok: Some(18.75),
            }),
        },
        ModelInfo {
            id: "claude-sonnet-4-6".into(),
            display_name: "Claude Sonnet 4.6".into(),
            aliases: vec!["sonnet".into()],
            provider_id: "anthropic".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_read_per_mtok: Some(0.3),
                cache_write_per_mtok: Some(3.75),
            }),
        },
        ModelInfo {
            id: "claude-haiku-4-5-20251213".into(),
            display_name: "Claude Haiku 4.5".into(),
            aliases: vec!["haiku".into()],
            provider_id: "anthropic".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 0.80,
                output_per_mtok: 4.0,
                cache_read_per_mtok: Some(0.08),
                cache_write_per_mtok: Some(1.0),
            }),
        },
    ]
}

fn openai_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gpt-4o".into(),
            display_name: "GPT-4o".into(),
            aliases: vec!["gpt4o".into(), "4o".into()],
            provider_id: "openai".into(),
            max_output_tokens: 16_384,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 2.5,
                output_per_mtok: 10.0,
                cache_read_per_mtok: Some(1.25),
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "gpt-4-turbo".into(),
            display_name: "GPT-4 Turbo".into(),
            aliases: vec!["gpt4".into()],
            provider_id: "openai".into(),
            max_output_tokens: 4_096,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 10.0,
                output_per_mtok: 30.0,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "o3".into(),
            display_name: "o3".into(),
            aliases: vec![],
            provider_id: "openai".into(),
            max_output_tokens: 100_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: true,
                tool_use: true,
                vision: true,
                streaming: true,
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 2.0,
                output_per_mtok: 8.0,
                cache_read_per_mtok: Some(0.5),
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "o4-mini".into(),
            display_name: "o4 Mini".into(),
            aliases: vec![],
            provider_id: "openai".into(),
            max_output_tokens: 100_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: true,
                tool_use: true,
                vision: true,
                streaming: true,
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 1.10,
                output_per_mtok: 4.40,
                cache_read_per_mtok: Some(0.275),
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "codex-mini-latest".into(),
            display_name: "Codex Mini".into(),
            aliases: vec!["codex".into()],
            provider_id: "openai".into(),
            max_output_tokens: 100_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: true,
                tool_use: true,
                vision: false,
                streaming: true,
            },
            pricing: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_is_non_empty() {
        let models = builtin_models();
        assert!(models.len() >= 5, "expected at least 5 built-in models");
    }

    #[test]
    fn all_models_have_provider_id() {
        for model in builtin_models() {
            assert!(
                !model.provider_id.is_empty(),
                "model {} has empty provider_id",
                model.id
            );
        }
    }

    #[test]
    fn aliases_resolve_correctly() {
        let models = builtin_models();
        let sonnet = models
            .iter()
            .find(|m| m.aliases.contains(&"sonnet".to_string()));
        assert!(sonnet.is_some(), "alias 'sonnet' should resolve");
        assert_eq!(sonnet.unwrap().id, "claude-sonnet-4-6");
    }

    #[test]
    fn infer_provider_for_known_prefixes() {
        assert_eq!(infer_provider("claude-sonnet-4-6"), "anthropic");
        assert_eq!(infer_provider("gpt-4o"), "openai");
        assert_eq!(infer_provider("o3"), "openai");
        assert_eq!(infer_provider("o4-mini"), "openai");
        assert_eq!(infer_provider("codex-mini-latest"), "openai");
        assert_eq!(infer_provider("llama3.2"), "other");
    }

    #[test]
    fn default_max_tokens_for_known_prefixes() {
        assert_eq!(default_max_tokens("claude-sonnet-4-6"), 64_000);
        assert_eq!(default_max_tokens("claude-opus-4-6"), 32_000);
        assert_eq!(default_max_tokens("gpt-4o"), 16_384);
        assert_eq!(default_max_tokens("llama3.2"), 8_192);
    }
}
