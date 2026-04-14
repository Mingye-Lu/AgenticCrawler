use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Error type for credential operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// I/O error (file not found, permission denied, etc.)
    Io(String),
    /// JSON parsing or serialization error
    Json { msg: String },
    /// Invalid credential format (e.g., old format)
    InvalidFormat { msg: String },
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::Json { msg } => write!(f, "JSON error: {msg}"),
            Self::InvalidFormat { msg } => write!(f, "Invalid format: {msg}"),
        }
    }
}

impl std::error::Error for CredentialError {}

impl From<io::Error> for CredentialError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// OAuth token information stored in credentials
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoredOAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

/// Provider-specific configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StoredProviderConfig {
    pub auth_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<StoredOAuthTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_secret_access_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcp_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcp_region: Option<String>,
}

/// Multi-provider credential store
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CredentialStore {
    pub active_provider: Option<String>,
    pub providers: HashMap<String, StoredProviderConfig>,
}

/// Get the credentials file path
///
/// Respects `ACRAWL_CONFIG_HOME` environment variable, falls back to `~/.acrawl/credentials.json`
#[must_use]
pub fn credentials_file_path() -> PathBuf {
    if let Ok(config_home) = std::env::var("ACRAWL_CONFIG_HOME") {
        PathBuf::from(config_home).join("credentials.json")
    } else {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".acrawl").join("credentials.json")
    }
}

/// Load credentials from a specific path
///
/// Returns empty `CredentialStore` if file doesn't exist.
/// Returns error if file is corrupt or uses old format.
pub fn load_credentials_from_path(
    path: &std::path::Path,
) -> Result<CredentialStore, CredentialError> {
    // Missing file is not an error - return empty store
    if !path.exists() {
        return Ok(CredentialStore::default());
    }

    let content = std::fs::read_to_string(path).map_err(|e| CredentialError::Io(e.to_string()))?;

    // Try to parse as JSON
    let json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| CredentialError::Json { msg: e.to_string() })?;

    // Check for old format (top-level "oauth" key)
    if json.get("oauth").is_some() && json.get("providers").is_none() {
        return Err(CredentialError::InvalidFormat {
            msg: "Credentials file uses old format. Run `acrawl auth` to reconfigure.".to_string(),
        });
    }

    // Parse as new format
    serde_json::from_value::<CredentialStore>(json)
        .map_err(|e| CredentialError::Json { msg: e.to_string() })
}

/// Load credentials from disk
///
/// Returns empty `CredentialStore` if file doesn't exist.
/// Returns error if file is corrupt or uses old format.
pub fn load_credentials() -> Result<CredentialStore, CredentialError> {
    load_credentials_from_path(&credentials_file_path())
}

/// Save credentials to a specific path
///
/// Creates parent directory if it doesn't exist.
/// Writes atomically (temp file + rename).
pub fn save_credentials_to_path(
    store: &CredentialStore,
    path: &std::path::Path,
) -> Result<(), CredentialError> {
    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CredentialError::Io(e.to_string()))?;
    }

    // Serialize to JSON
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| CredentialError::Json { msg: e.to_string() })?;

    // Write to temp file first
    let temp_path = path.with_extension("json.tmp");
    std::fs::write(&temp_path, json).map_err(|e| CredentialError::Io(e.to_string()))?;

    // Atomic rename
    std::fs::rename(&temp_path, path).map_err(|e| CredentialError::Io(e.to_string()))?;

    Ok(())
}

/// Save credentials to disk
///
/// Creates parent directory if it doesn't exist.
/// Writes atomically (temp file + rename).
pub fn save_credentials(store: &CredentialStore) -> Result<(), CredentialError> {
    save_credentials_to_path(store, &credentials_file_path())
}

/// Set or update a provider's configuration
///
/// Updates only the specified provider, leaving others unchanged.
pub fn set_provider_config(
    store: &mut CredentialStore,
    provider: &str,
    config: StoredProviderConfig,
) {
    store.providers.insert(provider.to_string(), config);
}

/// Remove a provider's configuration
///
/// Removes only the specified provider.
pub fn remove_provider_config(store: &mut CredentialStore, provider: &str) {
    store.providers.remove(provider);
}

