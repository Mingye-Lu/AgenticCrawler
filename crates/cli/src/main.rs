mod self_update;
mod uninstall;

use std::collections::BTreeMap;
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process;

use acrawl_ui::{
    app::{
        initial_model_from_credentials, run_auth_cli, run_resume_command, AllowedToolSet, LiveCli,
    },
    error::CliError,
    AuthFlags, CliOutputFormat, ConfigAction, TOKIO_RUNTIME,
};
use agent::mvp_tool_specs;
use commands::{render_slash_command_help, resume_supported_slash_commands, SlashCommand};
use render::format::{render_version_report, VERSION};
use runtime::Session;

fn main() {
    // Load settings.json and set env vars consumed by child processes / the crawler.
    let settings = runtime::load_settings();
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

    let args: Vec<String> = env::args().skip(1).collect();
    let action = match parse_args(&args) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(2);
        }
    };

    if let Err(error) = run(action) {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

fn run(action: CliAction) -> Result<(), Box<dyn std::error::Error>> {
    match action {
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
        CliAction::Auth { provider } => {
            if !std::io::stdin().is_terminal() {
                eprintln!(
                    "interactive `acrawl auth` requires a TTY; use `--api-key` for non-interactive configuration"
                );
                process::exit(2);
            }
            run_auth_cli(provider.as_deref())?;
        }
        CliAction::AuthConfigure {
            provider,
            flags,
            json,
        } => process::exit(acrawl_ui::run_auth_configure(&provider, flags, json)),
        CliAction::AuthStatus { check, json } => {
            process::exit(acrawl_ui::run_auth_status(check.as_deref(), json));
        }
        CliAction::AuthList { json } => process::exit(acrawl_ui::run_auth_list(json)),
        CliAction::Update => {
            let rt = TOKIO_RUNTIME.get().expect("tokio runtime not initialized");
            rt.block_on(self_update::run_self_update())?;
        }
        CliAction::Uninstall { purge } => uninstall::run_uninstall(purge)?,
        CliAction::InstallBrowser => install_browser()?,
        CliAction::Mcp => mcp_server::run_mcp_server(),
        CliAction::McpInstall => {
            if !std::io::stdin().is_terminal() {
                eprintln!(
                    "interactive `acrawl mcp install` requires a TTY; use `--client` or `--all` for non-interactive mode"
                );
                process::exit(2);
            }
            mcp_server::run_install()?;
        }
        CliAction::McpUninstall => {
            if !std::io::stdin().is_terminal() {
                eprintln!(
                    "interactive `acrawl mcp uninstall` requires a TTY; use `--client` or `--all` for non-interactive mode"
                );
                process::exit(2);
            }
            mcp_server::run_uninstall()?;
        }
        CliAction::ConfigGet {
            key,
            effective,
            json,
        } => process::exit(acrawl_ui::run_config(
            ConfigAction::Get { key, effective },
            json,
        )),
        CliAction::ConfigSet { key, value } => {
            process::exit(acrawl_ui::run_config(
                ConfigAction::Set { key, value },
                false,
            ));
        }
        CliAction::ConfigUnset { key } => {
            process::exit(acrawl_ui::run_config(ConfigAction::Unset { key }, false));
        }
        CliAction::ConfigPath => process::exit(acrawl_ui::run_config(ConfigAction::Path, false)),
        CliAction::McpInstallConfigured {
            clients,
            all,
            scope,
            json,
        } => process::exit(run_mcp_configured(clients, all, scope, json, true)),
        CliAction::McpUninstallConfigured {
            clients,
            all,
            scope,
            json,
        } => process::exit(run_mcp_configured(clients, all, scope, json, false)),
        CliAction::McpListClients { json } => {
            mcp_server::list_clients(json);
            process::exit(0);
        }
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

fn run_repl(model: String, allowed_tools: Option<AllowedToolSet>) -> Result<(), CliError> {
    if !std::io::stdout().is_terminal() {
        return Err(CliError::from(
            "acrawl REPL requires an interactive terminal. \
             For headless use, run `acrawl prompt \"<goal>\"` (one-shot) \
             or `acrawl --resume <session.json> <slash-commands>` (session maintenance).",
        ));
    }
    Ok(acrawl_tui::run_tui(model, allowed_tools)?)
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
    AuthConfigure {
        provider: String,
        flags: AuthFlags,
        json: bool,
    },
    AuthStatus {
        check: Option<String>,
        json: bool,
    },
    AuthList {
        json: bool,
    },
    Update,
    Uninstall {
        purge: bool,
    },
    InstallBrowser,
    Mcp,
    McpInstall,
    McpUninstall,
    ConfigGet {
        key: Option<String>,
        effective: bool,
        json: bool,
    },
    ConfigSet {
        key: String,
        value: String,
    },
    ConfigUnset {
        key: String,
    },
    ConfigPath,
    McpInstallConfigured {
        clients: Vec<String>,
        all: bool,
        scope: Option<String>,
        json: bool,
    },
    McpUninstallConfigured {
        clients: Vec<String>,
        all: bool,
        scope: Option<String>,
        json: bool,
    },
    McpListClients {
        json: bool,
    },
    Repl {
        model: Option<String>,
        allowed_tools: Option<AllowedToolSet>,
    },
    Help,
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
        "auth" => parse_auth_args(&rest[1..]),
        "config" => parse_config_args(&rest[1..]),
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
        "mcp" => parse_mcp_args(&rest[1..]),
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
        other => Err(format!(
            "unknown subcommand: {other}\n\nUsage: acrawl prompt \"your goal here\"\n       acrawl -p your goal here\n\nRun `acrawl --help` for all options."
        )),
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

fn parse_auth_args(args: &[String]) -> Result<CliAction, String> {
    match args.first().map(String::as_str) {
        Some("status") => parse_auth_status_args(&args[1..]),
        Some("list") => parse_auth_list_args(&args[1..]),
        _ => parse_auth_config_or_interactive_args(args),
    }
}

fn parse_auth_status_args(args: &[String]) -> Result<CliAction, String> {
    let mut check = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--check" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --check".to_string())?;
                check = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--check=") => {
                check = Some(flag[8..].to_string());
                index += 1;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            other => return Err(format!("unknown auth status option: {other}")),
        }
    }

    Ok(CliAction::AuthStatus { check, json })
}

fn parse_auth_list_args(args: &[String]) -> Result<CliAction, String> {
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other => return Err(format!("unknown auth list option: {other}")),
        }
    }
    Ok(CliAction::AuthList { json })
}

