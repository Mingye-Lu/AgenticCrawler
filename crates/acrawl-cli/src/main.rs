mod app;
mod auth;
mod display_width;
mod error;
mod format;
mod markdown;
mod mcp_install;
mod mcp_server;
mod output_sink;
mod self_update;
mod session_mgr;
mod tool_format;
mod tui;
mod uninstall;

use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(crate) static TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

use commands::{render_slash_command_help, resume_supported_slash_commands, SlashCommand};
use crawler::mvp_tool_specs;
use runtime::Session;

use app::{
    initial_model_from_credentials, run_auth_cli, run_repl, run_resume_command, AllowedToolSet,
    LiveCli,
};
use format::{render_version_report, VERSION};

pub(crate) use app::Provider;

fn main() {
    // Load settings.json and set env vars consumed by child processes / the crawler.
    let settings = runtime::load_settings();
    env::set_var(
        "ACRAWL_OUTPUT_DIR",
        runtime::settings_get_output_dir(&settings),
    );
    // Only seed HEADLESS from settings when not already overridden by a parent process.
    if env::var("HEADLESS").is_err() {
        env::set_var(
            "HEADLESS",
            if runtime::settings_get_headless(&settings) {
                "true"
            } else {
                "false"
            },
        );
    }

    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture
        );
        default_panic(info);
    }));

    TOKIO_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime")
    });

    if let Err(error) = run() {
        eprintln!("error: {error}\n\nRun `acrawl --help` for usage.");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    match parse_args(&args)? {
        CliAction::PrintSystemPrompt => print_system_prompt(),
        CliAction::Version => print_version(),
        CliAction::ResumeSession {
            session_path,
            commands,
        } => resume_session(&session_path, &commands),
        CliAction::Prompt {
            prompt,
            model,
            output_format,
            allowed_tools,
        } => {
            LiveCli::new_non_interactive(model, true, allowed_tools)?
                .run_turn_with_output(&prompt, output_format)?;
        }
        CliAction::Auth { provider } => run_auth_cli(provider.as_deref())?,
        CliAction::Update => {
            let rt = TOKIO_RUNTIME.get().expect("tokio runtime not initialized");
            rt.block_on(self_update::run_self_update())?;
        }
        CliAction::Uninstall { purge } => uninstall::run_uninstall(purge)?,
        CliAction::InstallBrowser => install_browser()?,
        CliAction::Mcp => mcp_server::run(),
        CliAction::McpInstall => mcp_install::run()?,
        CliAction::Repl {
            model,
            allowed_tools,
        } => {
            // When model is missing, start REPL and let inline TUI auth onboarding
            // collect provider/model instead of falling back to CLI auth prompts.
            let model = model.unwrap_or_default();
            run_repl(model, allowed_tools)?;
        }
        CliAction::Help => print_help(),
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliAction {
    PrintSystemPrompt,
    Version,
    ResumeSession {
        session_path: PathBuf,
        commands: Vec<String>,
    },
    Prompt {
        prompt: String,
        model: String,
        output_format: CliOutputFormat,
        allowed_tools: Option<AllowedToolSet>,
    },
    Auth {
        provider: Option<String>,
    },
    Update,
    Uninstall {
        purge: bool,
    },
    InstallBrowser,
    Mcp,
    McpInstall,
    Repl {
        model: Option<String>,
        allowed_tools: Option<AllowedToolSet>,
    },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliOutputFormat {
    Text,
    Json,
}

impl CliOutputFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unsupported value for --output-format: {other} (expected text or json)"
            )),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn parse_args(args: &[String]) -> Result<CliAction, String> {
    let mut model = initial_model_from_credentials();
    let mut output_format = CliOutputFormat::Text;
    let mut wants_version = false;
    let mut allowed_tool_values = Vec::new();
    let mut rest = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--version" | "-V" => {
                wants_version = true;
                index += 1;
            }
            "--model" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --model".to_string())?;
                model = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--model=") => {
                model = Some(flag[8..].to_string());
                index += 1;
            }
            "--output-format" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --output-format".to_string())?;
                output_format = CliOutputFormat::parse(value)?;
                index += 2;
            }
            flag if flag.starts_with("--output-format=") => {
                output_format = CliOutputFormat::parse(&flag[16..])?;
                index += 1;
            }
            "--no-headless" | "--headed" => {
                env::set_var("HEADLESS", "false");
                index += 1;
            }
            "--headless" => {
                env::set_var("HEADLESS", "true");
                index += 1;
            }
            flag if flag.starts_with("--headless=") => {
                let value = &flag[11..];
                let normalized = normalize_bool_flag("--headless", value)?;
                env::set_var("HEADLESS", if normalized { "true" } else { "false" });
                index += 1;
            }
            "-p" => {
                let prompt = args[index + 1..].join(" ");
                if prompt.trim().is_empty() {
                    return Err("-p requires a prompt string".to_string());
                }
                let model = model.clone().ok_or_else(|| {
                    "missing model: set --model, set env model vars, or run `acrawl auth` to configure a default model".to_string()
                })?;
                return Ok(CliAction::Prompt {
                    prompt,
                    model: model.clone(),
                    output_format,
                    allowed_tools: normalize_allowed_tools(&allowed_tool_values)?,
                });
            }
            "--print" => {
                output_format = CliOutputFormat::Text;
                index += 1;
            }
            "--allowedTools" | "--allowed-tools" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --allowedTools".to_string())?;
                allowed_tool_values.push(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--allowedTools=") => {
                allowed_tool_values.push(flag[15..].to_string());
                index += 1;
            }
            flag if flag.starts_with("--allowed-tools=") => {
                allowed_tool_values.push(flag[16..].to_string());
                index += 1;
            }
            other => {
                rest.push(other.to_string());
                index += 1;
            }
        }
    }

    if wants_version {
        return Ok(CliAction::Version);
    }

    let allowed_tools = normalize_allowed_tools(&allowed_tool_values)?;

    if rest.is_empty() {
        return Ok(CliAction::Repl {
            model,
            allowed_tools,
        });
    }
    if matches!(rest.first().map(String::as_str), Some("--help" | "-h")) {
        return Ok(CliAction::Help);
    }
    if rest.first().map(String::as_str) == Some("--resume") {
        return parse_resume_args(&rest[1..]);
    }

    match rest[0].as_str() {
        "system-prompt" => parse_system_prompt_args(&rest[1..]),
        "auth" => {
            let provider = rest.get(1).cloned();
            Ok(CliAction::Auth { provider })
        }
        "login" | "logout" => {
            Err(
                "`acrawl login` and `acrawl logout` have been removed. Use `acrawl auth` instead."
                    .to_string(),
            )
        }
        "update" => Ok(CliAction::Update),
        "uninstall" => Ok(CliAction::Uninstall {
            purge: rest.iter().any(|a| a == "--purge"),
        }),
        "install-browser" => Ok(CliAction::InstallBrowser),
        "mcp" => {
            if rest.get(1).map(String::as_str) == Some("install") {
                Ok(CliAction::McpInstall)
            } else {
                Ok(CliAction::Mcp)
            }
        }
        "prompt" => {
            let prompt = rest[1..].join(" ");
            if prompt.trim().is_empty() {
                return Err("prompt subcommand requires a prompt string".to_string());
            }
            let model = model.ok_or_else(|| {
                "missing model: set --model, set env model vars, or run `acrawl auth` to configure a default model".to_string()
            })?;
            Ok(CliAction::Prompt {
                prompt,
                model,
                output_format,
                allowed_tools,
            })
        }
        other if !other.starts_with('/') => Ok(CliAction::Prompt {
            prompt: rest.join(" "),
            model: model.ok_or_else(|| {
                "missing model: set --model, set env model vars, or run `acrawl auth` to configure a default model".to_string()
            })?,
            output_format,
            allowed_tools,
        }),
        other => Err(format!("unknown subcommand: {other}")),
    }
}

