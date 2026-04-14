#[derive(Debug, Clone)]
pub enum AuthHeaderFormat {
    Bearer,
    XApiKey(&'static str),
    AzureApiKey,
}

#[derive(Debug, Clone)]
pub enum ProviderProtocol {
    Anthropic,
    OpenAiResponses,
    ChatCompletions,
    Gemini,
    Bedrock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCategory {
    Popular,
    OssHosting,
    Specialized,
    Enterprise,
    Gateway,
    Other,
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub display_name: &'static str,
    pub base_url: &'static str,
    pub chat_path: &'static str,
    pub api_key_env_var: Option<&'static str>,
    pub auth_header_format: AuthHeaderFormat,
    pub supports_tools: bool,
    pub supports_streaming_tools: bool,
    pub supports_vision: bool,
    pub model_prefixes: &'static [&'static str],
    pub protocol: ProviderProtocol,
    pub category: ProviderCategory,
    pub transform_id: Option<&'static str>,
}

static BUILTIN_PRESETS: [ProviderPreset; 24] = [
    ProviderPreset {
        id: "anthropic",
        display_name: "Anthropic",
        base_url: "https://api.anthropic.com",
        chat_path: "/v1/messages",
        api_key_env_var: Some("ANTHROPIC_API_KEY"),
        auth_header_format: AuthHeaderFormat::XApiKey("x-api-key"),
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &["claude"],
        protocol: ProviderProtocol::Anthropic,
        category: ProviderCategory::Popular,
        transform_id: None,
    },
    ProviderPreset {
        id: "openai",
        display_name: "OpenAI",
        base_url: "https://api.openai.com",
        chat_path: "/v1/responses",
        api_key_env_var: Some("OPENAI_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &["gpt-", "o1", "o3", "o4", "codex-", "chatgpt-"],
        protocol: ProviderProtocol::OpenAiResponses,
        category: ProviderCategory::Popular,
        transform_id: None,
    },
    ProviderPreset {
        id: "google",
        display_name: "Google Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        chat_path: "",
        api_key_env_var: Some("GEMINI_API_KEY"),
        auth_header_format: AuthHeaderFormat::XApiKey("x-goog-api-key"),
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &["gemini-"],
        protocol: ProviderProtocol::Gemini,
        category: ProviderCategory::Popular,
        transform_id: None,
    },
    ProviderPreset {
        id: "bedrock",
        display_name: "AWS Bedrock",
        base_url: "",
        chat_path: "",
        api_key_env_var: Some("AWS_ACCESS_KEY_ID"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &["anthropic.claude"],
        protocol: ProviderProtocol::Bedrock,
        category: ProviderCategory::Enterprise,
        transform_id: None,
    },
    ProviderPreset {
        id: "other",
        display_name: "Other OpenAI-Compatible",
        base_url: "",
        chat_path: "/chat/completions",
        api_key_env_var: None,
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Other,
        transform_id: None,
    },
    ProviderPreset {
        id: "groq",
        display_name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("GROQ_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["llama-3", "gemma2", "mixtral"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::OssHosting,
        transform_id: None,
    },
    ProviderPreset {
        id: "cerebras",
        display_name: "Cerebras",
        base_url: "https://api.cerebras.ai/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("CEREBRAS_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["llama3.1-"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::OssHosting,
        transform_id: None,
    },
    ProviderPreset {
        id: "deepinfra",
        display_name: "DeepInfra",
        base_url: "https://api.deepinfra.com/v1/openai",
        chat_path: "/chat/completions",
        api_key_env_var: Some("DEEPINFRA_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["meta-llama/", "mistralai/"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::OssHosting,
        transform_id: None,
    },
    ProviderPreset {
        id: "togetherai",
        display_name: "Together AI",
        base_url: "https://api.together.xyz/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("TOGETHER_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::OssHosting,
        transform_id: None,
    },
    ProviderPreset {
        id: "perplexity",
        display_name: "Perplexity",
        base_url: "https://api.perplexity.ai",
        chat_path: "/chat/completions",
        api_key_env_var: Some("PERPLEXITY_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: false,
        supports_streaming_tools: false,
        supports_vision: false,
        model_prefixes: &["llama-3.1-sonar"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Specialized,
        transform_id: None,
    },
    ProviderPreset {
        id: "xai",
        display_name: "xAI",
        base_url: "https://api.x.ai/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("XAI_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["grok-"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Specialized,
        transform_id: None,
    },
    ProviderPreset {
        id: "cohere",
        display_name: "Cohere",
        base_url: "https://api.cohere.com/v2",
        chat_path: "/chat",
        api_key_env_var: Some("COHERE_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["command-"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Specialized,
        transform_id: None,
    },
    ProviderPreset {
        id: "vercel",
        display_name: "Vercel AI",
        base_url: "https://api.vercel.ai/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("VERCEL_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Gateway,
        transform_id: None,
    },
    ProviderPreset {
        id: "openrouter",
        display_name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("OPENROUTER_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Gateway,
        transform_id: None,
    },
    ProviderPreset {
        id: "venice",
        display_name: "Venice AI",
        base_url: "https://api.venice.ai/api/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("VENICE_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["dolphin-"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Other,
        transform_id: None,
    },
    ProviderPreset {
        id: "alibaba",
        display_name: "Alibaba (DashScope)",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("DASHSCOPE_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["qwen-"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Other,
        transform_id: None,
    },
    ProviderPreset {
        id: "sap",
        display_name: "SAP AI Core",
        base_url: "https://api.ai.prod.us-east-1.aws.ml.hana.ondemand.com/v2",
        chat_path: "/chat/completions",
        api_key_env_var: Some("SAP_AI_CORE_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Enterprise,
        transform_id: None,
    },
    ProviderPreset {
        id: "mistral",
        display_name: "Mistral AI",
        base_url: "https://api.mistral.ai/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("MISTRAL_API_KEY"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["mistral-", "codestral-"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Specialized,
        transform_id: Some("mistral"),
    },
    ProviderPreset {
        id: "cloudflare",
        display_name: "Cloudflare Workers AI",
        base_url: "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("CLOUDFLARE_API_TOKEN"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &["@cf/"],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Gateway,
        transform_id: None,
    },
    ProviderPreset {
        id: "cloudflare-gateway",
        display_name: "Cloudflare AI Gateway",
        base_url: "https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}",
        chat_path: "/chat/completions",
        api_key_env_var: Some("CLOUDFLARE_API_TOKEN"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Gateway,
        transform_id: None,
    },
    ProviderPreset {
        id: "gitlab",
        display_name: "GitLab Duo",
        base_url: "https://gitlab.com/api/v4/ai/v1",
        chat_path: "/chat/completions",
        api_key_env_var: Some("GITLAB_TOKEN"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: false,
        supports_streaming_tools: false,
        supports_vision: false,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Enterprise,
        transform_id: None,
    },
    ProviderPreset {
        id: "azure",
        display_name: "Azure OpenAI",
        base_url: "https://{resource_name}.openai.azure.com/openai/deployments/{deployment_name}",
        chat_path: "/chat/completions?api-version=2024-02-01",
        api_key_env_var: Some("AZURE_OPENAI_API_KEY"),
        auth_header_format: AuthHeaderFormat::AzureApiKey,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Enterprise,
        transform_id: None,
    },
    ProviderPreset {
        id: "copilot",
        display_name: "GitHub Copilot",
        base_url: "https://api.githubcopilot.com",
        chat_path: "/chat/completions",
        api_key_env_var: None,
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: false,
        model_prefixes: &[],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::Enterprise,
        transform_id: None,
    },
    ProviderPreset {
        id: "vertex",
        display_name: "Google Vertex AI",
        base_url: "https://{region}-aiplatform.googleapis.com/v1/projects/{project_id}/locations/{region}/publishers",
        chat_path: "",
        api_key_env_var: Some("GOOGLE_APPLICATION_CREDENTIALS"),
        auth_header_format: AuthHeaderFormat::Bearer,
        supports_tools: true,
        supports_streaming_tools: true,
        supports_vision: true,
        model_prefixes: &[],
        protocol: ProviderProtocol::Gemini,
        category: ProviderCategory::Enterprise,
        transform_id: None,
    },
];

/// How to add a new provider:
/// 1. Add a `ProviderPreset` entry to `BUILTIN_PRESETS` and keep the array size in sync.
/// 2. Add matching catalog entries in `catalog.rs` so aliases, pricing, and provider IDs line up.
/// 3. Wire auth prompts in `acrawl-cli` if the provider needs anything beyond the default API-key flow.
/// 4. Add or extend `ProviderProtocol` and client builders for non-standard transports.
/// 5. Give the preset clear `model_prefixes` so inference stays predictable.
/// 6. Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check`.
#[must_use]
pub fn builtin_presets() -> Vec<ProviderPreset> {
    BUILTIN_PRESETS.to_vec()
}

#[must_use]
pub fn preset_model_prefixes() -> Vec<(&'static str, &'static [&'static str])> {
    BUILTIN_PRESETS
        .iter()
        .map(|preset| (preset.id, preset.model_prefixes))
        .collect()
}

#[must_use]
pub fn find_preset(provider_id: &str) -> Option<&'static ProviderPreset> {
    BUILTIN_PRESETS
        .iter()
        .find(|preset| preset.id == provider_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_presets_contains_anthropic() {
        let preset = builtin_presets()
            .into_iter()
            .find(|preset| preset.id == "anthropic")
            .expect("anthropic preset should exist");

        assert_eq!(preset.base_url, "https://api.anthropic.com");
        assert_eq!(preset.chat_path, "/v1/messages");
        assert!(matches!(preset.protocol, ProviderProtocol::Anthropic));
    }

    #[test]
    fn test_builtin_presets_contains_openai() {
        let preset = builtin_presets()
            .into_iter()
            .find(|preset| preset.id == "openai")
            .expect("openai preset should exist");

        assert_eq!(preset.base_url, "https://api.openai.com");
        assert_eq!(preset.chat_path, "/v1/responses");
        assert!(matches!(preset.protocol, ProviderProtocol::OpenAiResponses));
    }

    #[test]
    fn test_builtin_presets_contains_bedrock() {
        let preset = builtin_presets()
            .into_iter()
            .find(|preset| preset.id == "bedrock")
            .expect("bedrock preset should exist");

        assert_eq!(preset.api_key_env_var, Some("AWS_ACCESS_KEY_ID"));
        assert!(matches!(preset.protocol, ProviderProtocol::Bedrock));
    }

    #[test]
    fn test_builtin_presets_contains_other() {
        let preset = builtin_presets()
            .into_iter()
            .find(|preset| preset.id == "other")
            .expect("other preset should exist");

        assert_eq!(preset.base_url, "");
        assert_eq!(preset.chat_path, "/chat/completions");
        assert!(matches!(preset.protocol, ProviderProtocol::ChatCompletions));
    }

    #[test]
    fn test_find_preset_by_id() {
        let preset = find_preset("openai").expect("openai preset should resolve by id");
        assert_eq!(preset.display_name, "OpenAI");
    }

    #[test]
    fn test_find_preset_unknown_returns_none() {
        assert!(find_preset("unknown").is_none());
    }

    #[test]
    fn test_perplexity_preset_exists() {
        let p = find_preset("perplexity").expect("perplexity preset should exist");
        assert_eq!(p.base_url, "https://api.perplexity.ai");
    }

    #[test]
    fn test_perplexity_no_tool_support() {
        let p = find_preset("perplexity").expect("perplexity preset should exist");
        assert!(!p.supports_tools);
    }

    #[test]
    fn test_xai_preset_exists() {
        let p = find_preset("xai").expect("xai preset should exist");
        assert_eq!(p.base_url, "https://api.x.ai/v1");
    }

    #[test]
    fn test_sap_preset_exists() {
        let p = find_preset("sap").expect("sap preset should exist");
        assert_eq!(
            p.base_url,
            "https://api.ai.prod.us-east-1.aws.ml.hana.ondemand.com/v2"
        );
        assert!(matches!(p.category, ProviderCategory::Enterprise));
    }

    #[test]
    fn test_cohere_preset_exists() {
        let p = find_preset("cohere").expect("cohere preset should exist");
        assert_eq!(p.base_url, "https://api.cohere.com/v2");
    }

    #[test]
    fn test_vercel_preset_exists() {
        let p = find_preset("vercel").expect("vercel preset should exist");
        assert_eq!(p.base_url, "https://api.vercel.ai/v1");
    }

    #[test]
    fn test_openrouter_preset_exists() {
        let p = find_preset("openrouter").expect("openrouter preset should exist");
        assert_eq!(p.base_url, "https://openrouter.ai/api/v1");
        assert!(p.supports_vision);
    }

    #[test]
    fn test_venice_preset_exists() {
        let p = find_preset("venice").expect("venice preset should exist");
        assert_eq!(p.base_url, "https://api.venice.ai/api/v1");
        assert!(!p.supports_vision);
    }

    #[test]
    fn test_alibaba_preset_exists() {
        let p = find_preset("alibaba").expect("alibaba preset should exist");
        assert_eq!(
            p.base_url,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(p.model_prefixes, &["qwen-"]);
    }

    #[test]
    fn test_groq_preset_exists() {
        let p = find_preset("groq").expect("groq preset should exist");
        assert_eq!(p.base_url, "https://api.groq.com/openai/v1");
        assert!(matches!(p.protocol, ProviderProtocol::ChatCompletions));
    }

    #[test]
    fn test_cerebras_preset_exists() {
        let p = find_preset("cerebras").expect("cerebras preset should exist");
        assert_eq!(p.base_url, "https://api.cerebras.ai/v1");
    }

    #[test]
    fn test_deepinfra_preset_exists() {
        let p = find_preset("deepinfra").expect("deepinfra preset should exist");
        assert_eq!(p.base_url, "https://api.deepinfra.com/v1/openai");
    }

    #[test]
    fn test_togetherai_preset_exists() {
        let p = find_preset("togetherai").expect("togetherai preset should exist");
        assert_eq!(p.base_url, "https://api.together.xyz/v1");
    }

    #[test]
    fn test_groq_routes_through_chat_completions() {
        let p = find_preset("groq").expect("groq preset should exist");
        assert!(matches!(p.protocol, ProviderProtocol::ChatCompletions));
    }

    #[test]
    fn test_mistral_preset_exists() {
        let p = find_preset("mistral").expect("mistral preset should exist");
        assert_eq!(p.base_url, "https://api.mistral.ai/v1");
    }

    #[test]
    fn test_mistral_uses_transform() {
        let p = find_preset("mistral").expect("mistral preset should exist");
        assert_eq!(p.transform_id, Some("mistral"));
    }

    #[test]
    fn test_cloudflare_preset_exists() {
        let p = find_preset("cloudflare").expect("cloudflare preset should exist");
        assert!(p.base_url.contains("cloudflare.com"));
    }

    #[test]
    fn test_cloudflare_requires_account_id() {
        let p = find_preset("cloudflare").expect("cloudflare preset should exist");
        assert!(p.base_url.contains("{account_id}"));
    }

    #[test]
    fn test_cloudflare_gateway_preset_exists() {
        let p = find_preset("cloudflare-gateway").expect("cloudflare-gateway preset should exist");
        assert!(p.base_url.contains("gateway.ai.cloudflare.com"));
    }

    #[test]
    fn test_azure_routes_through_chat_completions() {
        let p = find_preset("azure").expect("azure preset should exist");
        assert!(matches!(p.protocol, ProviderProtocol::ChatCompletions));
        assert!(matches!(
            p.auth_header_format,
            AuthHeaderFormat::AzureApiKey
        ));
        assert!(matches!(p.category, ProviderCategory::Enterprise));
        assert!(p.chat_path.contains("api-version="));
    }

    #[test]
    fn test_gitlab_preset_exists() {
        let p = find_preset("gitlab").expect("gitlab preset should exist");
        assert!(p.base_url.contains("gitlab.com"));
        assert!(matches!(p.category, ProviderCategory::Enterprise));
        assert!(!p.supports_tools);
    }

    #[test]
    fn test_copilot_preset_exists() {
        let p = find_preset("copilot").expect("copilot preset should exist");
        assert_eq!(p.base_url, "https://api.githubcopilot.com");
        assert!(matches!(p.protocol, ProviderProtocol::ChatCompletions));
        assert!(matches!(p.category, ProviderCategory::Enterprise));
        assert!(p.supports_tools);
        assert!(p.api_key_env_var.is_none());
    }

    #[test]
    fn test_vertex_preset_exists() {
        let p = find_preset("vertex").expect("vertex preset should exist");
        assert!(p.base_url.contains("aiplatform.googleapis.com"));
        assert!(matches!(p.protocol, ProviderProtocol::Gemini));
        assert!(matches!(p.category, ProviderCategory::Enterprise));
        assert!(matches!(p.auth_header_format, AuthHeaderFormat::Bearer));
        assert!(p.supports_tools);
        assert!(p.supports_vision);
    }

    #[test]
    fn test_all_presets_have_valid_base_url() {
        for p in builtin_presets() {
            assert!(
                p.base_url.is_empty()
                    || p.base_url.starts_with("https://")
                    || p.base_url.contains('{'),
                "preset {} has invalid base_url: {}",
                p.id,
                p.base_url
            );
        }
    }

    #[test]
    fn test_total_preset_count() {
        let presets = builtin_presets();
        assert!(
            presets.len() >= 22,
            "should have at least 22 presets, got {}",
            presets.len()
        );
    }

    #[test]
    fn test_all_preset_ids_are_unique() {
        let presets = builtin_presets();
        let ids: std::collections::HashSet<&str> = presets.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), presets.len(), "preset IDs must be unique");
    }

    #[test]
    fn test_all_presets_have_env_var_or_are_special() {
        let no_env_allowed = ["other", "copilot"];
        for p in builtin_presets() {
            if no_env_allowed.contains(&p.id) {
                continue;
            }
            assert!(
                p.api_key_env_var.is_some(),
                "preset '{}' should have an api_key_env_var",
                p.id
            );
            let env = p.api_key_env_var.unwrap();
            assert!(
                !env.is_empty(),
                "preset '{}' has empty api_key_env_var",
                p.id
            );
        }
    }

    #[test]
    fn test_preset_protocol_matches_routing() {
        for p in builtin_presets() {
            match p.protocol {
                ProviderProtocol::Anthropic => {
                    assert_eq!(
                        p.id, "anthropic",
                        "only anthropic should use Anthropic protocol"
                    );
                }
                ProviderProtocol::OpenAiResponses => {
                    assert_eq!(
                        p.id, "openai",
                        "only openai should use OpenAiResponses protocol"
                    );
                }
                ProviderProtocol::Bedrock => {
                    assert_eq!(p.id, "bedrock", "only bedrock should use Bedrock protocol");
                }
                ProviderProtocol::Gemini => {
                    assert!(
                        p.id == "google" || p.id == "vertex",
                        "only google/vertex should use Gemini protocol, got '{}'",
                        p.id
                    );
                }
                ProviderProtocol::ChatCompletions => {
                    assert_ne!(
                        p.id, "anthropic",
                        "anthropic should not use ChatCompletions"
                    );
                }
            }
        }
    }
}