fn parse_auth_config_or_interactive_args(args: &[String]) -> Result<CliAction, String> {
    let mut provider = None;
    let mut flags = AuthFlags::default();
    let mut json = false;
    let mut saw_cred_flag = false;
    let mut index = 0;

    if let Some(first) = args.first().filter(|arg| !arg.starts_with('-')) {
        provider = Some(first.clone());
        index = 1;
    }

    while index < args.len() {
        match args[index].as_str() {
            "--api-key" => {
                flags.api_key = Some(required_flag_value(args, index, "--api-key")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--api-key=") => {
                flags.api_key = Some(flag[10..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--access-key" => {
                flags.access_key = Some(required_flag_value(args, index, "--access-key")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--access-key=") => {
                flags.access_key = Some(flag[13..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--secret-key" => {
                flags.secret_key = Some(required_flag_value(args, index, "--secret-key")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--secret-key=") => {
                flags.secret_key = Some(flag[13..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--region" => {
                flags.region = Some(required_flag_value(args, index, "--region")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--region=") => {
                flags.region = Some(flag[9..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--resource-name" => {
                flags.resource_name =
                    Some(required_flag_value(args, index, "--resource-name")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--resource-name=") => {
                flags.resource_name = Some(flag[16..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--deployment-name" => {
                flags.deployment_name =
                    Some(required_flag_value(args, index, "--deployment-name")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--deployment-name=") => {
                flags.deployment_name = Some(flag[18..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--base-url" => {
                flags.base_url = Some(required_flag_value(args, index, "--base-url")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--base-url=") => {
                flags.base_url = Some(flag[11..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--gcp-project" => {
                flags.gcp_project =
                    Some(required_flag_value(args, index, "--gcp-project")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--gcp-project=") => {
                flags.gcp_project = Some(flag[14..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--gcp-region" => {
                flags.gcp_region = Some(required_flag_value(args, index, "--gcp-region")?.clone());
                saw_cred_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--gcp-region=") => {
                flags.gcp_region = Some(flag[13..].to_string());
                saw_cred_flag = true;
                index += 1;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            other => return Err(format!("unknown auth option: {other}")),
        }
    }

    if saw_cred_flag {
        let provider = provider.ok_or_else(|| {
            "non-interactive `acrawl auth` requires a provider before credential flags".to_string()
        })?;
        return Ok(CliAction::AuthConfigure {
            provider,
            flags,
            json,
        });
    }

    if json {
        return Err(
            "`--json` requires credential flags, `auth status`, or `auth list`".to_string(),
        );
    }

    if index < args.len() {
        return Err(format!("unknown auth option: {}", args[index]));
    }

    Ok(CliAction::Auth { provider })
}

fn parse_config_args(args: &[String]) -> Result<CliAction, String> {
    match args.first().map(String::as_str) {
        Some("get") => parse_config_get_args(&args[1..]),
        Some("set") => parse_config_set_args(&args[1..]),
        Some("unset") => parse_config_unset_args(&args[1..]),
        Some("path") => parse_config_path_args(&args[1..]),
        Some(other) => Err(format!(
            "unknown config subcommand: {other} (supported: get, set, unset, path)"
        )),
        None => Err("missing config subcommand (supported: get, set, unset, path)".to_string()),
    }
}

fn parse_config_get_args(args: &[String]) -> Result<CliAction, String> {
    let mut key = None;
    let mut effective = false;
    let mut json = false;

    for arg in args {
        match arg.as_str() {
            "--effective" => effective = true,
            "--json" => json = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown config get option: {other}"))
            }
            other => {
                if key.is_some() {
                    return Err("config get accepts at most one key".to_string());
                }
                key = Some(other.to_string());
            }
        }
    }

    Ok(CliAction::ConfigGet {
        key,
        effective,
        json,
    })
}

fn parse_config_set_args(args: &[String]) -> Result<CliAction, String> {
    if args.len() != 2 {
        return Err("usage: acrawl config set <key> <value>".to_string());
    }
    Ok(CliAction::ConfigSet {
        key: args[0].clone(),
        value: args[1].clone(),
    })
}

fn parse_config_unset_args(args: &[String]) -> Result<CliAction, String> {
    if args.len() != 1 {
        return Err("usage: acrawl config unset <key>".to_string());
    }
    Ok(CliAction::ConfigUnset {
        key: args[0].clone(),
    })
}

fn parse_config_path_args(args: &[String]) -> Result<CliAction, String> {
    if let Some(other) = args.first() {
        return Err(format!("unknown config path option: {other}"));
    }
    Ok(CliAction::ConfigPath)
}

fn parse_mcp_args(args: &[String]) -> Result<CliAction, String> {
    match args.first().map(String::as_str) {
        None => Ok(CliAction::Mcp),
        Some("install") => parse_mcp_configured_args(&args[1..], true),
        Some("uninstall") => parse_mcp_configured_args(&args[1..], false),
        Some("--help" | "-h" | "help") if args.len() == 1 => Ok(CliAction::Help),
        Some(other) => Err(format!(
            "unknown mcp subcommand: {other} (supported: install, uninstall)"
        )),
    }
}

fn parse_mcp_configured_args(args: &[String], install: bool) -> Result<CliAction, String> {
    if args.is_empty() {
        return Ok(if install {
            CliAction::McpInstall
        } else {
            CliAction::McpUninstall
        });
    }

    let mut clients = Vec::new();
    let mut all = false;
    let mut scope = None;
    let mut json = false;
    let mut list_clients = false;
    let mut saw_config_flag = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--client" => {
                saw_config_flag = true;
                let value = required_flag_value(args, index, "--client")?;
                extend_clients(&mut clients, value)?;
                index += 2;
            }
            flag if flag.starts_with("--client=") => {
                saw_config_flag = true;
                extend_clients(&mut clients, &flag[9..])?;
                index += 1;
            }
            "--all" => {
                all = true;
                saw_config_flag = true;
                index += 1;
            }
            "--scope" => {
                scope = Some(required_flag_value(args, index, "--scope")?.clone());
                saw_config_flag = true;
                index += 2;
            }
            flag if flag.starts_with("--scope=") => {
                scope = Some(flag[8..].to_string());
                saw_config_flag = true;
                index += 1;
            }
            "--yes" => {
                saw_config_flag = true;
                index += 1;
            }
            "--json" => {
                json = true;
                saw_config_flag = true;
                index += 1;
            }
            "--list-clients" => {
                list_clients = true;
                saw_config_flag = true;
                index += 1;
            }
            other => {
                return Err(format!(
                    "`acrawl mcp {}` does not accept extra arguments: {other}",
                    if install { "install" } else { "uninstall" }
                ));
            }
        }
    }

    if !saw_config_flag {
        return Ok(if install {
            CliAction::McpInstall
        } else {
            CliAction::McpUninstall
        });
    }

    if list_clients {
        return Ok(CliAction::McpListClients { json });
    }

    if let Some(scope_value) = scope.as_deref() {
        parse_scope(scope_value)?;
    }

    Ok(if install {
        CliAction::McpInstallConfigured {
            clients,
            all,
            scope,
            json,
        }
    } else {
        CliAction::McpUninstallConfigured {
            clients,
            all,
            scope,
            json,
        }
    })
}

fn required_flag_value<'a>(
    args: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a String, String> {
    args.get(index + 1)
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn extend_clients(clients: &mut Vec<String>, value: &str) -> Result<(), String> {
    for token in value.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let ide = mcp_server::client_from_key(trimmed).ok_or_else(|| {
            format!(
                "unknown MCP client `{trimmed}` (supported: {})",
                mcp_server::all_client_keys()
                    .iter()
                    .map(|(key, _)| *key)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        clients.push(client_key(ide).to_string());
    }
    Ok(())
}

fn parse_scope(value: &str) -> Result<mcp_server::Scope, String> {
    match value {
        "user" => Ok(mcp_server::Scope::Global),
        "project" => Ok(mcp_server::Scope::Project),
        other => Err(format!(
            "unsupported value for --scope: {other} (expected user or project)"
        )),
    }
}

fn client_key(ide: mcp_server::Ide) -> &'static str {
    mcp_server::all_client_keys()
        .iter()
        .find_map(|(key, _)| (mcp_server::client_from_key(key) == Some(ide)).then_some(*key))
        .unwrap_or_default()
}

fn run_mcp_configured(
    clients: Vec<String>,
    all: bool,
    scope: Option<String>,
    json: bool,
    install: bool,
) -> i32 {
    let explicit_client = !clients.is_empty();
    if !all && clients.is_empty() {
        return emit_subcommand_error(
            json,
            "non-interactive MCP mode requires `--client` or `--all`",
            2,
        );
    }

    let selected = if all {
        mcp_server::all_client_keys()
            .iter()
            .map(|(key, _)| mcp_server::client_from_key(key).unwrap_or(mcp_server::Ide::Cursor))
            .collect::<Vec<_>>()
    } else {
        clients
            .iter()
            .filter_map(|key| mcp_server::client_from_key(key))
            .collect::<Vec<_>>()
    };

    let scope = match scope.as_deref() {
        Some(value) => match parse_scope(value) {
            Ok(scope) => scope,
            Err(error) => return emit_subcommand_error(json, &error, 2),
        },
        None => mcp_server::Scope::Global,
    };

    let report = if install {
        mcp_server::run_install_for(&selected, scope, json)
    } else {
        mcp_server::run_uninstall_for(&selected, scope, json)
    };

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                return emit_subcommand_error(
                    true,
                    &format!("failed to serialize MCP report: {error}"),
                    1,
                );
            }
        }
    }

    let success_count = report
        .results
        .iter()
        .filter(|result| {
            matches!(
                result.status,
                mcp_server::installer::ClientStatus::Configured
                    | mcp_server::installer::ClientStatus::ManualInstructions
                    | mcp_server::installer::ClientStatus::Removed
                    | mcp_server::installer::ClientStatus::NotFound
            )
        })
        .count();

    if explicit_client && success_count == 0 {
        3
    } else if success_count > 0 {
        0
    } else {
        1
    }
}

fn emit_subcommand_error(json: bool, error: &str, code: i32) -> i32 {
    if json {
        println!("{}", serde_json::json!({ "ok": false, "error": error }));
    } else {
        eprintln!("{error}");
    }
    code
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
        agent::build_system_prompt(&mvp_tool_specs(), None).join("\n\n")
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
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "npm"]);
            c
        } else {
            Command::new("npm")
        };
        let status = cmd
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
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "npx"]);
        c
    } else {
        Command::new("npx")
    };
    let status = cmd
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
    writeln!(out, "  acrawl mcp")?;
    writeln!(out, "      Start the built-in MCP server over stdio")?;
    writeln!(out, "  acrawl mcp install")?;
    writeln!(out, "      Configure supported IDEs to launch `acrawl mcp`")?;
    writeln!(out, "  acrawl mcp uninstall")?;
    writeln!(out, "      Remove acrawl from IDE MCP configurations")?;
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
        "  --allowedTools TOOLS       Restrict enabled tools (repeatable; comma-separated)"
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
    writeln!(
        out,
        "  acrawl --model anthropic/claude-opus-4-6 prompt \"summarize this repo\""
    )?;
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
                "anthropic/claude-sonnet-4-6".to_string(),
                "prompt".to_string(),
                "hello".to_string(),
                "world".to_string(),
            ];
            assert_eq!(
                parse_args(&args).expect("args should parse"),
                CliAction::Prompt {
                    prompt: "hello world".to_string(),
                    model: "anthropic/claude-sonnet-4-6".to_string(),
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
                "anthropic/claude-opus-4-6".to_string(),
                "prompt".to_string(),
                "explain".to_string(),
                "this".to_string(),
            ];
            assert_eq!(
                parse_args(&args).expect("args should parse"),
                CliAction::Prompt {
                    prompt: "explain this".to_string(),
                    model: "anthropic/claude-opus-4-6".to_string(),
                    output_format: CliOutputFormat::Json,
                    allowed_tools: None,
                }
            );
        });
    }

    #[test]
    fn passes_model_through_verbatim() {
        with_clean_config_env(|| {
            let args = vec![
                "--model".to_string(),
                "anthropic/claude-opus-4-6".to_string(),
                "prompt".to_string(),
                "explain".to_string(),
                "this".to_string(),
            ];
            assert_eq!(
                parse_args(&args).expect("args should parse"),
                CliAction::Prompt {
                    prompt: "explain this".to_string(),
                    model: "anthropic/claude-opus-4-6".to_string(),
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
        let prompt = agent::build_system_prompt(&mvp_tool_specs(), None).join("\n\n");
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
    fn clear_command_takes_no_arguments() {
        assert_eq!(SlashCommand::parse("/clear"), Some(SlashCommand::Clear));
        assert_eq!(
            SlashCommand::parse("/clear --confirm"),
            Some(SlashCommand::Unknown("clear".to_string()))
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
    fn parses_mcp_subcommand() {
        assert_eq!(
            parse_args(&["mcp".to_string()]).expect("mcp"),
            CliAction::Mcp
        );
    }

    #[test]
    fn parses_mcp_install_subcommand() {
        assert_eq!(
            parse_args(&["mcp".to_string(), "install".to_string()]).expect("mcp install"),
            CliAction::McpInstall
        );
    }

    #[test]
    fn parses_mcp_help_as_global_help() {
        assert_eq!(
            parse_args(&["mcp".to_string(), "--help".to_string()]).expect("mcp help"),
            CliAction::Help
        );
    }

    #[test]
    fn rejects_unknown_mcp_subcommand() {
        let err = parse_args(&["mcp".to_string(), "bogus".to_string()])
            .expect_err("unknown mcp subcommand should fail");
        assert!(err.contains("unknown mcp subcommand: bogus"));
    }

    #[test]
    fn rejects_extra_args_for_mcp_install() {
        let err = parse_args(&[
            "mcp".to_string(),
            "install".to_string(),
            "extra".to_string(),
        ])
        .expect_err("mcp install extra args should fail");
        assert!(err.contains("does not accept extra arguments"));
    }

    #[test]
    fn uninstall_help_mentions_purge() {
        let mut help = Vec::new();
        print_help_to(&mut help).expect("help should render");
        let help = String::from_utf8(help).expect("help should be utf8");
        assert!(help.contains("acrawl uninstall"));
        assert!(help.contains("--purge"));
        assert!(help.contains("acrawl mcp"));
        assert!(help.contains("acrawl mcp install"));
    }
}
