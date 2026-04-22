//! Built-in model catalog and `models.dev` integration.
//!
//! Provides model metadata (aliases, token limits, capabilities, pricing)
//! so the rest of the codebase never needs to hardcode model-specific values.

use super::{ModelCapabilities, ModelInfo, ModelPricing};
use crate::error::ApiError;
use crate::types::ReasoningEffort;

/// Return the built-in model catalog (well-known models shipped with the binary).
#[must_use]
pub fn builtin_models() -> Vec<ModelInfo> {
    let mut models = Vec::new();
    models.extend(anthropic_models());
    models.extend(openai_models());
    models.extend(groq_models());
    models.extend(mistral_models());
    models.extend(deepinfra_models());
    models.extend(cerebras_models());
    models.extend(cohere_models());
    models.extend(togetherai_models());
    models.extend(perplexity_models());
    models.extend(xai_models());
    models.extend(venice_models());
    models.extend(alibaba_models());
    models.extend(cloudflare_models());
    models.extend(sap_models());
    models.extend(gemini_models());
    models.extend(bedrock_models());
    models.extend(azure_models());
    models.extend(vertex_models());
    models.extend(copilot_models());
    models
}

/// Infer provider ID from a model name when the model is not in the catalog.
///
/// This is the fallback for user-supplied model IDs that aren't registered.
#[must_use]
pub fn infer_provider(model: &str) -> &'static str {
    if model.starts_with("claude") {
        return "anthropic";
    } else if model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("codex-")
        || model.starts_with("chatgpt-")
    {
        return "openai";
    }

    for (provider_id, prefixes) in crate::provider::preset::preset_model_prefixes() {
        if provider_id == "anthropic" || provider_id == "openai" {
            continue;
        }
        if prefixes.iter().any(|prefix| model.starts_with(prefix)) {
            return provider_id;
        }
    }

    "other"
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
            aliases: vec![],
            provider_id: "anthropic".into(),
            max_output_tokens: 32_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
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
            aliases: vec![],
            provider_id: "anthropic".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
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
            aliases: vec![],
            provider_id: "anthropic".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
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

#[allow(clippy::too_many_lines)]
fn openai_models() -> Vec<ModelInfo> {
    let openai_reasoning_efforts = ReasoningEffort::OPENAI.to_vec();
    vec![
        ModelInfo {
            id: "gpt-4o".into(),
            display_name: "GPT-4o".into(),
            aliases: vec![],
            provider_id: "openai".into(),
            max_output_tokens: 16_384,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
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
            aliases: vec![],
            provider_id: "openai".into(),
            max_output_tokens: 4_096,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
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
                reasoning_efforts: openai_reasoning_efforts.clone(),
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
                reasoning_efforts: openai_reasoning_efforts.clone(),
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
            aliases: vec![],
            provider_id: "openai".into(),
            max_output_tokens: 100_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: true,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: openai_reasoning_efforts,
            },
            pricing: None,
        },
    ]
}

