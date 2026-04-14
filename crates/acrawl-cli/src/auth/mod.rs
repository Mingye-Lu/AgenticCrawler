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

/// Specialized auth flow for AWS Bedrock (access key + secret + region).
fn run_bedrock_auth() -> Result<(), Box<dyn std::error::Error>> {
    eprint!("AWS Access Key ID: ");
    io::stderr().flush()?;
    let mut access_key = String::new();
    io::stdin().read_line(&mut access_key)?;
    let access_key = access_key.trim().to_string();
    if access_key.is_empty() {
        return Err("AWS Access Key ID is required".into());
    }

    eprint!("AWS Secret Access Key: ");
    io::stderr().flush()?;
    let mut secret_key = String::new();
    io::stdin().read_line(&mut secret_key)?;
    let secret_key = secret_key.trim().to_string();
    if secret_key.is_empty() {
        return Err("AWS Secret Access Key is required".into());
    }

    eprint!("AWS Region [us-east-1]: ");
    io::stderr().flush()?;
    let mut region = String::new();
    io::stdin().read_line(&mut region)?;
    let region = region.trim().to_string();
    let region = if region.is_empty() {
        "us-east-1".to_string()
    } else {
        region
    };

    persist_preset_credentials(
        "bedrock",
        api::StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some(access_key),
            aws_secret_access_key: Some(secret_key),
            region: Some(region),
            ..Default::default()
        },
    )
}

/// Specialized auth flow for Azure `OpenAI` (resource name + deployment + API key).
fn run_azure_auth() -> Result<(), Box<dyn std::error::Error>> {
    eprint!("Azure Resource Name (e.g. myresource): ");
    io::stderr().flush()?;
    let mut resource_name = String::new();
    io::stdin().read_line(&mut resource_name)?;
    let resource_name = resource_name.trim().to_string();
    if resource_name.is_empty() {
        return Err("Azure Resource Name is required".into());
    }

    eprint!("Deployment Name (e.g. gpt-4o): ");
    io::stderr().flush()?;
    let mut deployment_name = String::new();
    io::stdin().read_line(&mut deployment_name)?;
    let deployment_name = deployment_name.trim().to_string();
    if deployment_name.is_empty() {
        return Err("Deployment Name is required".into());
    }

    eprint!("Azure API Key: ");
    io::stderr().flush()?;
    let mut api_key = String::new();
    io::stdin().read_line(&mut api_key)?;
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("Azure API Key is required".into());
    }

    persist_preset_credentials(
        "azure",
        api::StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some(api_key),
            resource_name: Some(resource_name),
            deployment_name: Some(deployment_name),
            ..Default::default()
        },
    )
}

