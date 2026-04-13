use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Represents the LLM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Anthropic,
    OpenAi,
    Other,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::OpenAi => write!(f, "openai"),
            Self::Other => write!(f, "other"),
        }
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "other" => Ok(Self::Other),
            _ => Err(format!("Unknown provider: {s}")),
        }
    }
}

/// Represents the authentication method for a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey {
        key: String,
    },
    #[serde(rename_all = "camelCase")]
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
        scopes: Vec<String>,
    },
}

/// Configuration for a provider with its authentication method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider: Provider,
    pub auth_method: AuthMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_has_three_variants() {
        // Test that Provider has exactly 3 variants by testing Display for each
        assert_eq!(Provider::Anthropic.to_string(), "anthropic");
        assert_eq!(Provider::OpenAi.to_string(), "openai");
        assert_eq!(Provider::Other.to_string(), "other");
    }

    #[test]
    fn test_provider_from_str_case_insensitive() {
        // Test case-insensitive parsing
        assert_eq!(
            "anthropic".parse::<Provider>().unwrap(),
            Provider::Anthropic
        );
        assert_eq!(
            "ANTHROPIC".parse::<Provider>().unwrap(),
            Provider::Anthropic
        );
        assert_eq!(
            "AnThRoPiC".parse::<Provider>().unwrap(),
            Provider::Anthropic
        );

        assert_eq!("openai".parse::<Provider>().unwrap(), Provider::OpenAi);
        assert_eq!("OPENAI".parse::<Provider>().unwrap(), Provider::OpenAi);
        assert_eq!("OpenAI".parse::<Provider>().unwrap(), Provider::OpenAi);

        assert_eq!("other".parse::<Provider>().unwrap(), Provider::Other);
        assert_eq!("OTHER".parse::<Provider>().unwrap(), Provider::Other);
    }

    #[test]
    fn test_provider_from_str_rejects_codex() {
        // Test that "codex" is not a valid provider
        let result = "codex".parse::<Provider>();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Unknown provider: codex");
    }

    #[test]
    fn test_auth_method_api_key_serde_roundtrip() {
        let auth = AuthMethod::ApiKey {
            key: "test-key-123".to_string(),
        };

        let json = serde_json::to_string(&auth).expect("serialize");
        let deserialized: AuthMethod = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(auth, deserialized);
    }

    #[test]
    fn test_auth_method_oauth_serde_roundtrip_with_camel_case() {
        let auth = AuthMethod::OAuth {
            access_token: "access-123".to_string(),
            refresh_token: Some("refresh-456".to_string()),
            expires_at: Some(1704067200i64),
            scopes: vec!["read".to_string(), "write".to_string()],
        };

        let json = serde_json::to_string(&auth).expect("serialize");

        // Verify camelCase in JSON
        assert!(json.contains("accessToken"));
        assert!(json.contains("refreshToken"));
        assert!(json.contains("expiresAt"));
        assert!(json.contains("scopes"));

        let deserialized: AuthMethod = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(auth, deserialized);
    }

    #[test]
    fn test_provider_config_serde_roundtrip() {
        let config = ProviderConfig {
            provider: Provider::Anthropic,
            auth_method: AuthMethod::ApiKey {
                key: "sk-ant-123".to_string(),
            },
            default_model: Some("claude-sonnet-4-6".to_string()),
            base_url: Some("https://api.anthropic.com".to_string()),
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: ProviderConfig = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(config, deserialized);
    }
}