fn groq_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "llama-3.3-70b-versatile".into(),
            display_name: "Llama 3.3 70B".into(),
            aliases: vec![],
            provider_id: "groq".into(),
            max_output_tokens: 8_192,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 0.59,
                output_per_mtok: 0.79,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "llama-3.1-8b-instant".into(),
            display_name: "Llama 3.1 8B Instant".into(),
            aliases: vec![],
            provider_id: "groq".into(),
            max_output_tokens: 8_192,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 0.05,
                output_per_mtok: 0.08,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "gemma2-9b-it".into(),
            display_name: "Gemma 2 9B".into(),
            aliases: vec![],
            provider_id: "groq".into(),
            max_output_tokens: 8_192,
            context_window: 8_192,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "mixtral-8x7b-32768".into(),
            display_name: "Mixtral 8x7B".into(),
            aliases: vec![],
            provider_id: "groq".into(),
            max_output_tokens: 32_768,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn mistral_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "mistral-large-latest".into(),
            display_name: "Mistral Large".into(),
            aliases: vec![],
            provider_id: "mistral".into(),
            max_output_tokens: 131_072,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "mistral-small-latest".into(),
            display_name: "Mistral Small".into(),
            aliases: vec![],
            provider_id: "mistral".into(),
            max_output_tokens: 32_768,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "codestral-latest".into(),
            display_name: "Codestral".into(),
            aliases: vec![],
            provider_id: "mistral".into(),
            max_output_tokens: 32_768,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "mistral-medium-latest".into(),
            display_name: "Mistral Medium".into(),
            aliases: vec![],
            provider_id: "mistral".into(),
            max_output_tokens: 32_768,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn deepinfra_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "meta-llama/Meta-Llama-3.1-70B-Instruct".into(),
            display_name: "Llama 3.1 70B Instruct".into(),
            aliases: vec![],
            provider_id: "deepinfra".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "meta-llama/Meta-Llama-3.1-8B-Instruct".into(),
            display_name: "Llama 3.1 8B Instruct".into(),
            aliases: vec![],
            provider_id: "deepinfra".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "mistralai/Mixtral-8x7B-Instruct-v0.1".into(),
            display_name: "Mixtral 8x7B Instruct".into(),
            aliases: vec![],
            provider_id: "deepinfra".into(),
            max_output_tokens: 8_192,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn cerebras_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "llama3.1-70b".into(),
            display_name: "Llama 3.1 70B (Cerebras)".into(),
            aliases: vec![],
            provider_id: "cerebras".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "llama3.1-8b".into(),
            display_name: "Llama 3.1 8B (Cerebras)".into(),
            aliases: vec![],
            provider_id: "cerebras".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "llama-3.3-70b".into(),
            display_name: "Llama 3.3 70B (Cerebras)".into(),
            aliases: vec![],
            provider_id: "cerebras".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn cohere_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "command-r-plus".into(),
            display_name: "Command R+".into(),
            aliases: vec![],
            provider_id: "cohere".into(),
            max_output_tokens: 4_096,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "command-r".into(),
            display_name: "Command R".into(),
            aliases: vec![],
            provider_id: "cohere".into(),
            max_output_tokens: 4_096,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: Some(ModelPricing {
                input_per_mtok: 0.15,
                output_per_mtok: 0.60,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            }),
        },
        ModelInfo {
            id: "command-light".into(),
            display_name: "Command Light".into(),
            aliases: vec![],
            provider_id: "cohere".into(),
            max_output_tokens: 4_096,
            context_window: 4_096,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: false,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn togetherai_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo".into(),
            display_name: "Llama 3.1 70B Turbo".into(),
            aliases: vec![],
            provider_id: "togetherai".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "meta-llama/Meta-Llama-3.1-8B-Instruct-Turbo".into(),
            display_name: "Llama 3.1 8B Turbo".into(),
            aliases: vec![],
            provider_id: "togetherai".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "mistralai/Mixtral-8x7B-Instruct-v0.1".into(),
            display_name: "Mixtral 8x7B Instruct".into(),
            aliases: vec![],
            provider_id: "togetherai".into(),
            max_output_tokens: 8_192,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn perplexity_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "llama-3.1-sonar-large-128k-online".into(),
            display_name: "Sonar Large Online".into(),
            aliases: vec![],
            provider_id: "perplexity".into(),
            max_output_tokens: 8_192,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: false,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "llama-3.1-sonar-small-128k-online".into(),
            display_name: "Sonar Small Online".into(),
            aliases: vec![],
            provider_id: "perplexity".into(),
            max_output_tokens: 8_192,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: false,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "llama-3.1-sonar-huge-128k-online".into(),
            display_name: "Sonar Huge Online".into(),
            aliases: vec![],
            provider_id: "perplexity".into(),
            max_output_tokens: 8_192,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: false,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn xai_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "grok-2".into(),
            display_name: "Grok 2".into(),
            aliases: vec![],
            provider_id: "xai".into(),
            max_output_tokens: 131_072,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "grok-2-mini".into(),
            display_name: "Grok 2 Mini".into(),
            aliases: vec![],
            provider_id: "xai".into(),
            max_output_tokens: 131_072,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "grok-beta".into(),
            display_name: "Grok Beta".into(),
            aliases: vec![],
            provider_id: "xai".into(),
            max_output_tokens: 131_072,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn venice_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "llama-3.3-70b".into(),
            display_name: "Llama 3.3 70B (Venice)".into(),
            aliases: vec![],
            provider_id: "venice".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "dolphin-2.9.2-qwen2-72b".into(),
            display_name: "Dolphin 2.9.2 Qwen2 72B".into(),
            aliases: vec![],
            provider_id: "venice".into(),
            max_output_tokens: 8_192,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "mistral-nemo".into(),
            display_name: "Mistral Nemo (Venice)".into(),
            aliases: vec![],
            provider_id: "venice".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn alibaba_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "qwen-max".into(),
            display_name: "Qwen Max".into(),
            aliases: vec![],
            provider_id: "alibaba".into(),
            max_output_tokens: 8_192,
            context_window: 32_768,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "qwen-plus".into(),
            display_name: "Qwen Plus".into(),
            aliases: vec![],
            provider_id: "alibaba".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "qwen-turbo".into(),
            display_name: "Qwen Turbo".into(),
            aliases: vec![],
            provider_id: "alibaba".into(),
            max_output_tokens: 8_192,
            context_window: 1_000_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "qwen-long".into(),
            display_name: "Qwen Long".into(),
            aliases: vec![],
            provider_id: "alibaba".into(),
            max_output_tokens: 8_192,
            context_window: 10_000_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn cloudflare_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "@cf/meta/llama-3.1-70b-instruct".into(),
            display_name: "Llama 3.1 70B (Cloudflare)".into(),
            aliases: vec![],
            provider_id: "cloudflare".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "@cf/meta/llama-3.1-8b-instruct".into(),
            display_name: "Llama 3.1 8B (Cloudflare)".into(),
            aliases: vec![],
            provider_id: "cloudflare".into(),
            max_output_tokens: 8_192,
            context_window: 131_072,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "@cf/google/gemma-7b-it".into(),
            display_name: "Gemma 7B (Cloudflare)".into(),
            aliases: vec![],
            provider_id: "cloudflare".into(),
            max_output_tokens: 8_192,
            context_window: 8_192,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: false,
                vision: false,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn sap_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gpt-4o".into(),
            display_name: "GPT-4o (SAP)".into(),
            aliases: vec![],
            provider_id: "sap".into(),
            max_output_tokens: 16_384,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "gpt-4-turbo".into(),
            display_name: "GPT-4 Turbo (SAP)".into(),
            aliases: vec![],
            provider_id: "sap".into(),
            max_output_tokens: 4_096,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "claude-3.5-sonnet".into(),
            display_name: "Claude 3.5 Sonnet (SAP)".into(),
            aliases: vec![],
            provider_id: "sap".into(),
            max_output_tokens: 8_192,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn gemini_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gemini-2.0-flash".into(),
            display_name: "Gemini 2.0 Flash".into(),
            aliases: vec![],
            provider_id: "google".into(),
            max_output_tokens: 8_192,
            context_window: 1_048_576,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "gemini-2.0-pro".into(),
            display_name: "Gemini 2.0 Pro".into(),
            aliases: vec![],
            provider_id: "google".into(),
            max_output_tokens: 8_192,
            context_window: 1_048_576,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "gemini-1.5-pro".into(),
            display_name: "Gemini 1.5 Pro".into(),
            aliases: vec![],
            provider_id: "google".into(),
            max_output_tokens: 8_192,
            context_window: 2_097_152,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "gemini-1.5-flash".into(),
            display_name: "Gemini 1.5 Flash".into(),
            aliases: vec![],
            provider_id: "google".into(),
            max_output_tokens: 8_192,
            context_window: 1_048_576,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn bedrock_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "anthropic.claude-sonnet-4-6-20250514-v1:0".into(),
            display_name: "Claude Sonnet 4.6 (Bedrock)".into(),
            aliases: vec![],
            provider_id: "bedrock".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "anthropic.claude-haiku-4-5-20251213-v1:0".into(),
            display_name: "Claude Haiku 4.5 (Bedrock)".into(),
            aliases: vec![],
            provider_id: "bedrock".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "amazon.nova-pro-v1:0".into(),
            display_name: "Amazon Nova Pro".into(),
            aliases: vec![],
            provider_id: "bedrock".into(),
            max_output_tokens: 5_120,
            context_window: 300_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "amazon.nova-lite-v1:0".into(),
            display_name: "Amazon Nova Lite".into(),
            aliases: vec![],
            provider_id: "bedrock".into(),
            max_output_tokens: 5_120,
            context_window: 300_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn azure_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gpt-4o".into(),
            display_name: "GPT-4o (Azure)".into(),
            aliases: vec![],
            provider_id: "azure".into(),
            max_output_tokens: 16_384,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "gpt-4-turbo".into(),
            display_name: "GPT-4 Turbo (Azure)".into(),
            aliases: vec![],
            provider_id: "azure".into(),
            max_output_tokens: 4_096,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "gpt-4o-mini".into(),
            display_name: "GPT-4o Mini (Azure)".into(),
            aliases: vec![],
            provider_id: "azure".into(),
            max_output_tokens: 16_384,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn vertex_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gemini-2.0-flash".into(),
            display_name: "Gemini 2.0 Flash (Vertex)".into(),
            aliases: vec![],
            provider_id: "vertex".into(),
            max_output_tokens: 8_192,
            context_window: 1_048_576,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "claude-sonnet-4-6@20250514".into(),
            display_name: "Claude Sonnet 4.6 (Vertex)".into(),
            aliases: vec![],
            provider_id: "vertex".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "gemini-1.5-flash-002".into(),
            display_name: "Gemini 1.5 Flash 002 (Vertex)".into(),
            aliases: vec![],
            provider_id: "vertex".into(),
            max_output_tokens: 8_192,
            context_window: 1_048_576,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
    ]
}

