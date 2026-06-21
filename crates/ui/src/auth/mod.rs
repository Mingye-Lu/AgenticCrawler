pub mod anthropic;
pub mod builder;
pub mod configure;
pub mod custom;
pub mod group;
pub mod mask;
pub mod openai;

use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::mpsc;

use api::oauth::{parse_oauth_callback_request_target, OAuthCallbackParams};

use self::builder::{build_provider_config, CredGroup, CredInputs};

use crate::app::Provider;

pub use anthropic::default_oauth_config;

/// Load the credentials store, warning to stderr if the file existed but
/// failed to parse so users notice a corrupted credentials file instead of
/// silently getting an empty store and a fresh re-auth prompt.
#[must_use]
pub fn load_credentials_or_warn() -> api::CredentialStore {
    match api::load_credentials() {
        Ok(store) => store,
        Err(err) => {
            eprintln!("warning: failed to load credentials ({err}); starting from an empty store");
            api::CredentialStore::default()
        }
    }
}

/// Bind a TCP listener for the OAuth callback on `preferred_port`.
/// Retries briefly in case the port is stuck in `TIME_WAIT` from a prior
/// session, then returns a clear error if still occupied.
pub fn bind_oauth_listener(preferred_port: u16) -> io::Result<(TcpListener, u16)> {
    for attempt in 0..4u8 {
        match TcpListener::bind(("127.0.0.1", preferred_port)) {
            Ok(listener) => return Ok((listener, preferred_port)),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse && attempt < 3 => {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "Port {preferred_port} is already in use. \
                         A previous auth session may still be running 鈥?\
                         close it or kill the process using the port, then retry."
                    ),
                ));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Result of provider selection 鈥?either a legacy enum variant or a preset provider.
#[derive(Debug, Clone)]
pub enum ProviderChoice {
    Legacy(Provider),
    Preset(api::ProviderPreset),
}

pub fn run_auth_for_provider(provider: Provider) -> Result<(), Box<dyn std::error::Error>> {
    match provider {
        Provider::Anthropic => anthropic::run_auth(),
        Provider::OpenAi => openai::run_auth(),
        Provider::Other => custom::run_auth(),
    }
}

pub fn run_auth_cli(provider: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let choice = match provider {
        Some(p) => resolve_provider_arg(p)?,
        None => prompt_provider_choice()?,
    };
    let label = provider_choice_label(&choice).to_string();
    match choice {
        ProviderChoice::Legacy(target) => run_auth_for_provider(target)?,
        ProviderChoice::Preset(ref preset) => run_preset_auth(preset)?,
    }
    eprintln!("鉁?{label} credentials configured successfully.");
    Ok(())
}

pub fn persist_provider_credentials(
    provider: Provider,
    mut config: api::StoredProviderConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = load_credentials_or_warn();
    let provider_name = provider_label(provider).to_string();
    if config.default_model.is_none() {
        config.default_model = store
            .providers
            .get(&provider_name)
            .and_then(|existing| existing.default_model.clone());
    }
    api::set_provider_config(&mut store, &provider_name, config);
    api::save_credentials(&store)?;
    Ok(())
}

/// Persist credentials for a preset provider (uses preset ID as key).
pub fn persist_preset_credentials(
    preset_id: &str,
    mut config: api::StoredProviderConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = load_credentials_or_warn();
    let key = preset_id.to_string();
    if config.default_model.is_none() {
        config.default_model = store
            .providers
            .get(&key)
            .and_then(|existing| existing.default_model.clone());
    }
    api::set_provider_config(&mut store, &key, config);
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
        "amazon-bedrock",
        build_provider_config(
            CredGroup::Bedrock,
            CredInputs {
                access_key: Some(access_key),
                secret_key: Some(secret_key),
                region: Some(region),
                ..CredInputs::default()
            },
        ),
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
        build_provider_config(
            CredGroup::Azure,
            CredInputs {
                api_key: Some(api_key),
                resource_name: Some(resource_name),
                deployment_name: Some(deployment_name),
                ..CredInputs::default()
            },
        ),
    )
}