/// Get the active provider's configuration
///
/// Returns `None` if no active provider is set or if the active provider doesn't exist.
#[must_use]
pub fn get_active_config(store: &CredentialStore) -> Option<&StoredProviderConfig> {
    store
        .active_provider
        .as_ref()
        .and_then(|provider| store.providers.get(provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn test_temp_dir(test_name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("acrawl-test-{test_name}-{counter}"))
    }

    #[test]
    fn test_save_and_load_round_trip() {
        let temp_dir = test_temp_dir("round_trip");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let cred_path = temp_dir.join("credentials.json");

        let mut store = CredentialStore {
            active_provider: Some("anthropic".to_string()),
            providers: HashMap::new(),
        };

        let anthropic_config = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some("sk-ant-test123".to_string()),
            default_model: Some("claude-sonnet-4-6".to_string()),
            ..Default::default()
        };

        let openai_config = StoredProviderConfig {
            auth_method: "oauth".to_string(),
            oauth: Some(StoredOAuthTokens {
                access_token: "access_token_123".to_string(),
                refresh_token: Some("refresh_token_456".to_string()),
                expires_at: Some(1_234_567_890),
                scopes: vec!["read".to_string(), "write".to_string()],
                account_id: None,
            }),
            ..Default::default()
        };

        let other_config = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            base_url: Some("http://localhost:11434/v1".to_string()),
            api_key: Some(String::new()),
            default_model: Some("llama3.2".to_string()),
            ..Default::default()
        };

        store
            .providers
            .insert("anthropic".to_string(), anthropic_config);
        store.providers.insert("openai".to_string(), openai_config);
        store.providers.insert("other".to_string(), other_config);

        save_credentials_to_path(&store, &cred_path).unwrap();

        let loaded = load_credentials_from_path(&cred_path).unwrap();
        assert_eq!(loaded, store);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_missing_file_returns_empty_store() {
        let temp_dir = test_temp_dir("missing");
        let _ = fs::remove_dir_all(&temp_dir);

        let cred_path = temp_dir.join("credentials.json");

        let store = load_credentials_from_path(&cred_path).unwrap();
        assert_eq!(store, CredentialStore::default());
    }

    #[test]
    fn test_set_provider_config_updates_one_provider() {
        let mut store = CredentialStore::default();

        let config1 = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some("key1".to_string()),
            ..Default::default()
        };

        let config2 = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some("key2".to_string()),
            ..Default::default()
        };

        set_provider_config(&mut store, "provider1", config1.clone());
        set_provider_config(&mut store, "provider2", config2.clone());

        assert_eq!(store.providers.get("provider1"), Some(&config1));
        assert_eq!(store.providers.get("provider2"), Some(&config2));

        let config1_updated = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some("key1_updated".to_string()),
            ..Default::default()
        };

        set_provider_config(&mut store, "provider1", config1_updated.clone());

        assert_eq!(store.providers.get("provider1"), Some(&config1_updated));
        assert_eq!(store.providers.get("provider2"), Some(&config2));
    }

    #[test]
    fn test_remove_provider_config_removes_only_specified() {
        let mut store = CredentialStore::default();

        let config1 = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some("key1".to_string()),
            ..Default::default()
        };

        let config2 = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some("key2".to_string()),
            ..Default::default()
        };

        set_provider_config(&mut store, "provider1", config1);
        set_provider_config(&mut store, "provider2", config2);

        remove_provider_config(&mut store, "provider1");

        assert!(!store.providers.contains_key("provider1"));
        assert!(store.providers.contains_key("provider2"));
    }

    #[test]
    fn test_get_active_config_returns_correct_provider() {
        let store = CredentialStore {
            active_provider: Some("anthropic".to_string()),
            providers: {
                let mut map = HashMap::new();
                map.insert(
                    "anthropic".to_string(),
                    StoredProviderConfig {
                        auth_method: "api_key".to_string(),
                        api_key: Some("sk-ant-test".to_string()),
                        ..Default::default()
                    },
                );
                map
            },
        };

        let config = &store.providers["anthropic"];
        assert_eq!(get_active_config(&store), Some(config));
    }

    #[test]
    fn test_corrupt_json_returns_error() {
        let temp_dir = test_temp_dir("corrupt");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let cred_path = temp_dir.join("credentials.json");
        fs::write(&cred_path, "{ invalid json }").unwrap();

        let result = load_credentials_from_path(&cred_path);
        assert!(matches!(result, Err(CredentialError::Json { .. })));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_old_format_oauth_key_returns_error() {
        let temp_dir = test_temp_dir("old_format");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let cred_path = temp_dir.join("credentials.json");
        let old_format = r#"{"oauth": {"accessToken": "token123"}}"#;
        fs::write(&cred_path, old_format).unwrap();

        let result = load_credentials_from_path(&cred_path);
        assert!(
            matches!(result, Err(CredentialError::InvalidFormat { msg }) if msg.contains("acrawl auth"))
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_credentials_file_path_respects_env_var() {
        let _lock = crate::test_env_lock();
        let custom_path = "/custom/config/path";
        std::env::set_var("ACRAWL_CONFIG_HOME", custom_path);

        let path = credentials_file_path();
        assert_eq!(path, PathBuf::from(custom_path).join("credentials.json"));
    }

    #[test]
    fn test_get_active_config_returns_none_when_no_active_provider() {
        let store = CredentialStore::default();
        assert_eq!(get_active_config(&store), None);
    }

    #[test]
    fn test_get_active_config_returns_none_when_provider_not_found() {
        let store = CredentialStore {
            active_provider: Some("nonexistent".to_string()),
            providers: HashMap::new(),
        };

        assert_eq!(get_active_config(&store), None);
    }
}