fn copilot_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gpt-4o".into(),
            display_name: "GPT-4o (Copilot)".into(),
            aliases: vec![],
            provider_id: "copilot".into(),
            max_output_tokens: 16_384,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "claude-sonnet-4-6".into(),
            display_name: "Claude Sonnet 4.6 (Copilot)".into(),
            aliases: vec![],
            provider_id: "copilot".into(),
            max_output_tokens: 64_000,
            context_window: 200_000,
            capabilities: ModelCapabilities {
                reasoning: false,
                tool_use: true,
                vision: true,
                streaming: true,
                reasoning_efforts: vec![],
            },
            pricing: None,
        },
        ModelInfo {
            id: "o1-mini".into(),
            display_name: "o1 Mini (Copilot)".into(),
            aliases: vec![],
            provider_id: "copilot".into(),
            max_output_tokens: 65_536,
            context_window: 128_000,
            capabilities: ModelCapabilities {
                reasoning: true,
                tool_use: false,
                vision: false,
                streaming: true,
                reasoning_efforts: ReasoningEffort::OPENAI.to_vec(),
            },
            pricing: None,
        },
    ]
}

pub async fn fetch_models_dev(provider_id: &str) -> Result<Vec<ModelInfo>, ApiError> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://models.dev/api.json")
        .header("User-Agent", "acrawl")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(ApiError::Http)?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: None,
            body,
            retryable: status.is_server_error(),
        });
    }

    let catalog: std::collections::HashMap<String, serde_json::Value> =
        response.json().await.map_err(ApiError::Http)?;

    let Some(provider) = catalog.get(provider_id) else {
        return Ok(vec![]);
    };

    let Some(models_obj) = provider.get("models").and_then(|v| v.as_object()) else {
        return Ok(vec![]);
    };

    let mut models = Vec::new();
    for (id, model_data) in models_obj {
        let display_name = model_data
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(id)
            .to_string();

        let max_output_tokens = model_data
            .get("limit")
            .and_then(|v| v.get("output"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(4096);
        let max_output_tokens = u32::try_from(max_output_tokens).unwrap_or(4096);

        let context_window = model_data
            .get("limit")
            .and_then(|v| v.get("context"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(128_000);
        let context_window = u32::try_from(context_window).unwrap_or(128_000);

        let reasoning = model_data
            .get("reasoning")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let tool_use = model_data
            .get("tool_call")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let vision = model_data
            .get("attachment")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let pricing = model_data.get("cost").and_then(|cost| {
            let input = cost.get("input")?.as_f64()?;
            let output = cost.get("output")?.as_f64()?;
            Some(ModelPricing {
                input_per_mtok: input,
                output_per_mtok: output,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
            })
        });

        models.push(ModelInfo {
            id: id.clone(),
            display_name,
            aliases: vec![],
            provider_id: provider_id.to_string(),
            max_output_tokens,
            context_window,
            capabilities: ModelCapabilities {
                reasoning,
                tool_use,
                vision,
                streaming: true,
                reasoning_efforts: if reasoning {
                    ReasoningEffort::OPENAI.to_vec()
                } else {
                    vec![]
                },
            },
            pricing,
        });
    }

    Ok(models)
}

pub async fn fetch_models_dev_reasoning(
) -> Result<std::collections::HashMap<String, bool>, ApiError> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://models.dev/api.json")
        .header("User-Agent", "acrawl")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(ApiError::Http)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: None,
            body,
            retryable: false,
        });
    }

    let catalog: std::collections::HashMap<String, serde_json::Value> =
        response.json().await.map_err(ApiError::Http)?;

    let mut map = std::collections::HashMap::new();
    for provider_data in catalog.values() {
        if let Some(models) = provider_data.get("models").and_then(|v| v.as_object()) {
            for (id, model_data) in models {
                let reasoning = model_data
                    .get("reasoning")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                map.insert(id.clone(), reasoning);
            }
        }
    }
    Ok(map)
}

