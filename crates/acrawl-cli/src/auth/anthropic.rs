use std::env;
use std::io::{self, Write};

use api::{AnthropicClient, AuthSource};
use runtime::{
    generate_pkce_pair, generate_state, load_oauth_credentials, save_oauth_credentials,
    ConfigLoader, OAuthAuthorizationRequest, OAuthConfig, OAuthTokenExchangeRequest,
};

use super::{
    bind_oauth_listener, open_browser, persist_provider_credentials, wait_for_oauth_callback,
    Provider,
};

const DEFAULT_OAUTH_CALLBACK_PORT: u16 = 4545;

pub(super) fn run_auth() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("Anthropic authentication:");
    eprintln!("  1) API key  (sk-ant-...)");
    eprintln!("  2) OAuth    (PKCE browser flow)");
    eprint!("Choice [1/2]: ");
    io::stderr().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim() {
        "2" | "oauth" => {
            run_login()?;
            let oauth = load_oauth_credentials()?
                .ok_or("Anthropic OAuth completed, but no saved token was found")?;
            persist_provider_credentials(
                Provider::Anthropic,
                api::StoredProviderConfig {
                    auth_method: "oauth".to_string(),
                    oauth: Some(api::StoredOAuthTokens {
                        access_token: oauth.access_token,
                        refresh_token: oauth.refresh_token,
                        expires_at: oauth.expires_at.and_then(|v| i64::try_from(v).ok()),
                        scopes: oauth.scopes,
                        account_id: None,
                    }),
                    ..Default::default()
                },
            )?;
        }
        _ => {
            eprint!("Paste your Anthropic API key (sk-ant-...): ");
            io::stderr().flush()?;
            let mut key = String::new();
            io::stdin().read_line(&mut key)?;
            let key = key.trim().to_string();
            if key.is_empty() {
                return Err("API key is required for Anthropic".into());
            }
            persist_provider_credentials(
                Provider::Anthropic,
                api::StoredProviderConfig {
                    auth_method: "api_key".to_string(),
                    api_key: Some(key),
                    ..Default::default()
                },
            )?;
        }
    }
    Ok(())
}

pub(crate) fn run_login() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let config = ConfigLoader::default_for(&cwd).load()?;
    let default_oauth = default_oauth_config();
    let oauth = config.oauth().unwrap_or(&default_oauth);
    let preferred_port = oauth.callback_port.unwrap_or(DEFAULT_OAUTH_CALLBACK_PORT);
    let (listener, actual_port) = bind_oauth_listener(preferred_port)?;
    let redirect_uri = runtime::loopback_redirect_uri(actual_port);
    let pkce = generate_pkce_pair()?;
    let state = generate_state()?;
    let authorize_url =
        OAuthAuthorizationRequest::from_config(oauth, redirect_uri.clone(), state.clone(), &pkce)
            .build_url();
    println!("Starting OAuth login...");
    println!("Listening for callback on {redirect_uri}");
    if let Err(error) = open_browser(&authorize_url) {
        eprintln!("warning: failed to open browser automatically: {error}");
        println!("Open this URL manually:\n{authorize_url}");
    }
    let callback = wait_for_oauth_callback(listener)?;
    if let Some(error) = callback.error {
        let description = callback
            .error_description
            .unwrap_or_else(|| "authorization failed".to_string());
        return Err(io::Error::other(format!("{error}: {description}")).into());
    }
    let code = callback.code.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "callback did not include code")
    })?;
    let returned_state = callback.state.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "callback did not include state")
    })?;
    if returned_state != state {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "oauth state mismatch").into());
    }
    let client = AnthropicClient::from_auth(AuthSource::None);
    let exchange_request =
        OAuthTokenExchangeRequest::from_config(oauth, code, state, pkce.verifier, redirect_uri);
    let rt = tokio::runtime::Runtime::new()?;
    let token_set = rt.block_on(client.exchange_oauth_code(oauth, &exchange_request))?;
    save_oauth_credentials(&runtime::OAuthTokenSet {
        access_token: token_set.access_token,
        refresh_token: token_set.refresh_token,
        expires_at: token_set.expires_at,
        scopes: token_set.scopes,
    })?;
    println!("OAuth login complete.");
    Ok(())
}

pub(crate) fn default_oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: String::from("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
        authorize_url: String::from("https://platform.claude.com/oauth/authorize"),
        token_url: String::from("https://platform.claude.com/v1/oauth/token"),
        callback_port: None,
        manual_redirect_url: None,
        scopes: vec![
            String::from("user:profile"),
            String::from("user:inference"),
            String::from("user:sessions:claude_code"),
        ],
    }
}