/// Generic API key auth flow for preset providers.
pub(crate) fn run_preset_auth(
    preset: &api::ProviderPreset,
) -> Result<(), Box<dyn std::error::Error>> {
    if preset.id == "copilot" {
        return run_copilot_device_code_auth();
    }
    if preset.id == "bedrock" {
        return run_bedrock_auth();
    }
    if preset.id == "azure" {
        return run_azure_auth();
    }
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

fn run_copilot_device_code_auth() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Handle::try_current()
        .map_or_else(|_| tokio::runtime::Runtime::new().map(Some), |_| Ok(None))?;

    let run = async {
        eprintln!("To authenticate with GitHub Copilot:");
        let (device, poll_future) = api::copilot::run_device_code_flow()
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
        eprintln!(
            "Visit {} and enter code: {}",
            device.verification_uri, device.user_code
        );

        if let Err(e) = open_browser(&device.verification_uri) {
            eprintln!("(Could not open browser: {e})");
        }

        eprintln!("Waiting for authorization…");
        let result = poll_future
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;

        persist_preset_credentials(
            "copilot",
            api::StoredProviderConfig {
                auth_method: "oauth".to_string(),
                oauth: Some(api::StoredOAuthTokens {
                    access_token: result.copilot_token,
                    refresh_token: Some(result.github_token),
                    ..Default::default()
                }),
                base_url: Some("https://api.githubcopilot.com".to_string()),
                ..Default::default()
            },
        )?;

        Ok::<(), Box<dyn std::error::Error>>(())
    };

    if let Some(runtime) = rt {
        runtime.block_on(run)
    } else {
        tokio::runtime::Handle::current().block_on(run)
    }
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
                Ok(ProviderChoice::Preset(*preset))
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
    use api::ProviderCategory;

    let all_presets = api::builtin_presets();
    let mut counter = 1_usize;
    let mut indexed: Vec<api::ProviderPreset> = Vec::new();

    let categories: &[(ProviderCategory, &str)] = &[
        (ProviderCategory::Popular, "=== Popular ==="),
        (ProviderCategory::OssHosting, "=== Open Source Hosting ==="),
        (ProviderCategory::Specialized, "=== Specialized ==="),
        (ProviderCategory::Enterprise, "=== Enterprise ==="),
        (ProviderCategory::Gateway, "=== Routing/Gateway ==="),
        (ProviderCategory::Other, "=== Other ==="),
    ];

    eprintln!("\nSelect a provider to authenticate:");
    for (cat, label) in categories {
        let presets_in_cat: Vec<_> = all_presets.iter().filter(|p| p.category == *cat).collect();
        if presets_in_cat.is_empty() {
            continue;
        }
        eprintln!("\n{label}");
        for p in presets_in_cat {
            eprintln!("  {counter}) {}", p.display_name);
            indexed.push(*p);
            counter += 1;
        }
    }

    eprint!("\nChoice [1-{}]: ", indexed.len());
    io::stderr().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let trimmed = choice.trim();

    if let Ok(n) = trimmed.parse::<usize>() {
        if n >= 1 && n <= indexed.len() {
            let preset = indexed[n - 1];
            return Ok(ProviderChoice::Preset(preset));
        }
    }

    // Try by id
    if let Some(p) = indexed.iter().find(|p| p.id == trimmed) {
        return Ok(ProviderChoice::Preset(*p));
    }

    Err(format!("invalid choice '{trimmed}'").into())
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
        assert!(api::find_preset("groq").is_some());
        let result = resolve_provider_arg("groq");
        assert!(result.is_ok(), "groq should resolve via preset lookup");
        assert!(matches!(result.unwrap(), ProviderChoice::Preset(_)));
    }

    #[test]
    fn test_parse_provider_arg_mistral() {
        assert!(api::find_preset("mistral").is_some());
        let result = resolve_provider_arg("mistral");
        assert!(result.is_ok(), "mistral should resolve via preset lookup");
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
        let choice = ProviderChoice::Preset(*preset);
        assert_eq!(provider_choice_label(&choice), "OpenAI");
    }

    #[test]
    fn test_resolve_copilot_provider() {
        let result = resolve_provider_arg("copilot");
        assert!(result.is_ok(), "copilot should resolve via preset lookup");
        assert!(matches!(result.unwrap(), ProviderChoice::Preset(p) if p.id == "copilot"));
    }

    #[test]
    fn test_all_presets_have_category() {
        let presets = api::builtin_presets();
        assert!(!presets.is_empty(), "should have presets");
        for p in &presets {
            // Every preset has some category — verify it compiles and returns a value
            let _ = format!("{:?}", p.category);
        }
    }

    #[test]
    fn test_prompt_provider_choice_lists_all() {
        // Verify the grouped menu would include every preset:
        // build the same indexed vec the function builds and check count.
        use api::ProviderCategory;
        let all_presets = api::builtin_presets();
        let categories = [
            ProviderCategory::Popular,
            ProviderCategory::OssHosting,
            ProviderCategory::Specialized,
            ProviderCategory::Enterprise,
            ProviderCategory::Gateway,
            ProviderCategory::Other,
        ];
        let mut count = 0_usize;
        for cat in &categories {
            count += all_presets.iter().filter(|p| p.category == *cat).count();
        }
        assert_eq!(
            count,
            all_presets.len(),
            "every preset must belong to exactly one known category"
        );
    }

    #[test]
    fn test_direct_provider_arg_still_works() {
        let result = resolve_provider_arg("groq");
        assert!(
            result.is_ok(),
            "direct groq arg should work: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_direct_provider_arg_anthropic_still_works() {
        let result = resolve_provider_arg("anthropic");
        assert!(result.is_ok());
    }
}
