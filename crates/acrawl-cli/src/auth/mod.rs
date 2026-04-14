pub(crate) mod anthropic;
pub(crate) mod custom;
pub(crate) mod openai;

use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::mpsc;

use runtime::{clear_oauth_credentials, parse_oauth_callback_request_target};

use super::Provider;

pub(crate) use anthropic::{default_oauth_config, run_login};

/// Result of provider selection — either a legacy enum variant or a preset provider.
#[derive(Debug, Clone)]
pub(crate) enum ProviderChoice {
    Legacy(Provider),
    Preset(api::ProviderPreset),
}

pub(crate) fn run_auth_for_provider(provider: Provider) -> Result<(), Box<dyn std::error::Error>> {
    match provider {
        Provider::Anthropic => anthropic::run_auth(),
        Provider::OpenAi => openai::run_auth(),
        Provider::Other => custom::run_auth(),
    }
}

pub(crate) fn run_auth_cli(provider: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let choice = match provider {
        Some(p) => resolve_provider_arg(p)?,
        None => prompt_provider_choice()?,
    };
    let label = provider_choice_label(&choice).to_string();
    match choice {
        ProviderChoice::Legacy(target) => run_auth_for_provider(target)?,
        ProviderChoice::Preset(ref preset) => run_preset_auth(preset)?,
    }
    eprintln!("✅ {label} credentials configured successfully.");
    Ok(())
}

pub(crate) fn persist_provider_credentials(
    provider: Provider,
    mut config: api::StoredProviderConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = api::load_credentials().unwrap_or_default();
    let provider_name = provider_label(provider).to_string();
    if config.default_model.is_none() {
        config.default_model = store
            .providers
            .get(&provider_name)
            .and_then(|existing| existing.default_model.clone());
    }
    api::set_provider_config(&mut store, &provider_name, config);
    store.active_provider = Some(provider_name);
    api::save_credentials(&store)?;
    Ok(())
}

/// Persist credentials for a preset provider (uses preset ID as key).
pub(crate) fn persist_preset_credentials(
    preset_id: &str,
    mut config: api::StoredProviderConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = api::load_credentials().unwrap_or_default();
    let key = preset_id.to_string();
    if config.default_model.is_none() {
        config.default_model = store
            .providers
            .get(&key)
            .and_then(|existing| existing.default_model.clone());
    }
    api::set_provider_config(&mut store, &key, config);
    store.active_provider = Some(key);
    api::save_credentials(&store)?;
    Ok(())
}

/// Generic API key auth flow for preset providers.
pub(crate) fn run_preset_auth(
    preset: &api::ProviderPreset,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(env_var) = preset.api_key_env_var {
        eprintln!("(Hint: also readable from {env_var} env var)");
    }
    eprint!("Enter {} API key: ", preset.display_name);
    io::stderr().flush()?;
    let mut key = String::new();
    io::stdin().read_line(&mut key)?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(format!("API key is required for {}", preset.display_name).into());
    }
    persist_preset_credentials(
        preset.id,
        api::StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some(key),
            base_url: Some(preset.base_url.to_string()),
            ..Default::default()
        },
    )
}

pub(crate) fn run_logout() -> Result<(), Box<dyn std::error::Error>> {
    clear_oauth_credentials()?;
    println!("OAuth credentials cleared.");
    Ok(())
}

/// Resolve a provider argument to either a legacy `Provider` or a preset.
///
/// Tries the legacy enum first (anthropic/claude, openai/gpt, other),
/// then falls back to preset lookup by ID.
pub(crate) fn resolve_provider_arg(
    value: &str,
) -> Result<ProviderChoice, Box<dyn std::error::Error>> {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "anthropic" | "claude" => Ok(ProviderChoice::Legacy(Provider::Anthropic)),
        "openai" | "gpt" => Ok(ProviderChoice::Legacy(Provider::OpenAi)),
        "other" => Ok(ProviderChoice::Legacy(Provider::Other)),
        _ => {
            if let Some(preset) = api::find_preset(&lower) {
                Ok(ProviderChoice::Preset(preset.clone()))
            } else {
                Err(format!(
                    "unknown provider '{value}'. Use 'acrawl auth' to see available providers."
                )
                .into())
            }
        }
    }
}

