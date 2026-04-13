use std::io::{self, Write};

use api::{AnthropicClient, AuthSource};
use runtime::OAuthTokenExchangeRequest;

use super::{open_browser, persist_provider_credentials, wait_for_oauth_callback, Provider};

pub(super) fn run_auth() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("OpenAI authentication:");
    eprintln!("  1) API key  (sk-...)");
    eprintln!("  2) OAuth    (PKCE browser flow)");
    eprint!("Choice [1/2]: ");
    io::stderr().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim() {
        "2" | "oauth" => run_openai_login()?,
        _ => {
            eprint!("Paste your OpenAI API key (sk-...): ");
            io::stderr().flush()?;
            let mut key = String::new();
            io::stdin().read_line(&mut key)?;
            let key = key.trim().to_string();
            if key.is_empty() {
                return Err("API key is required for OpenAI".into());
            }
            persist_provider_credentials(
                Provider::OpenAi,
                api::StoredProviderConfig {
                    auth_method: "openai_key".to_string(),
                    api_key: Some(key),
                    ..Default::default()
                },
            )?;
        }
    }
    Ok(())
}

pub(super) fn run_openai_login() -> Result<(), Box<dyn std::error::Error>> {
    let login_req = api::codex_login()?;
    let port = login_req
        .config
        .callback_port
        .unwrap_or(api::CODEX_CALLBACK_PORT);
    println!("Starting OpenAI OAuth login...");
    println!("Listening for callback on {}", login_req.redirect_uri);
    if let Err(error) = open_browser(&login_req.authorization_url) {
        eprintln!("warning: failed to open browser automatically: {error}");
        println!("Open this URL manually:\n{}", login_req.authorization_url);
    }
    let callback = wait_for_oauth_callback(port)?;
    if let Some(error) = callback.error {
        let description = callback
            .error_description
            .unwrap_or_else(|| "authorization failed".to_string());
        return Err(io::Error::other(format!("{error}: {description}")).into());
    }
    let code = callback
        .code
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "callback missing code"))?;
    let returned_state = callback
        .state
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "callback missing state"))?;
    if returned_state != login_req.state {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "oauth state mismatch").into());
    }
    let client = AnthropicClient::from_auth(AuthSource::None);
    let exchange_request = OAuthTokenExchangeRequest::from_config(
        &login_req.config,
        code,
        login_req.state,
        login_req.pkce.verifier,
        login_req.redirect_uri,
    );
    let rt = tokio::runtime::Runtime::new()?;
    let token_set =
        rt.block_on(client.exchange_oauth_code(&login_req.config, &exchange_request))?;
    persist_provider_credentials(
        Provider::OpenAi,
        api::StoredProviderConfig {
            auth_method: "oauth".to_string(),
            oauth: Some(api::StoredOAuthTokens {
                access_token: token_set.access_token,
                refresh_token: token_set.refresh_token,
                expires_at: token_set.expires_at.and_then(|v| i64::try_from(v).ok()),
                scopes: token_set.scopes,
                account_id: None,
            }),
            ..Default::default()
        },
    )?;
    println!("OpenAI OAuth login complete.");
    Ok(())
}
