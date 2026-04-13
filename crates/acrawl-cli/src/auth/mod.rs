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

pub(crate) fn run_auth_for_provider(provider: Provider) -> Result<(), Box<dyn std::error::Error>> {
    match provider {
        Provider::Anthropic => anthropic::run_auth(),
        Provider::OpenAi => openai::run_auth(),
        Provider::Other => custom::run_auth(),
    }
}

pub(crate) fn run_auth_cli(provider: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let target = match provider {
        Some(p) => parse_provider_arg(p)?,
        None => prompt_provider_choice()?,
    };
    run_auth_for_provider(target)?;
    eprintln!(
        "✅ {} credentials configured successfully.",
        provider_label(target)
    );
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

pub(crate) fn run_logout() -> Result<(), Box<dyn std::error::Error>> {
    clear_oauth_credentials()?;
    println!("OAuth credentials cleared.");
    Ok(())
}

pub(crate) fn parse_provider_arg(value: &str) -> Result<Provider, Box<dyn std::error::Error>> {
    match value.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => Ok(Provider::Anthropic),
        "openai" | "gpt" => Ok(Provider::OpenAi),
        "other" => Ok(Provider::Other),
        other => {
            Err(format!("unknown provider '{other}'. Use anthropic, openai, or other.").into())
        }
    }
}

pub(crate) fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenAi => "openai",
        Provider::Other => "other",
    }
}

pub(crate) fn interactive_login_prompt(
    provider: Provider,
) -> Result<(), Box<dyn std::error::Error>> {
    match provider {
        Provider::Anthropic => {
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
        Provider::OpenAi => {
            eprintln!("No OpenAI credentials found.");
            run_auth_for_provider(Provider::OpenAi)
        }
        Provider::Other => {
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
    }
}

pub(crate) fn prompt_provider_choice() -> Result<Provider, Box<dyn std::error::Error>> {
    eprintln!("Select a provider to authenticate:");
    eprintln!("  1) Anthropic (OAuth)");
    eprintln!("  2) OpenAI   (API key)");
    eprintln!("  3) Other    (local/OpenAI-compatible)");
    eprint!("Choice [1/2/3]: ");
    io::stderr().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim() {
        "1" | "anthropic" => Ok(Provider::Anthropic),
        "2" | "openai" => Ok(Provider::OpenAi),
        "3" | "other" => Ok(Provider::Other),
        other => Err(format!("invalid choice '{other}'").into()),
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

#[allow(dead_code, clippy::needless_pass_by_value)]
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
