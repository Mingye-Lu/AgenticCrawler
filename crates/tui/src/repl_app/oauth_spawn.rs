use super::*;

pub(super) fn extract_openai_account_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .or_else(|| {
            claims
                .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            claims
                .pointer("/organizations/0/id")
                .and_then(|v| v.as_str())
        })
        .map(String::from)
}

pub(super) fn spawn_extension_connection_watch(
    cli: &Arc<Mutex<LiveCli>>,
    ui_tx: &mpsc::Sender<ReplTuiEvent>,
) {
    let connection_watch = {
        let g = cli.lock().expect("cli lock");
        g.extension_connection_watch()
    };
    let Some(watch) = connection_watch else {
        return;
    };
    spawn_extension_connection_watch_from_receiver(watch, cli, ui_tx);
}

pub(super) fn spawn_extension_connection_watch_from_receiver(
    mut connection_watch: tokio::sync::watch::Receiver<bool>,
    cli: &Arc<Mutex<LiveCli>>,
    ui_tx: &mpsc::Sender<ReplTuiEvent>,
) {
    let cli_clone = cli.clone();
    let ui_tx_clone = ui_tx.clone();
    std::thread::spawn(move || {
        let rt = crate::TOKIO_RUNTIME.get().expect("tokio runtime");
        let connected = rt.block_on(async {
            if *connection_watch.borrow() {
                true
            } else {
                connection_watch.changed().await.is_ok() && *connection_watch.borrow()
            }
        });
        if connected {
            let setup = {
                let mut g = cli_clone.lock().expect("cli lock");
                g.prepare_extension_bridge_activation()
            };
            let result = match setup {
                Ok((shared, saved_state)) => {
                    let init_result = rt.block_on(async {
                        prime_extension_bridge(&shared, saved_state.as_ref()).await
                    });
                    match init_result {
                        Ok(()) => {
                            let mut g = cli_clone.lock().expect("cli lock");
                            g.activate_extension_bridge(shared);
                            Ok(())
                        }
                        Err(error) => {
                            let mut g = cli_clone.lock().expect("cli lock");
                            g.restore_pending_extension_state(saved_state);
                            Err(error)
                        }
                    }
                }
                Err(error) => Err(error),
            };
            let _ = ui_tx_clone.send(ReplTuiEvent::ExtensionBridgeResult {
                success: result.is_ok(),
                message: match result {
                    Ok(()) => "Extension bridge\n  \
                              Result           connected \u{2014} browser commands routed to extension"
                        .to_string(),
                    Err(error) => format!("Extension bridge\n  Error            {error}"),
                },
            });
        }
    });
}

pub(super) async fn prime_extension_bridge(
    shared: &SharedBridge,
    saved_state: Option<&BrowserState>,
) -> Result<(), String> {
    let mut bridge = shared.lock().await;
    if let Some(state) = saved_state {
        bridge
            .new_page(None)
            .await
            .map_err(|error| error.to_string())?;

        bridge
            .import_cookies_only(state)
            .await
            .map_err(|error| error.to_string())?;

        if !state.url.is_empty() && state.url != "about:blank" {
            bridge
                .navigate(&state.url)
                .await
                .map_err(|error| error.to_string())?;
            bridge
                .import_local_storage(state)
                .await
                .map_err(|error| error.to_string())?;
        }
    }

    Ok(())
}

