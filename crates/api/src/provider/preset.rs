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

#[derive(Debug, Clone)]
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
}

static BUILTIN_PRESETS: [ProviderPreset; 15] = [
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
        model_prefixes: &["llama3."],
        protocol: ProviderProtocol::ChatCompletions,
        category: ProviderCategory::OssHosting,
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
    },
];

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
}