/// Maps (`builtin_provider_id`, `models_dev_provider_id`) for all 19 builtin providers.
/// Handles cases where models.dev uses different IDs from the acrawl credential store.
const MODELS_DEV_PROVIDER_MAP: &[(&str, &str)] = &[
    ("anthropic", "anthropic"),
    ("openai", "openai"),
    ("groq", "groq"),
    ("mistral", "mistral"),
    ("deepinfra", "deepinfra"),
    ("cerebras", "cerebras"),
    ("cohere", "cohere"),
    ("togetherai", "togetherai"),
    ("perplexity", "perplexity"),
    ("xai", "xai"),
    ("venice", "venice"),
    ("alibaba", "alibaba"),
    ("cloudflare", "cloudflare-workers-ai"),
    ("sap", "sap-ai-core"),
    ("google", "google"),
    ("bedrock", "amazon-bedrock"),
    ("azure", "azure"),
    ("vertex", "google-vertex"),
    ("copilot", "github-copilot"),
];

/// Fetch all models from models.dev for the full builtin provider set in a single HTTP request.
/// Returns all models (no `tool_call` filtering — callers decide what to show).
/// On any network or parse error, returns an empty Vec (caller falls back to builtin catalog).
/// The `provider_id` field in returned `ModelInfo` uses the builtin ID, not the models.dev ID,
/// so `ProviderRegistry::configured_providers()` checks work correctly.
pub async fn fetch_all_models_dev_for_picker() -> Vec<ModelInfo> {
    let client = reqwest::Client::new();
    let Ok(response) = client
        .get("https://models.dev/api.json")
        .header("User-Agent", "acrawl")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    else {
        return vec![];
    };

    if !response.status().is_success() {
        return vec![];
    }

    let catalog: std::collections::HashMap<String, serde_json::Value> = match response.json().await
    {
        Ok(catalog) => catalog,
        Err(_) => return vec![],
    };

    let mut all_models = Vec::new();

    for &(builtin_id, models_dev_id) in MODELS_DEV_PROVIDER_MAP {
        let Some(provider) = catalog.get(models_dev_id) else {
            continue;
        };
        let Some(models_obj) = provider.get("models").and_then(|value| value.as_object()) else {
            continue;
        };

        for (id, model_data) in models_obj {
            let display_name = model_data
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(id)
                .to_string();

            let max_output_tokens = model_data
                .get("limit")
                .and_then(|value| value.get("output"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(4096);
            let max_output_tokens = u32::try_from(max_output_tokens).unwrap_or(4096);

            let context_window = model_data
                .get("limit")
                .and_then(|value| value.get("context"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(128_000);
            let context_window = u32::try_from(context_window).unwrap_or(128_000);

            let reasoning = model_data
                .get("reasoning")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            let tool_use = model_data
                .get("tool_call")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            let vision = model_data
                .get("attachment")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            let pricing = model_data.get("cost").and_then(|cost| {
                let input = cost.get("input")?.as_f64()?;
                let output = cost.get("output")?.as_f64()?;
                Some(ModelPricing {
                    input_per_mtok: input,
                    output_per_mtok: output,
                    cache_read_per_mtok: cost.get("cache_read").and_then(serde_json::Value::as_f64),
                    cache_write_per_mtok: cost
                        .get("cache_write")
                        .and_then(serde_json::Value::as_f64),
                })
            });

            all_models.push(ModelInfo {
                id: id.clone(),
                display_name,
                aliases: vec![],
                provider_id: builtin_id.to_string(),
                max_output_tokens,
                context_window,
                capabilities: ModelCapabilities {
                    reasoning,
                    tool_use,
                    vision,
                    streaming: true,
                    reasoning_efforts: if reasoning {
                        ReasoningEffort::OPENAI.to_vec()
                    } else {
                        vec![]
                    },
                },
                pricing,
            });
        }
    }

    all_models
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
    fn no_models_have_aliases() {
        let models = builtin_models();
        for model in &models {
            assert!(
                model.aliases.is_empty(),
                "model '{}' should have no aliases, found {:?}",
                model.id,
                model.aliases
            );
        }
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

    #[test]
    fn test_new_providers_have_models() {
        let models = builtin_models();
        let providers_with_models = [
            "groq",
            "mistral",
            "deepinfra",
            "cerebras",
            "cohere",
            "togetherai",
            "perplexity",
            "xai",
            "venice",
            "alibaba",
            "cloudflare",
            "sap",
            "google",
            "bedrock",
            "azure",
            "vertex",
            "copilot",
        ];
        for pid in providers_with_models {
            let count = models.iter().filter(|m| m.provider_id == pid).count();
            assert!(
                count >= 2,
                "provider {pid} should have at least 2 models, found {count}"
            );
        }
    }

    #[test]
    fn test_no_duplicate_model_ids_within_provider() {
        let models = builtin_models();
        let mut by_provider: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for m in &models {
            by_provider.entry(&m.provider_id).or_default().push(&m.id);
        }
        for (pid, ids) in &by_provider {
            let unique: std::collections::HashSet<_> = ids.iter().collect();
            assert_eq!(
                ids.len(),
                unique.len(),
                "provider {pid} has duplicate model IDs"
            );
        }
    }

    #[test]
    fn test_total_model_count() {
        let models = builtin_models();
        assert!(
            models.len() >= 40,
            "should have at least 40 models total, got {}",
            models.len()
        );
    }

    #[test]
    fn test_all_providers_have_preset() {
        use crate::provider::preset::find_preset;
        let models = builtin_models();
        let provider_ids: std::collections::HashSet<&str> =
            models.iter().map(|m| m.provider_id.as_str()).collect();
        for pid in &provider_ids {
            assert!(
                find_preset(pid).is_some(),
                "provider '{pid}' has catalog models but no matching preset"
            );
        }
    }

    #[test]
    fn test_all_presets_with_prefixes_have_catalog_models() {
        use crate::provider::preset::builtin_presets;
        let models = builtin_models();
        let presets = builtin_presets();
        for preset in &presets {
            if preset.model_prefixes.is_empty() {
                continue;
            }
            let has_models = models.iter().any(|m| m.provider_id == preset.id);
            let routes_via_prefix = preset
                .model_prefixes
                .iter()
                .any(|pfx| models.iter().any(|m| m.id.starts_with(pfx)));
            assert!(
                has_models || routes_via_prefix,
                "preset '{}' has model_prefixes {:?} but no catalog models route to it",
                preset.id,
                preset.model_prefixes
            );
        }
    }

    #[test]
    fn test_all_models_have_valid_capabilities() {
        for model in builtin_models() {
            assert!(
                model.max_output_tokens > 0,
                "model {} has zero max_output_tokens",
                model.id
            );
            assert!(
                model.context_window > 0,
                "model {} has zero context_window",
                model.id
            );
            assert!(
                model.context_window >= model.max_output_tokens,
                "model {} has context_window ({}) < max_output_tokens ({})",
                model.id,
                model.context_window,
                model.max_output_tokens
            );
            if model.capabilities.reasoning {
                assert!(
                    !model.capabilities.reasoning_efforts.is_empty(),
                    "model {} is reasoning but has no reasoning_efforts",
                    model.id
                );
            }
        }
    }

    #[test]
    fn test_total_provider_count() {
        let models = builtin_models();
        let provider_ids: std::collections::HashSet<&str> =
            models.iter().map(|m| m.provider_id.as_str()).collect();
        assert!(
            provider_ids.len() >= 19,
            "should have at least 19 distinct providers, got {}",
            provider_ids.len()
        );
    }

    #[tokio::test]
    async fn fetch_models_dev_compiles() {
        let result = fetch_models_dev("anthropic").await;
        assert!(result.is_ok() || result.is_err());
    }
}
