//! Codex provider — OpenAI Chat Completions with OAuth PKCE authentication.
//!
//! [`resolve_codex_auth`] tries sources in order: stored OAuth credentials,
//! then `OPENAI_API_KEY`. Both produce [`AuthSource::BearerToken`] because
//! OpenAI uses `Authorization: Bearer <token>` for all auth methods.

use runtime::{
    clear_oauth_credentials, generate_pkce_pair, generate_state, load_oauth_credentials,
    save_oauth_credentials, OAuthAuthorizationRequest, OAuthConfig, PkceCodePair,
};

use crate::client::{oauth_token_is_expired, AuthSource, OAuthTokenSet};
use crate::error::ApiError;

pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_CALLBACK_PORT: u16 = 1455;
pub const CODEX_SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];
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

#[must_use]
pub fn read_codex_model() -> String {
    std::env::var("CODEX_MODEL").unwrap_or_else(|_| DEFAULT_CODEX_MODEL.to_string())
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

pub fn logout() -> Result<(), ApiError> {
    clear_oauth_credentials().map_err(ApiError::from)
}

/// Resolves Codex auth: stored OAuth token > `OPENAI_API_KEY` > error.
pub fn resolve_codex_auth() -> Result<AuthSource, ApiError> {
    if let Ok(Some(token_set)) = load_oauth_credentials() {
        let api_token = OAuthTokenSet {
            access_token: token_set.access_token,
            refresh_token: token_set.refresh_token,
            expires_at: token_set.expires_at,
            scopes: token_set.scopes,
        };
        if !oauth_token_is_expired(&api_token) {
            return Ok(AuthSource::BearerToken(api_token.access_token));
        }
    }

    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            return Ok(AuthSource::BearerToken(key));
        }
    }

    Err(ApiError::Auth(
        "no Codex OAuth credentials or OPENAI_API_KEY found; run `acrawl login` to authenticate"
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use runtime::{
        clear_oauth_credentials, code_challenge_s256, load_oauth_credentials,
        save_oauth_credentials,
    };

    use super::*;
    use crate::error::ApiError;

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
        let _guard = env_lock();
        std::env::remove_var("CODEX_MODEL");
        assert_eq!(read_codex_model(), DEFAULT_CODEX_MODEL);
    }

    #[test]
    fn codex_model_reads_from_env() {
        let _guard = env_lock();
        std::env::set_var("CODEX_MODEL", "codex-large-2025");
        assert_eq!(read_codex_model(), "codex-large-2025");
        std::env::remove_var("CODEX_MODEL");
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
    fn resolve_codex_auth_uses_stored_oauth_token() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "codex-oauth-token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_secs() + 3600),
            scopes: vec!["openid".to_string()],
        })
        .expect("save credentials");

        let auth = resolve_codex_auth().expect("should resolve from OAuth");
        assert_eq!(auth.bearer_token(), Some("codex-oauth-token"));

        clear_oauth_credentials().expect("clear");
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }

    #[test]
    fn resolve_codex_auth_skips_expired_oauth_falls_back_to_api_key() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::set_var("OPENAI_API_KEY", "sk-fallback-key");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "expired-codex-token".to_string(),
            refresh_token: None,
            expires_at: Some(1),
            scopes: Vec::new(),
        })
        .expect("save expired credentials");

        let auth = resolve_codex_auth().expect("should fall back to API key");
        assert_eq!(auth.bearer_token(), Some("sk-fallback-key"));

        clear_oauth_credentials().expect("clear");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }

    #[test]
    fn resolve_codex_auth_uses_api_key_when_no_oauth_stored() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::set_var("OPENAI_API_KEY", "sk-test-key");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        let auth = resolve_codex_auth().expect("should resolve from API key");
        assert_eq!(auth.bearer_token(), Some("sk-test-key"));

        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn resolve_codex_auth_errors_when_no_credentials() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        let error = resolve_codex_auth().expect_err("should error without credentials");
        assert!(matches!(error, ApiError::Auth(ref msg) if msg.contains("acrawl login")));

        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn logout_clears_stored_credentials() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "token-to-clear".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_secs() + 3600),
            scopes: Vec::new(),
        })
        .expect("save credentials");

        logout().expect("logout should succeed");

        let loaded = load_oauth_credentials().expect("load after logout");
        assert!(loaded.is_none());

        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }

    #[test]
    fn save_codex_credentials_persists_token_set() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
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
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }
}
