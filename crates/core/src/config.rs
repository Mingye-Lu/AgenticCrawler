use std::path::PathBuf;

/// OAuth provider configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub callback_port: Option<u16>,
    pub manual_redirect_url: Option<String>,
    pub scopes: Vec<String>,
}

/// Platform-specific home directory.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Get the configuration home directory.
/// Reads `ACRAWL_CONFIG_HOME` env var, falls back to ~/.acrawl/
#[must_use]
pub fn config_home_dir() -> PathBuf {
    if let Ok(custom_home) = std::env::var("ACRAWL_CONFIG_HOME") {
        PathBuf::from(custom_home)
    } else {
        home_dir().join(".acrawl")
    }
}