pub(super) fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let standard = input.replace('-', "+").replace('_', "/");
    let padded = match standard.len() % 4 {
        2 => format!("{standard}=="),
        3 => format!("{standard}="),
        _ => standard,
    };
    let table: [u8; 256] = {
        let mut t = [255u8; 256];
        for (i, &c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            .iter()
            .enumerate()
        {
            #[allow(clippy::cast_possible_truncation)]
            {
                t[c as usize] = i as u8;
            }
        }
        t[b'=' as usize] = 0;
        t
    };
    let bytes: Vec<u8> = padded
        .bytes()
        .filter(|&b| b != b'\n' && b != b'\r')
        .collect();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let (a, b, c, d) = (
            table[chunk[0] as usize],
            table[chunk[1] as usize],
            table[chunk[2] as usize],
            table[chunk[3] as usize],
        );
        if a == 255 || b == 255 || c == 255 || d == 255 {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    Some(out)
}

#[allow(clippy::too_many_lines)]
pub(super) fn spawn_anthropic_oauth_thread(
    ui_tx: Sender<ReplTuiEvent>,
    active_modal: &mut Option<ActiveModal>,
) {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    if let Some(modal) = active_modal.as_mut().and_then(ActiveModal::as_auth_mut) {
        if let AuthModalStep::OAuthWaiting {
            cancel_tx: ref mut tx,
            ..
        } = modal.step
        {
            *tx = Some(cancel_tx);
        }
    }
    let ui_tx2 = ui_tx.clone();
    thread::spawn(move || {
        let result: Result<(), Box<dyn std::error::Error + Send>> = (|| {
            use crate::app::{
                bind_oauth_listener, default_oauth_config, open_browser,
                wait_for_oauth_callback_cancellable,
            };
            use api::oauth::{
                generate_pkce_pair, generate_state, loopback_redirect_uri,
                OAuthAuthorizationRequest, OAuthTokenExchangeRequest,
            };
            use api::{AnthropicClient, AuthSource};

            let oauth = default_oauth_config();
            let preferred_port = oauth.callback_port.unwrap_or(4545);
            let (listener, actual_port) = bind_oauth_listener(preferred_port)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let redirect_uri = loopback_redirect_uri(actual_port);
            let pkce = generate_pkce_pair()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let state_val =
                generate_state().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let authorize_url = OAuthAuthorizationRequest::from_config(
                &oauth,
                redirect_uri.clone(),
                state_val.clone(),
                &pkce,
            )
            .build_url();
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: "Opening browser...".to_string(),
            });
            if let Err(err) = open_browser(&authorize_url) {
                let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                    message: format!("Browser failed. Visit: {authorize_url}  ({err})"),
                });
            }
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: format!("Waiting for OAuth callback on port {actual_port}\u{2026}"),
            });
            let callback = wait_for_oauth_callback_cancellable(listener, cancel_rx)?;
            if let Some(error) = callback.error {
                let desc = callback.error_description.unwrap_or_default();
                return Err(Box::new(std::io::Error::other(format!("{error}: {desc}"))) as _);
            }
            let code = callback.code.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing code",
                )) as Box<dyn std::error::Error + Send>
            })?;
            let returned_state = callback.state.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing state",
                )) as Box<dyn std::error::Error + Send>
            })?;
            if returned_state != state_val {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "oauth state mismatch",
                )) as _);
            }
            let client = AnthropicClient::from_auth(AuthSource::None);
            let exchange = OAuthTokenExchangeRequest::from_config(
                &oauth,
                code,
                state_val,
                pkce.verifier,
                redirect_uri,
            );
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let token_set = rt
                .block_on(client.exchange_oauth_code(&oauth, &exchange))
                .map_err(|e| -> Box<dyn std::error::Error + Send> {
                    Box::new(std::io::Error::other(e.to_string()))
                })?;
            let mut store = crate::auth::load_credentials_or_warn();
            api::credentials::set_provider_config(
                &mut store,
                "anthropic",
                api::StoredProviderConfig {
                    auth_method: "oauth".to_string(),
                    oauth: Some(api::StoredOAuthTokens {
                        access_token: token_set.access_token.clone(),
                        refresh_token: token_set.refresh_token.clone(),
                        expires_at: token_set.expires_at.and_then(|v| i64::try_from(v).ok()),
                        scopes: token_set.scopes.clone(),
                        account_id: None,
                    }),
                    ..Default::default()
                },
            );
            api::credentials::save_credentials(&store).map_err(
                |e| -> Box<dyn std::error::Error + Send> {
                    Box::new(std::io::Error::other(e.to_string()))
                },
            )?;
            Ok(())
        })();
        let _ = ui_tx.send(ReplTuiEvent::AuthOAuthComplete {
            provider: "anthropic".to_string(),
            result: result.map_err(|e| e.to_string()),
        });
    });
}