fn normalize_allowed_tools(values: &[String]) -> Result<Option<AllowedToolSet>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    let canonical_names = mvp_tool_specs()
        .into_iter()
        .map(|spec| spec.name.to_string())
        .collect::<Vec<_>>();
    let name_map = canonical_names
        .iter()
        .map(|name| (normalize_tool_name(name), name.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut allowed = AllowedToolSet::new();
    for value in values {
        for token in value
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|token| !token.is_empty())
        {
            let normalized = normalize_tool_name(token);
            let canonical = name_map.get(&normalized).ok_or_else(|| {
                format!(
                    "unsupported tool in --allowedTools: {token} (expected one of: {})",
                    canonical_names.join(", ")
                )
            })?;
            allowed.insert(canonical.clone());
        }
    }
    Ok(Some(allowed))
}

fn normalize_tool_name(value: &str) -> String {
    value.trim().replace('-', "_").to_ascii_lowercase()
}

fn normalize_bool_flag(flag: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!(
            "unsupported value for {flag}: {other} (expected true/false)"
        )),
    }
}

fn parse_system_prompt_args(args: &[String]) -> Result<CliAction, String> {
    if let Some(other) = args.first() {
        return Err(format!("unknown system-prompt option: {other}"));
    }
    Ok(CliAction::PrintSystemPrompt)
}

