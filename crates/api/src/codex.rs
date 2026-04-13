//! Codex OAuth PKCE helpers.

use runtime::{
    generate_pkce_pair, generate_state, save_oauth_credentials, OAuthAuthorizationRequest,
    OAuthConfig, PkceCodePair,
};

use crate::error::ApiError;

pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_CALLBACK_PORT: u16 = 1455;
pub const CODEX_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "model.request",
    "api.model.read",
    "api.responses.write",
];
pub const DEFAULT_CODEX_MODEL: &str = "codex-mini-latest";

#[must_use]
pub fn codex_oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: OPENAI_CLIENT_ID.to_string(),
        authorize_url: OPENAI_AUTH_URL.to_string(),
        token_url: OPENAI_TOKEN_URL.to_string(),
        callback_port: Some(CODEX_CALLBACK_PORT),
        manual_redirect_url: None,
        scopes: CODEX_SCOPES.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Loopback redirect URI with `/auth/callback` path (matches Python implementation).
#[must_use]
pub fn codex_redirect_uri() -> String {
    format!("http://localhost:{CODEX_CALLBACK_PORT}/auth/callback")
}

#[derive(Debug)]
pub struct CodexLoginRequest {
    pub authorization_url: String,
    pub pkce: PkceCodePair,
    pub state: String,
    pub config: OAuthConfig,
    pub redirect_uri: String,
}

/// Initiates the Codex OAuth PKCE login flow, returning the authorization URL
/// and PKCE artifacts needed for the token exchange after user approval.
pub fn login() -> Result<CodexLoginRequest, ApiError> {
    let config = codex_oauth_config();
    let pkce = generate_pkce_pair().map_err(ApiError::from)?;
    let state = generate_state().map_err(ApiError::from)?;
    let redirect_uri = codex_redirect_uri();

    let auth_request =
        OAuthAuthorizationRequest::from_config(&config, &redirect_uri, &state, &pkce)
            .with_extra_param("id_token_add_organizations", "true")
            .with_extra_param("codex_cli_simplified_flow", "true");

    Ok(CodexLoginRequest {
        authorization_url: auth_request.build_url(),
        pkce,
        state,
        config,
        redirect_uri,
    })
}

pub fn save_codex_credentials(token_set: &runtime::OAuthTokenSet) -> Result<(), ApiError> {
    save_oauth_credentials(token_set).map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use runtime::{clear_oauth_credentials, code_challenge_s256, load_oauth_credentials};

    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn temp_config_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "codex-api-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_secs()
    }

    #[test]
    fn codex_oauth_config_has_correct_endpoints() {
        let config = codex_oauth_config();
        assert_eq!(config.client_id, OPENAI_CLIENT_ID);
        assert_eq!(config.authorize_url, OPENAI_AUTH_URL);
        assert_eq!(config.token_url, OPENAI_TOKEN_URL);
        assert_eq!(config.callback_port, Some(CODEX_CALLBACK_PORT));
        assert_eq!(
            config.scopes,
            CODEX_SCOPES
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn codex_redirect_uri_matches_expected_format() {
        assert_eq!(codex_redirect_uri(), "http://localhost:1455/auth/callback");
    }

    #[test]
    fn pkce_s256_challenge_matches_rfc7636_vector() {
        let challenge = code_challenge_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn pkce_challenge_is_url_safe_and_unpadded() {
        let challenge = code_challenge_s256("test-verifier-string-for-codex");
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
        assert!(!challenge.contains('='));
        assert!(!challenge.is_empty());
    }

    #[test]
    fn default_codex_model_is_codex_mini_latest() {
        assert_eq!(DEFAULT_CODEX_MODEL, "codex-mini-latest");
    }

    #[test]
    #[cfg(unix)]
    fn login_produces_valid_authorization_url() {
        let request = login().expect("login should produce a request");
        assert!(request
            .authorization_url
            .starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(request.authorization_url.contains("response_type=code"));
        assert!(request
            .authorization_url
            .contains(&format!("client_id={OPENAI_CLIENT_ID}")));
        assert!(request
            .authorization_url
            .contains("code_challenge_method=S256"));
        assert!(request
            .authorization_url
            .contains("codex_cli_simplified_flow=true"));
        assert!(!request.pkce.verifier.is_empty());
        assert!(!request.pkce.challenge.is_empty());
        assert!(!request.state.is_empty());
        assert_eq!(request.redirect_uri, codex_redirect_uri());
    }

    #[test]
    fn save_codex_credentials_persists_token_set() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ACRAWL_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        let token_set = runtime::OAuthTokenSet {
            access_token: "new-codex-token".to_string(),
            refresh_token: Some("new-refresh".to_string()),
            expires_at: Some(now_secs() + 7200),
            scopes: vec!["openid".to_string(), "offline_access".to_string()],
        };
        save_codex_credentials(&token_set).expect("save codex credentials");

        let loaded = load_oauth_credentials()
            .expect("load credentials")
            .expect("token set present");
        assert_eq!(loaded.access_token, "new-codex-token");
        assert_eq!(loaded.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(
            loaded.scopes,
            vec!["openid".to_string(), "offline_access".to_string()]
        );

        clear_oauth_credentials().expect("clear");
        std::env::remove_var("ACRAWL_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }
}
