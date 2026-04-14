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

static BUILTIN_PRESETS: [ProviderPreset; 3] = [
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
];

#[must_use]
pub fn builtin_presets() -> Vec<ProviderPreset> {
    BUILTIN_PRESETS.to_vec()
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
}