/// Parse a provider argument into the legacy `Provider` enum.
///
/// Accepts all preset provider IDs in addition to the legacy aliases.
/// Preset providers are mapped to `Provider::Other`.
pub(crate) fn parse_provider_arg(value: &str) -> Result<Provider, Box<dyn std::error::Error>> {
    match resolve_provider_arg(value)? {
        ProviderChoice::Legacy(p) => Ok(p),
        ProviderChoice::Preset(_) => Ok(Provider::Other),
    }
}

pub(crate) fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenAi => "openai",
        Provider::Other => "other",
    }
}

pub(crate) fn provider_choice_label(choice: &ProviderChoice) -> &str {
    match choice {
        ProviderChoice::Legacy(p) => provider_label(*p),
        ProviderChoice::Preset(preset) => preset.display_name,
    }
}

pub(crate) fn interactive_login_prompt(
    choice: &ProviderChoice,
) -> Result<(), Box<dyn std::error::Error>> {
    match choice {
        ProviderChoice::Legacy(Provider::Anthropic) => {
            eprint!("No Anthropic credentials found. Log in via OAuth? [Y/n] ");
            io::stderr().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            let answer = answer.trim();
            if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") {
                return Err("authentication required — run `acrawl auth anthropic`".into());
            }
            run_auth_for_provider(Provider::Anthropic)
        }
        ProviderChoice::Legacy(Provider::OpenAi) => {
            eprintln!("No OpenAI credentials found.");
            run_auth_for_provider(Provider::OpenAi)
        }
        ProviderChoice::Legacy(Provider::Other) => {
            eprint!("No Other provider credentials found. Configure now? [Y/n] ");
            io::stderr().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            let answer = answer.trim();
            if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") {
                return Err("authentication required — run `acrawl auth other`".into());
            }
            run_auth_for_provider(Provider::Other)
        }
        ProviderChoice::Preset(preset) => {
            eprint!(
                "No {} credentials found. Configure now? [Y/n] ",
                preset.display_name
            );
            io::stderr().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            let answer = answer.trim();
            if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") {
                return Err(
                    format!("authentication required — run `acrawl auth {}`", preset.id).into(),
                );
            }
            run_preset_auth(preset)
        }
    }
}

pub(crate) fn prompt_provider_choice() -> Result<ProviderChoice, Box<dyn std::error::Error>> {
    eprintln!("Select a provider to authenticate:");
    eprintln!("  1) Anthropic (OAuth)");
    eprintln!("  2) OpenAI   (API key)");
    eprintln!("  3) Other    (local/OpenAI-compatible)");

    let extra_presets: Vec<_> = api::builtin_presets()
        .into_iter()
        .filter(|p| !matches!(p.id, "anthropic" | "openai" | "other"))
        .collect();

    for (i, preset) in extra_presets.iter().enumerate() {
        eprintln!("  {}) {} (API key)", i + 4, preset.display_name);
    }

    let max = 3 + extra_presets.len();
    eprint!("Choice [1-{max}]: ");
    io::stderr().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let trimmed = choice.trim();

    match trimmed {
        "1" | "anthropic" => Ok(ProviderChoice::Legacy(Provider::Anthropic)),
        "2" | "openai" => Ok(ProviderChoice::Legacy(Provider::OpenAi)),
        "3" | "other" => Ok(ProviderChoice::Legacy(Provider::Other)),
        other => {
            if let Ok(n) = other.parse::<usize>() {
                if n >= 4 && n <= max {
                    return Ok(ProviderChoice::Preset(extra_presets[n - 4].clone()));
                }
            }
            if let Some(preset) = extra_presets.iter().find(|p| p.id == other) {
                return Ok(ProviderChoice::Preset(preset.clone()));
            }
            Err(format!("invalid choice '{other}'").into())
        }
    }
}