#[allow(clippy::too_many_lines)]
pub(super) fn spawn_openai_oauth_thread(
    ui_tx: Sender<ReplTuiEvent>,
    active_modal: &mut Option<ActiveModal>,
) {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    if let Some(modal) = active_modal.as_mut().and_then(ActiveModal::as_auth_mut) {
        if let AuthModalStep::OAuthWaiting {
            cancel_tx: ref mut tx,
            ..
        } = modal.step
        {
            *tx = Some(cancel_tx);
        }
    }
    let ui_tx2 = ui_tx.clone();
    thread::spawn(move || {
        let result: Result<(), Box<dyn std::error::Error + Send>> = (|| {
            use crate::app::{
                bind_oauth_listener, open_browser, wait_for_oauth_callback_cancellable,
            };
            use api::oauth::OAuthTokenExchangeRequest;
            use api::{AnthropicClient, AuthSource};

            let (listener, actual_port) = bind_oauth_listener(api::CODEX_CALLBACK_PORT)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let login_request = api::codex_login(actual_port).map_err(|e| {
                Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error + Send>
            })?;
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: "Opening browser for OpenAI login...".to_string(),
            });
            if let Err(err) = open_browser(&login_request.authorization_url) {
                let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                    message: format!(
                        "Browser failed. Visit: {}  ({err})",
                        login_request.authorization_url
                    ),
                });
            }
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: format!("Waiting for Codex OAuth callback on port {actual_port}\u{2026}"),
            });
            let callback = wait_for_oauth_callback_cancellable(listener, cancel_rx)?;
            if let Some(error) = callback.error {
                let desc = callback.error_description.unwrap_or_default();
                return Err(Box::new(std::io::Error::other(format!("{error}: {desc}"))) as _);
            }
            let code = callback.code.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing code",
                )) as Box<dyn std::error::Error + Send>
            })?;
            let returned_state = callback.state.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing state",
                )) as Box<dyn std::error::Error + Send>
            })?;
            if returned_state != login_request.state {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "oauth state mismatch",
                )) as _);
            }
            let client = AnthropicClient::from_auth(AuthSource::None);
            let exchange = OAuthTokenExchangeRequest::from_config(
                &login_request.config,
                code,
                login_request.state,
                login_request.pkce.verifier,
                login_request.redirect_uri,
            );
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let token_set = rt
                .block_on(client.exchange_oauth_code(&login_request.config, &exchange))
                .map_err(|e| -> Box<dyn std::error::Error + Send> {
                    Box::new(std::io::Error::other(e.to_string()))
                })?;
            let account_id = extract_openai_account_id(&token_set.access_token);
            let oauth_tokens = api::StoredOAuthTokens {
                access_token: token_set.access_token,
                refresh_token: token_set.refresh_token,
                expires_at: token_set.expires_at.and_then(|v| i64::try_from(v).ok()),
                scopes: token_set.scopes,
                account_id,
            };
            let mut store = crate::auth::load_credentials_or_warn();
            let mut cfg = store.providers.get("openai").cloned().unwrap_or_default();
            cfg.auth_method = "oauth".to_string();
            cfg.oauth = Some(oauth_tokens);
            api::credentials::set_provider_config(&mut store, "openai", cfg);
            api::credentials::save_credentials(&store).map_err(
                |e| -> Box<dyn std::error::Error + Send> {
                    Box::new(std::io::Error::other(e.to_string()))
                },
            )?;
            Ok(())
        })();
        let _ = ui_tx.send(ReplTuiEvent::AuthOAuthComplete {
            provider: "openai".to_string(),
            result: result.map_err(|e| e.to_string()),
        });
    });
}