/// Generic API key auth flow for preset providers.
pub fn run_preset_auth(preset: &api::ProviderPreset) -> Result<(), Box<dyn std::error::Error>> {
    if preset.id == "copilot" {
        return run_copilot_device_code_auth();
    }
    if preset.id == "amazon-bedrock" {
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
        build_provider_config(
            CredGroup::Simple,
            CredInputs {
                api_key: Some(key),
                base_url: Some(preset.base_url.to_string()),
                ..CredInputs::default()
            },
        ),
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

/// Resolve a provider argument to either a legacy `Provider` or a preset.
///
/// Tries the legacy enum first (anthropic/claude, openai/gpt, other),
/// then falls back to preset lookup by ID.
pub fn resolve_provider_arg(value: &str) -> Result<ProviderChoice, Box<dyn std::error::Error>> {
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
pub fn parse_provider_arg(value: &str) -> Result<Provider, Box<dyn std::error::Error>> {
    match resolve_provider_arg(value)? {
        ProviderChoice::Legacy(p) => Ok(p),
        ProviderChoice::Preset(_) => Ok(Provider::Other),
    }
}

#[must_use]
pub fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenAi => "openai",
        Provider::Other => "other",
    }
}

#[must_use]
pub fn provider_choice_label(choice: &ProviderChoice) -> &str {
    match choice {
        ProviderChoice::Legacy(p) => provider_label(*p),
        ProviderChoice::Preset(preset) => preset.display_name,
    }
}

pub fn interactive_login_prompt(choice: &ProviderChoice) -> Result<(), Box<dyn std::error::Error>> {
    match choice {
        ProviderChoice::Legacy(Provider::Anthropic) => {
            eprint!("No Anthropic credentials found. Log in via OAuth? [Y/n] ");
            io::stderr().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            let answer = answer.trim();
            if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") {
                return Err("authentication required 鈥?run `acrawl auth anthropic`".into());
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
                return Err("authentication required 鈥?run `acrawl auth other`".into());
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
                    format!("authentication required 鈥?run `acrawl auth {}`", preset.id).into(),
                );
            }
            run_preset_auth(preset)
        }
    }
}

pub fn prompt_provider_choice() -> Result<ProviderChoice, Box<dyn std::error::Error>> {
    use api::ProviderCategory;
    use dialoguer::{theme::ColorfulTheme, FuzzySelect};

    fn category_short(cat: ProviderCategory) -> &'static str {
        match cat {
            ProviderCategory::Popular => "Popular",
            ProviderCategory::OssHosting => "Open Source",
            ProviderCategory::Specialized => "Specialized",
            ProviderCategory::Enterprise => "Enterprise",
            ProviderCategory::Gateway => "Gateway",
            ProviderCategory::Other => "Other",
        }
    }

    let all_presets = api::builtin_presets();

    // Build list in the canonical category order
    let category_order: &[ProviderCategory] = &[
        ProviderCategory::Popular,
        ProviderCategory::OssHosting,
        ProviderCategory::Specialized,
        ProviderCategory::Enterprise,
        ProviderCategory::Gateway,
        ProviderCategory::Other,
    ];
    let mut indexed: Vec<api::ProviderPreset> = Vec::new();
    for cat in category_order {
        for p in all_presets.iter().filter(|p| p.category == *cat) {
            indexed.push(*p);
        }
    }

    let col_width = category_order
        .iter()
        .map(|c| category_short(*c).len())
        .max()
        .unwrap_or(0);

    let mut items: Vec<String> = indexed
        .iter()
        .map(|p| {
            format!(
                "{:<width$}  {}",
                category_short(p.category),
                p.display_name,
                width = col_width
            )
        })
        .collect();
    // Let users reach Provider::Other (custom base URL + API key) from the menu.
    items.push(format!(
        "{:<width$}  Custom (enter base URL + API key manually)",
        "Other",
        width = col_width
    ));

    let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select a provider (type to filter, Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    match selection {
        Some(i) if i == indexed.len() => Ok(ProviderChoice::Legacy(Provider::Other)),
        Some(i) => Ok(ProviderChoice::Preset(indexed[i])),
        None => Err("no provider selected".into()),
    }
}

pub fn open_browser(url: &str) -> io::Result<()> {
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

#[allow(clippy::needless_pass_by_value)]
pub(super) fn wait_for_oauth_callback(
    listener: TcpListener,
) -> Result<OAuthCallbackParams, Box<dyn std::error::Error>> {
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
pub fn wait_for_oauth_callback_cancellable(
    listener: TcpListener,
    cancel_rx: mpsc::Receiver<()>,
) -> Result<OAuthCallbackParams, Box<dyn std::error::Error + Send>> {
    listener
        .set_nonblocking(true)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(5);
    // Emit a progress line every 30s so a slow network or IdP doesn't look
    // like a hang. Track when we last printed instead of dividing remaining
    // time, so the first message lands ~30s in rather than at startup.
    let mut last_status = std::time::Instant::now();
    let status_interval = std::time::Duration::from_secs(30);
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                "OAuth callback timed out after 5 minutes",
            )));
        }
        if now.duration_since(last_status) >= status_interval {
            let remaining = deadline.saturating_duration_since(now);
            eprintln!(
                "Waiting for OAuth callback ({}s remaining; press Ctrl+C to cancel)...",
                remaining.as_secs()
            );
            last_status = now;
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
            // Every preset has some category 鈥?verify it compiles and returns a value
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