pub(crate) fn open_browser(url: &str) -> io::Result<()> {
    let escaped;
    let commands = if cfg!(target_os = "macos") {
        vec![("open", vec![url])]
    } else if cfg!(target_os = "windows") {
        escaped = url.replace('&', "^&");
        vec![("cmd", vec!["/C", "start", "", &escaped])]
    } else {
        vec![("xdg-open", vec![url])]
    };
    for (program, args) in commands {
        match Command::new(program).args(args).spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no supported browser opener command found",
    ))
}

pub(super) fn wait_for_oauth_callback(
    port: u16,
) -> Result<runtime::OAuthCallbackParams, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let (mut stream, _) = listener.accept()?;
    let mut buffer = [0_u8; 4096];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let request_line = request.lines().next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing callback request line")
    })?;
    let target = request_line.split_whitespace().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing callback request target",
        )
    })?;
    let callback = parse_oauth_callback_request_target(target)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let body = if callback.error.is_some() {
        "OAuth login failed. You can close this window."
    } else {
        "OAuth login succeeded. You can close this window."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    Ok(callback)
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn wait_for_oauth_callback_cancellable(
    port: u16,
    cancel_rx: mpsc::Receiver<()>,
) -> Result<runtime::OAuthCallbackParams, Box<dyn std::error::Error + Send>> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
    listener
        .set_nonblocking(true)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                "OAuth callback timed out after 5 minutes",
            )));
        }
        if cancel_rx.try_recv().is_ok() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::Interrupted,
                "OAuth cancelled by user",
            )));
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0_u8; 4096];
                let bytes_read = stream
                    .read(&mut buffer)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
                let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                let request_line = request.lines().next().ok_or_else(|| {
                    Box::new(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "missing callback request line",
                    )) as Box<dyn std::error::Error + Send>
                })?;
                let target = request_line.split_whitespace().nth(1).ok_or_else(|| {
                    Box::new(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "missing callback request target",
                    )) as Box<dyn std::error::Error + Send>
                })?;
                let callback = parse_oauth_callback_request_target(target).map_err(|error| {
                    Box::new(io::Error::new(io::ErrorKind::InvalidData, error))
                        as Box<dyn std::error::Error + Send>
                })?;
                let body = if callback.error.is_some() {
                    "OAuth login failed. You can close this window."
                } else {
                    "OAuth login succeeded. You can close this window."
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
                return Ok(callback);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(Box::new(e) as Box<dyn std::error::Error + Send>),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_provider_arg_anthropic() {
        let result = parse_provider_arg("anthropic");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Provider::Anthropic));
    }

    #[test]
    fn test_parse_provider_arg_openai() {
        let result = parse_provider_arg("openai");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Provider::OpenAi));
    }

    #[test]
    fn test_auth_other_still_works() {
        let result = parse_provider_arg("other");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Provider::Other));
    }

    #[test]
    fn test_parse_provider_arg_groq() {
        assert!(api::find_preset("groq").is_none());
        let result = resolve_provider_arg("groq");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown provider"),
            "expected 'unknown provider' in error, got: {err_msg}"
        );
    }

    #[test]
    fn test_parse_provider_arg_mistral() {
        assert!(api::find_preset("mistral").is_none());
        let result = resolve_provider_arg("mistral");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_provider_arg_legacy_aliases() {
        assert!(matches!(
            resolve_provider_arg("claude").unwrap(),
            ProviderChoice::Legacy(Provider::Anthropic)
        ));
        assert!(matches!(
            resolve_provider_arg("gpt").unwrap(),
            ProviderChoice::Legacy(Provider::OpenAi)
        ));
    }

    #[test]
    fn test_provider_choice_label_legacy() {
        let choice = ProviderChoice::Legacy(Provider::Anthropic);
        assert_eq!(provider_choice_label(&choice), "anthropic");
    }

    #[test]
    fn test_provider_choice_label_preset() {
        let preset = api::find_preset("openai").expect("openai preset exists");
        let choice = ProviderChoice::Preset(preset.clone());
        assert_eq!(provider_choice_label(&choice), "OpenAI");
    }
}