fn parse_resume_args(args: &[String]) -> Result<CliAction, String> {
    let session_path = args
        .first()
        .ok_or_else(|| "missing session path for --resume".to_string())
        .map(PathBuf::from)?;
    let raw_args = &args[1..];
    // Re-join arguments into slash commands, splitting on leading '/' tokens.
    // This allows commands like `/clear --confirm` or `/config env`
    // where arguments don't start with '/'.
    let mut commands = Vec::new();
    let mut current = String::new();
    for arg in raw_args {
        if arg.trim_start().starts_with('/') && !current.is_empty() {
            commands.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(arg);
    }
    if !current.is_empty() {
        commands.push(current);
    }
    if commands.is_empty() {
        return Err("--resume requires at least one slash command".to_string());
    }
    if let Some(bad) = commands.iter().find(|c| !c.trim_start().starts_with('/')) {
        return Err(format!(
            "--resume trailing arguments must be slash commands (got '{bad}')"
        ));
    }
    // Validate the head of each grouped command against the resume-safe
    // command set. The previous parser only checked that each token started
    // with '/', so `--resume session.json /not-a-command` would parse
    // successfully and then fail at runtime with a confusing error.
    let resume_supported: Vec<&'static str> = resume_supported_slash_commands()
        .iter()
        .map(|spec| spec.name)
        .collect();
    for command in &commands {
        let head = command
            .trim_start()
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");
        if head.is_empty() {
            return Err(format!(
                "--resume command is missing a name (got '{command}')"
            ));
        }
        let head_lower = head.to_ascii_lowercase();
        if !resume_supported.iter().any(|name| *name == head_lower) {
            return Err(format!(
                "--resume command '/{head}' is not a recognised resume-safe slash command \
                 (supported: {})",
                resume_supported
                    .iter()
                    .map(|n| format!("/{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(CliAction::ResumeSession {
        session_path,
        commands,
    })
}

fn print_system_prompt() {
    println!(
        "{}",
        crawler::build_system_prompt(&mvp_tool_specs()).join("\n\n")
    );
}

fn print_version() {
    println!("{}", render_version_report());
}

fn resume_session(session_path: &Path, commands: &[String]) {
    let session = match Session::load_from_path(session_path) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("failed to restore session: {error}");
            std::process::exit(1);
        }
    };
    if commands.is_empty() {
        println!(
            "Restored session from {} ({} messages).",
            session_path.display(),
            session.messages.len()
        );
        return;
    }
    let mut session = session;
    for raw_command in commands {
        let Some(command) = SlashCommand::parse(raw_command) else {
            eprintln!("unsupported resumed command: {raw_command}");
            std::process::exit(2);
        };
        match run_resume_command(session_path, &session, &command) {
            Ok(outcome) => {
                session = outcome.session;
                if let Some(message) = outcome.message {
                    println!("{message}");
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn install_browser() -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    // Check for Node.js 20+
    let node_output = Command::new("node").arg("--version").output();
    let node_major = match node_output {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            let version = version.trim().trim_start_matches('v');
            version
                .split('.')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
        }
        _ => None,
    };

    match node_major {
        None => {
            eprintln!("Error: Node.js not found. Node.js 20+ is required for browser automation.");
            eprintln!("Install from https://nodejs.org/");
            return Err("Node.js not found".into());
        }
        Some(major) if major < 20 => {
            eprintln!("Error: Node.js 20+ required for browser automation (found v{major}.x).");
            eprintln!("Install from https://nodejs.org/");
            return Err(format!("Node.js {major} is too old").into());
        }
        _ => {}
    }

    let config_home = runtime::config_home_dir();
    std::fs::create_dir_all(&config_home)?;

    // npm install cloakbrowser playwright-core
    let already_installed = config_home
        .join("node_modules")
        .join("cloakbrowser")
        .exists()
        && config_home
            .join("node_modules")
            .join("playwright-core")
            .exists();

    if already_installed {
        println!(
            "CloakBrowser already installed at {} (skipping npm install)",
            config_home
                .join("node_modules")
                .join("cloakbrowser")
                .display()
        );
    } else {
        println!("Installing CloakBrowser...");
        let status = Command::new("npm")
            .args([
                "install",
                "--prefix",
                &config_home.to_string_lossy(),
                "cloakbrowser",
                "playwright-core",
            ])
            .status()?;
        if !status.success() {
            return Err(format!(
                "npm install failed. Run manually: npm install --prefix \"{}\" cloakbrowser playwright-core",
                config_home.display()
            )
            .into());
        }
        println!("CloakBrowser installed.");
    }

    // Install system-level OS dependencies required by Chromium (Linux only).
    // playwright-core ships an install-deps subcommand that handles this correctly
    // across distros; it requires root on Linux but is a no-op on macOS.
    #[cfg(target_os = "linux")]
    {
        println!("Installing system dependencies for Chromium...");
        let status = Command::new("npx")
            .args([
                "--prefix",
                &config_home.to_string_lossy(),
                "playwright-core",
                "install-deps",
                "chromium",
            ])
            .status();
        match status {
            Ok(s) if s.success() => println!("System dependencies installed."),
            _ => eprintln!(
                "WARNING: Could not install system dependencies (may need sudo). \
                 If the browser fails to launch, run: sudo npx --prefix \"{}\" playwright-core install-deps chromium",
                config_home.display()
            ),
        }
    }

    // Download the browser binary
    println!("Ensuring browser binary is downloaded...");
    let status = Command::new("npx")
        .args([
            "--prefix",
            &config_home.to_string_lossy(),
            "cloakbrowser",
            "install",
        ])
        .status()?;
    if status.success() {
        println!("Browser binary ready.");
    } else {
        eprintln!("WARNING: Browser binary download failed. It will be downloaded on first use.");
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn print_help_to(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "acrawl v{VERSION}")?;
    writeln!(out)?;
    writeln!(out, "Usage:")?;
    writeln!(
        out,
        "  acrawl [--model MODEL] [--allowedTools TOOL[,TOOL...]]"
    )?;
    writeln!(out, "      Start the interactive REPL")?;
    writeln!(
        out,
        "  acrawl [--model MODEL] [--output-format text|json] prompt TEXT"
    )?;
    writeln!(out, "      Send one prompt and exit")?;
    writeln!(
        out,
        "  acrawl [--model MODEL] [--output-format text|json] TEXT"
    )?;
    writeln!(out, "      Shorthand non-interactive prompt mode")?;
    writeln!(
        out,
        "  acrawl --resume SESSION.json [/status] [/compact] [...]"
    )?;
    writeln!(
        out,
        "      Inspect or maintain a saved session without entering the REPL"
    )?;
    writeln!(out, "  acrawl system-prompt")?;
    writeln!(out, "  acrawl auth [anthropic|openai|other]")?;
    writeln!(
        out,
        "      Configure credentials for a provider interactively"
    )?;
    writeln!(out, "  acrawl update")?;
    writeln!(out, "  acrawl install-browser")?;
    writeln!(
        out,
        "      Install CloakBrowser and download the browser binary"
    )?;
    writeln!(out, "  acrawl uninstall [--purge]")?;
    writeln!(
        out,
        "      Remove acrawl. --purge also deletes settings, credentials, and sessions"
    )?;
    writeln!(out)?;
    writeln!(out, "Flags:")?;
    writeln!(
        out,
        "  --model MODEL              Override the active model"
    )?;
    writeln!(
        out,
        "  --output-format FORMAT     Non-interactive output format: text or json"
    )?;
    writeln!(
        out,
        "  --no-headless              Launch the browser in headed (visible) mode"
    )?;
    writeln!(
        out,
        "  --headless[=BOOL]          Force headless on/off (overrides HEADLESS env)"
    )?;
    writeln!(
        out,
        "  --allowedTools TOOLS       Restrict enabled tools (repeatable; comma-separated aliases supported)"
    )?;
    writeln!(
        out,
        "  --version, -V              Print version and build information locally"
    )?;
    writeln!(out)?;
    writeln!(out, "Interactive slash commands:")?;
    writeln!(out, "{}", render_slash_command_help())?;
    writeln!(out)?;
    let resume_commands = resume_supported_slash_commands()
        .into_iter()
        .map(|spec| match spec.argument_hint {
            Some(hint) => format!("/{} {}", spec.name, hint),
            None => format!("/{}", spec.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "Resume-safe commands: {resume_commands}")?;
    writeln!(out, "Examples:")?;
    writeln!(out, "  acrawl --model claude-opus \"summarize this repo\"")?;
    writeln!(
        out,
        "  acrawl --output-format json prompt \"explain src/main.rs\""
    )?;
    writeln!(
        out,
        "  acrawl --allowedTools read,glob \"summarize Cargo.toml\""
    )?;
    writeln!(
        out,
        "  acrawl --resume session.json /status /compact /export notes.txt"
    )?;
    Ok(())
}

fn print_help() {
    let _ = print_help_to(&mut io::stdout());
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;

    static MODEL_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn model_env_mutex() -> std::sync::MutexGuard<'static, ()> {
        MODEL_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("model env mutex")
    }

    /// Isolate tests that call `parse_args` / `initial_model_from_credentials` from the
    /// real credential store and settings file by pointing `ACRAWL_CONFIG_HOME` to a
    /// temporary directory.
    #[allow(clippy::items_after_statements)]
    fn with_clean_config_env(f: impl FnOnce()) {
        let _guard = model_env_mutex();
        let saved_config_home = env::var("ACRAWL_CONFIG_HOME").ok();
        let temp_dir = std::env::temp_dir().join(format!("acrawl_cli_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        f();
        match saved_config_home {
            Some(val) => env::set_var("ACRAWL_CONFIG_HOME", val),
            None => env::remove_var("ACRAWL_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn defaults_to_repl_when_no_args() {
        with_clean_config_env(|| {
            assert_eq!(
                parse_args(&[]).expect("args should parse"),
                CliAction::Repl {
                    model: None,
                    allowed_tools: None,
                }
            );
        });
    }

    #[test]
    fn parses_prompt_subcommand() {
        with_clean_config_env(|| {
            let args = vec![
                "--model".to_string(),
                "claude-sonnet-4-6".to_string(),
                "prompt".to_string(),
                "hello".to_string(),
                "world".to_string(),
            ];
            assert_eq!(
                parse_args(&args).expect("args should parse"),
                CliAction::Prompt {
                    prompt: "hello world".to_string(),
                    model: "claude-sonnet-4-6".to_string(),
                    output_format: CliOutputFormat::Text,
                    allowed_tools: None,
                }
            );
        });
    }

    #[test]
    fn parses_bare_prompt_and_json_output_flag() {
        with_clean_config_env(|| {
            let args = vec![
                "--output-format=json".to_string(),
                "--model".to_string(),
                "claude-opus".to_string(),
                "explain".to_string(),
                "this".to_string(),
            ];
            assert_eq!(
                parse_args(&args).expect("args should parse"),
                CliAction::Prompt {
                    prompt: "explain this".to_string(),
                    model: "claude-opus".to_string(),
                    output_format: CliOutputFormat::Json,
                    allowed_tools: None,
                }
            );
        });
    }

    #[test]
    fn passes_model_through_without_alias_resolution() {
        with_clean_config_env(|| {
            let args = vec![
                "--model".to_string(),
                "claude-opus-4-6".to_string(),
                "explain".to_string(),
                "this".to_string(),
            ];
            assert_eq!(
                parse_args(&args).expect("args should parse"),
                CliAction::Prompt {
                    prompt: "explain this".to_string(),
                    model: "claude-opus-4-6".to_string(),
                    output_format: CliOutputFormat::Text,
                    allowed_tools: None,
                }
            );
        });
    }

    #[test]
    fn parses_version_flags() {
        assert_eq!(
            parse_args(&["--version".to_string()]).expect("parse"),
            CliAction::Version
        );
        assert_eq!(
            parse_args(&["-V".to_string()]).expect("parse"),
            CliAction::Version
        );
    }

    #[test]
    fn initial_model_defaults_without_credentials() {
        with_clean_config_env(|| {
            assert_eq!(initial_model_from_credentials(), None);
        });
    }

    #[test]
    fn initial_model_skips_unprefixed_settings_model() {
        with_clean_config_env(|| {
            runtime::save_settings(&runtime::Settings {
                model: Some("claude-sonnet-4-6".to_string()),
                ..Default::default()
            })
            .expect("save test settings");

            assert_eq!(initial_model_from_credentials(), None);
        });
    }

    #[test]
    fn initial_model_accepts_prefixed_settings_model() {
        with_clean_config_env(|| {
            runtime::save_settings(&runtime::Settings {
                model: Some("anthropic/claude-sonnet-4-6".to_string()),
                ..Default::default()
            })
            .expect("save test settings");

            assert_eq!(
                initial_model_from_credentials(),
                Some("anthropic/claude-sonnet-4-6".to_string())
            );
        });
    }

    #[test]
    fn rejects_ide_tool_names_not_in_crawler_toolset() {
        let err = parse_args(&["--allowedTools".to_string(), "read_file".to_string()])
            .expect_err("read_file is an IDE tool, not a crawler tool");
        assert!(err.contains("unsupported tool in --allowedTools: read_file"));
    }

    #[test]
    fn system_prompt_contains_no_ide_content() {
        let prompt = crawler::build_system_prompt(&mvp_tool_specs()).join("\n\n");
        assert!(
            !prompt.contains("Working directory"),
            "system prompt should not mention working directory"
        );
        assert!(
            !prompt.contains("Git status"),
            "system prompt should not mention git status"
        );
        assert!(
            !prompt.contains("Model family"),
            "system prompt should not mention model family"
        );
        assert!(
            !prompt.contains("Claw Code"),
            "system prompt should not mention Claw Code"
        );
        assert!(
            prompt.contains("autonomous web crawler"),
            "system prompt should describe crawler role"
        );
    }

    #[test]
    fn rejects_unknown_allowed_tools() {
        let error = parse_args(&["--allowedTools".to_string(), "teleport".to_string()])
            .expect_err("tool should be rejected");
        assert!(error.contains("unsupported tool in --allowedTools: teleport"));
    }

    #[test]
    fn login_and_logout_subcommands_return_error() {
        let login_err = parse_args(&["login".to_string()]).unwrap_err();
        assert!(login_err.contains("removed"), "{login_err}");
        let logout_err = parse_args(&["logout".to_string()]).unwrap_err();
        assert!(logout_err.contains("removed"), "{logout_err}");
    }

    #[test]
    fn parses_resume_flag_with_slash_command() {
        let args = vec![
            "--resume".to_string(),
            "session.json".to_string(),
            "/compact".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::ResumeSession {
                session_path: PathBuf::from("session.json"),
                commands: vec!["/compact".to_string()],
            }
        );
    }

    #[test]
    fn parses_resume_flag_with_multiple_slash_commands() {
        let args = vec![
            "--resume".to_string(),
            "session.json".to_string(),
            "/status".to_string(),
            "/compact".to_string(),
            "/cost".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::ResumeSession {
                session_path: PathBuf::from("session.json"),
                commands: vec![
                    "/status".to_string(),
                    "/compact".to_string(),
                    "/cost".to_string(),
                ],
            }
        );
    }

    #[test]
    fn rejects_resume_unknown_slash_command() {
        // Previously, anything starting with `/` was accepted by the resume
        // parser and the failure only surfaced at runtime with a confusing
        // error. Validate eagerly against the resume-safe spec list.
        let args = vec![
            "--resume".to_string(),
            "session.json".to_string(),
            "/not-a-command".to_string(),
        ];
        let err = parse_args(&args).expect_err("unknown command must be rejected");
        assert!(
            err.contains("not a recognised resume-safe slash command"),
            "{err}"
        );
        assert!(err.contains("/not-a-command"), "{err}");
    }

    #[test]
    fn rejects_resume_command_known_but_not_resume_safe() {
        // `/model` exists but is not in resume_supported_slash_commands(); it
        // mutates session-level config in a way that's not safe at replay.
        let args = vec![
            "--resume".to_string(),
            "session.json".to_string(),
            "/auth".to_string(),
        ];
        let err = parse_args(&args).expect_err("non-resume-safe command must be rejected");
        assert!(
            err.contains("not a recognised resume-safe slash command"),
            "{err}"
        );
    }

    #[test]
    fn parses_resume_flag_with_command_arguments() {
        let args = vec![
            "--resume".to_string(),
            "session.json".to_string(),
            "/clear".to_string(),
            "--confirm".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::ResumeSession {
                session_path: PathBuf::from("session.json"),
                commands: vec!["/clear --confirm".to_string()],
            }
        );
    }

    #[test]
    fn shared_help_uses_resume_annotation_copy() {
        let help = commands::render_slash_command_help();
        assert!(help.contains("Slash commands"));
        assert!(help.contains("works with --resume SESSION.json"));
    }

    #[test]
    fn resume_supported_command_list_matches_expected_surface() {
        let names = resume_supported_slash_commands()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["help", "status", "compact", "clear", "cost", "config", "version", "export",]
        );
    }

    #[test]
    fn clear_command_requires_explicit_confirmation_flag() {
        assert_eq!(
            SlashCommand::parse("/clear"),
            Some(SlashCommand::Clear { confirm: false })
        );
        assert_eq!(
            SlashCommand::parse("/clear --confirm"),
            Some(SlashCommand::Clear { confirm: true })
        );
    }

    #[test]
    fn parses_sessions_and_config_slash_commands() {
        assert_eq!(
            SlashCommand::parse("/sessions"),
            Some(SlashCommand::Sessions)
        );
        assert_eq!(
            SlashCommand::parse("/config"),
            Some(SlashCommand::Config { section: None })
        );
        assert_eq!(
            SlashCommand::parse("/config env"),
            Some(SlashCommand::Config {
                section: Some("env".to_string())
            })
        );
        assert_eq!(SlashCommand::parse("/debug"), Some(SlashCommand::Debug));
    }

    #[test]
    fn auth_subcommand_parses_without_provider() {
        assert_eq!(
            parse_args(&["auth".to_string()]).expect("auth should parse"),
            CliAction::Auth { provider: None }
        );
    }

    #[test]
    fn auth_subcommand_parses_with_provider() {
        assert_eq!(
            parse_args(&["auth".to_string(), "anthropic".to_string()]).expect("auth anthropic"),
            CliAction::Auth {
                provider: Some("anthropic".to_string())
            }
        );
    }

    #[test]
    fn auth_subcommand_parses_openai() {
        assert_eq!(
            parse_args(&["auth".to_string(), "openai".to_string()]).expect("auth openai"),
            CliAction::Auth {
                provider: Some("openai".to_string())
            }
        );
    }

    #[test]
    fn parses_uninstall_subcommand() {
        assert_eq!(
            parse_args(&["uninstall".to_string()]).expect("uninstall"),
            CliAction::Uninstall { purge: false }
        );
    }

    #[test]
    fn parses_uninstall_with_purge_flag() {
        assert_eq!(
            parse_args(&["uninstall".to_string(), "--purge".to_string()])
                .expect("uninstall --purge"),
            CliAction::Uninstall { purge: true }
        );
    }

    #[test]
    fn uninstall_help_mentions_purge() {
        let mut help = Vec::new();
        print_help_to(&mut help).expect("help should render");
        let help = String::from_utf8(help).expect("help should be utf8");
        assert!(help.contains("acrawl uninstall"));
        assert!(help.contains("--purge"));
    }
}
