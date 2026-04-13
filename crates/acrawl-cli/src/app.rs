use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;

use crate::render::{MarkdownStreamState, Spinner, TerminalRenderer};
use api::{
    AnthropicClient, AuthSource, ChatCompletionsClient, ContentBlockDelta, InputContentBlock,
    InputMessage, MessageRequest, MessageResponse, OpenAiMessageStream, OpenAiResponsesClient,
    OutputContentBlock, ResponsesMessageStream, StreamEvent as ApiStreamEvent, ToolChoice,
    ToolDefinition, ToolResultContentBlock,
};
use commands::{slash_command_specs, SlashCommand};
use crawler::{mvp_tool_specs, CrawlerAgent, ToolRegistry};
use runtime::{
    clear_oauth_credentials, generate_pkce_pair, generate_state, load_oauth_credentials,
    load_system_prompt, parse_oauth_callback_request_target, save_oauth_credentials, ApiClient,
    ApiRequest, AssistantEvent, CompactionConfig, ConfigLoader, ConversationMessage,
    ConversationRuntime, MessageRole, OAuthAuthorizationRequest, OAuthConfig,
    OAuthTokenExchangeRequest, PermissionMode, PermissionPolicy, PermissionPromptDecision,
    PermissionPrompter, PermissionRequest, RuntimeError, Session, TokenUsage, ToolError,
    ToolExecutor,
};
use serde_json::json;

use crate::format::{
    format_auto_compaction_notice, format_compact_report, format_cost_report, format_model_report,
    format_model_switch_report, format_permissions_report, format_permissions_switch_report,
    format_resume_report, format_status_report, normalize_permission_mode, render_config_report,
    render_export_text, render_last_tool_debug_report, render_repl_help, render_version_report,
    resolve_export_path, status_context, StatusUsage, DEFAULT_DATE,
};
use crate::input;
use crate::session_mgr::{
    create_managed_session_handle, render_session_list, resolve_session_reference, SessionHandle,
};
use crate::tui::tool_panel::{format_tool_call_start, format_tool_result};
use crate::tui::ReplTuiEvent;

pub(crate) type AllowedToolSet = BTreeSet<String>;

const DEFAULT_OAUTH_CALLBACK_PORT: u16 = 4545;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    Anthropic,
    OpenAi,
    Other,
}

fn provider_for_model(model: &str) -> Provider {
    if model.starts_with("claude") {
        Provider::Anthropic
    } else if model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("codex-")
        || model.starts_with("chatgpt-")
    {
        Provider::OpenAi
    } else {
        Provider::Other
    }
}

pub(crate) fn max_tokens_for_model(model: &str) -> u32 {
    match provider_for_model(model) {
        Provider::OpenAi => 16_384,
        // OpenAI-compatible endpoints vary widely; keep a conservative default
        // to avoid immediate 400s on providers capped at 8192 (e.g. DeepSeek-compatible setups).
        Provider::Other => 8_192,
        Provider::Anthropic => {
            if model.contains("opus") {
                32_000
            } else {
                64_000
            }
        }
    }
}

pub(crate) fn resolve_model_alias(model: &str) -> &str {
    match model {
        "opus" => "claude-opus-4-6",
        "sonnet" => "claude-sonnet-4-6",
        "haiku" => "claude-haiku-4-5-20251213",
        "gpt4o" | "4o" => "gpt-4o",
        "gpt4" => "gpt-4-turbo",
        "codex" => "codex-mini-latest",
        _ => model,
    }
}

pub(crate) fn initial_model_from_credentials() -> Option<String> {
    if let Some(model) = initial_model_from_env() {
        return Some(model);
    }
    let store = api::load_credentials().unwrap_or_default();
    if let Some(provider_name) = &store.active_provider {
        if let Some(config) = store.providers.get(provider_name) {
            if let Some(model) = &config.default_model {
                return Some(resolve_model_alias(model).to_string());
            }
        }
    }
    None
}

fn initial_model_from_env() -> Option<String> {
    let provider = env::var("LLM_PROVIDER")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let env_model = match provider.as_str() {
        "openai" => env::var("OPENAI_MODEL").ok(),
        "codex" => env::var("CODEX_MODEL").ok(),
        _ => env::var("CLAUDE_MODEL").ok(),
    }?;
    let trimmed = env_model.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(resolve_model_alias(trimmed).to_string())
    }
}

pub(crate) fn default_permission_mode() -> PermissionMode {
    let settings = runtime::load_settings();
    settings
        .permission_mode
        .as_deref()
        .and_then(normalize_permission_mode)
        .map_or(PermissionMode::DangerFullAccess, permission_mode_from_label)
}

pub(crate) fn permission_mode_from_label(mode: &str) -> PermissionMode {
    match mode {
        "read-only" => PermissionMode::ReadOnly,
        "workspace-write" => PermissionMode::WorkspaceWrite,
        "danger-full-access" => PermissionMode::DangerFullAccess,
        other => panic!("unsupported permission mode label: {other}"),
    }
}

pub(crate) fn filter_tool_specs(allowed_tools: Option<&AllowedToolSet>) -> Vec<crawler::ToolSpec> {
    mvp_tool_specs()
        .into_iter()
        .filter(|spec| allowed_tools.is_none_or(|allowed| allowed.contains(spec.name)))
        .collect()
}

pub(crate) fn slash_command_completion_candidates() -> Vec<String> {
    slash_command_specs()
        .iter()
        .map(|spec| format!("/{}", spec.name))
        .collect()
}

pub(crate) fn run_repl(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let classic =
        runtime::load_settings().classic_repl.unwrap_or(false) || !io::stdout().is_terminal();
    if classic {
        run_repl_classic(model, allowed_tools, permission_mode)
    } else {
        crate::tui::run_repl_ratatui(model, allowed_tools, permission_mode)
    }
}

fn run_repl_classic(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cli = LiveCli::new(model, true, allowed_tools, permission_mode)?;
    let mut editor = input::LineEditor::new("> ", slash_command_completion_candidates());
    println!("{}", cli.startup_banner());

    loop {
        match editor.read_line()? {
            input::ReadOutcome::Submit(input) => {
                let trimmed = input.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.eq_ignore_ascii_case("/exit") || trimmed.eq_ignore_ascii_case("/quit") {
                    cli.persist_session()?;
                    break;
                }
                if let Some(command) = SlashCommand::parse(&trimmed) {
                    if cli.handle_repl_command(command)? {
                        cli.persist_session()?;
                    }
                    continue;
                }
                editor.push_history(input);
                cli.run_turn(&trimmed)?;
            }
            input::ReadOutcome::Cancel => {}
            input::ReadOutcome::Exit => {
                cli.persist_session()?;
                break;
            }
        }
    }

    Ok(())
}

pub(crate) struct LiveCli {
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    system_prompt: Vec<String>,
    runtime: ConversationRuntime<LlmRuntimeClient, CliToolExecutor>,
    session: SessionHandle,
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandUiResult {
    pub(crate) message: String,
    pub(crate) persist_after: bool,
}

impl LiveCli {
    pub(crate) fn new(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let system_prompt = build_system_prompt()?;
        let session = create_managed_session_handle()?;
        let runtime = build_runtime(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            true,
            allowed_tools.clone(),
            permission_mode,
            None,
        )?;
        let cli = Self {
            model,
            allowed_tools,
            permission_mode,
            system_prompt,
            runtime,
            session,
            ui_tx: None,
        };
        cli.persist_session()?;
        Ok(cli)
    }

    pub(crate) fn new_with_ui_tx(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
        ui_tx: mpsc::Sender<ReplTuiEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let system_prompt = build_system_prompt()?;
        let session = create_managed_session_handle()?;
        let runtime = build_runtime(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            false,
            allowed_tools.clone(),
            permission_mode,
            Some(ui_tx.clone()),
        )?;
        let cli = Self {
            model,
            allowed_tools,
            permission_mode,
            system_prompt,
            runtime,
            session,
            ui_tx: Some(ui_tx),
        };
        cli.persist_session()?;
        Ok(cli)
    }

    pub(crate) fn session_id(&self) -> &str {
        self.session.id.as_str()
    }

    pub(crate) fn model_name(&self) -> &str {
        &self.model
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    pub(crate) fn cumulative_usage(&self) -> TokenUsage {
        self.runtime.usage().cumulative_usage()
    }

    fn ui_sender(&self) -> Option<mpsc::Sender<ReplTuiEvent>> {
        self.ui_tx.clone()
    }

    pub(crate) fn cancel_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.runtime.cancel_flag()
    }

    pub(crate) fn run_turn_tui(
        &mut self,
        input: &str,
        mut permission_prompter: ChannelPermissionPrompter,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(ReplTuiEvent::TurnStarting);
        }
        let result = self.runtime.run_turn(input, Some(&mut permission_prompter));
        let finish: Result<(), String> = match &result {
            Ok(summary) => {
                if let Some(ev) = summary.auto_compaction {
                    let msg = format_auto_compaction_notice(ev.removed_message_count);
                    if let Some(tx) = &self.ui_tx {
                        let _ = tx.send(ReplTuiEvent::SystemMessage(msg));
                    }
                }
                self.persist_session().map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        };
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(ReplTuiEvent::TurnFinished(finish.clone()));
        }
        match result {
            Ok(_) => finish.map_err(std::convert::Into::into),
            Err(e) => Err(e.into()),
        }
    }

    fn startup_banner(&self) -> String {
        let cwd = env::current_dir().map_or_else(
            |_| "<unknown>".to_string(),
            |path| path.display().to_string(),
        );
        format!(
            "\x1b[38;5;35m\
  █████╗  ██████╗██████╗  █████╗ ██╗    ██╗██╗\n\
 ██╔══██╗██╔════╝██╔══██╗██╔══██╗██║    ██║██║\n\
 ███████║██║     ██████╔╝███████║██║ █╗ ██║██║\n\
 ██╔══██║██║     ██╔══██╗██╔══██║██║███╗██║██║\n\
 ██║  ██║╚██████╗██║  ██║██║  ██║╚███╔███╔╝███████╗\n\
 ╚═╝  ╚═╝ ╚═════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝ ╚══════╝\x1b[0m 🕷️\n\n\
  \x1b[2mModel\x1b[0m            {}\n\
  \x1b[2mPermissions\x1b[0m      {}\n\
  \x1b[2mDirectory\x1b[0m        {}\n\
  \x1b[2mSession\x1b[0m          {}\n\n\
  Type \x1b[1m/help\x1b[0m for commands · \x1b[2mShift+Enter\x1b[0m for newline",
            self.model,
            self.permission_mode.as_str(),
            cwd,
            self.session.id,
        )
    }

    pub(crate) fn run_turn(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut spinner = Spinner::new();
        let mut stdout = io::stdout();
        spinner.tick(
            "🕷️ Thinking...",
            TerminalRenderer::new().color_theme(),
            &mut stdout,
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let result = self.runtime.run_turn(input, Some(&mut permission_prompter));
        match result {
            Ok(summary) => {
                spinner.finish(
                    "✨ Done",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                println!();
                if let Some(event) = summary.auto_compaction {
                    println!(
                        "{}",
                        format_auto_compaction_notice(event.removed_message_count)
                    );
                }
                self.persist_session()?;
                Ok(())
            }
            Err(error) => {
                spinner.fail(
                    "❌ Request failed",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                Err(Box::new(error))
            }
        }
    }

    pub(crate) fn run_turn_with_output(
        &mut self,
        input: &str,
        output_format: super::CliOutputFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match output_format {
            super::CliOutputFormat::Text => self.run_turn(input),
            super::CliOutputFormat::Json => self.run_prompt_json(input),
        }
    }

    fn run_prompt_json(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        let session = self.runtime.session().clone();
        let mut runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            false,
            self.allowed_tools.clone(),
            self.permission_mode,
            self.ui_sender(),
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let summary = runtime.run_turn(input, Some(&mut permission_prompter))?;
        self.runtime = runtime;
        self.persist_session()?;
        println!(
            "{}",
            json!({
                "message": final_assistant_text(&summary),
                "model": self.model,
                "iterations": summary.iterations,
                "auto_compaction": summary.auto_compaction.map(|event| json!({
                    "removed_messages": event.removed_message_count,
                    "notice": format_auto_compaction_notice(event.removed_message_count),
                })),
                "tool_uses": collect_tool_uses(&summary),
                "tool_results": collect_tool_results(&summary),
                "usage": {
                    "input_tokens": summary.usage.input_tokens,
                    "output_tokens": summary.usage.output_tokens,
                    "cache_creation_input_tokens": summary.usage.cache_creation_input_tokens,
                    "cache_read_input_tokens": summary.usage.cache_read_input_tokens,
                }
            })
        );
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn handle_repl_command(
        &mut self,
        command: SlashCommand,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(match command {
            SlashCommand::Help => {
                println!("{}", render_repl_help());
                false
            }
            SlashCommand::Status => {
                println!("{}", self.status_report()?);
                false
            }
            SlashCommand::Debug => {
                println!("{}", self.debug_tool_call_report()?);
                false
            }
            SlashCommand::Compact => {
                let result = self.compact_command()?;
                println!("{}", result.message);
                result.persist_after
            }
            SlashCommand::Model { model } => {
                let result = self.model_command(model)?;
                println!("{}", result.message);
                result.persist_after
            }
            SlashCommand::Permissions { mode } => {
                let result = self.permissions_command(mode)?;
                println!("{}", result.message);
                result.persist_after
            }
            SlashCommand::Clear { confirm } => {
                let result = self.clear_session_command(confirm)?;
                println!("{}", result.message);
                result.persist_after
            }
            SlashCommand::Cost => {
                println!("{}", self.cost_report());
                false
            }
            SlashCommand::Resume { session_path } => {
                let result = self.resume_session_command(session_path)?;
                println!("{}", result.message);
                result.persist_after
            }
            SlashCommand::Config { section } => {
                println!("{}", Self::config_report(section.as_deref())?);
                false
            }
            SlashCommand::Version => {
                println!("{}", Self::version_report());
                false
            }
            SlashCommand::Export { path } => {
                println!("{}", self.export_session_report(path.as_deref())?);
                false
            }
            SlashCommand::Session { action, target } => {
                let result = self.session_command(action.as_deref(), target.as_deref())?;
                println!("{}", result.message);
                result.persist_after
            }
            SlashCommand::Auth { provider } => {
                self.run_auth(provider.as_deref())?;
                false
            }
            SlashCommand::Headed => {
                env::set_var("HEADLESS", "false");
                self.reset_browser();
                println!("Browser mode\n  Result           switched to headed (visible)");
                false
            }
            SlashCommand::Headless => {
                env::set_var("HEADLESS", "true");
                self.reset_browser();
                println!("Browser mode\n  Result           switched to headless");
                false
            }
            SlashCommand::Unknown(name) => {
                eprintln!("unknown slash command: /{name}");
                false
            }
        })
    }

    pub(crate) fn persist_session(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime.session().save_to_path(&self.session.path)?;
        Ok(())
    }

    pub(crate) fn reset_browser(&mut self) {
        self.runtime.tool_executor_mut().reset_browser();
    }

    pub(crate) fn status_report(&self) -> Result<String, Box<dyn std::error::Error>> {
        let cumulative = self.runtime.usage().cumulative_usage();
        let latest = self.runtime.usage().current_turn_usage();
        Ok(format_status_report(
            &self.model,
            StatusUsage {
                message_count: self.runtime.session().messages.len(),
                turns: self.runtime.usage().turns(),
                latest,
                cumulative,
                estimated_tokens: self.runtime.estimated_tokens(),
            },
            self.permission_mode.as_str(),
            &status_context(Some(&self.session.path))?,
        ))
    }

    pub(crate) fn model_command(
        &mut self,
        model: Option<String>,
    ) -> Result<CommandUiResult, Box<dyn std::error::Error>> {
        let Some(model) = model else {
            return Ok(CommandUiResult {
                message: format_model_report(
                    &self.model,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                ),
                persist_after: false,
            });
        };
        let model = resolve_model_alias(&model).to_string();
        if model == self.model {
            return Ok(CommandUiResult {
                message: format_model_report(
                    &self.model,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                ),
                persist_after: false,
            });
        }
        let previous = self.model.clone();
        let session = self.runtime.session().clone();
        let message_count = session.messages.len();
        self.runtime = build_runtime(
            session,
            model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            self.ui_sender(),
        )?;
        self.model.clone_from(&model);
        Ok(CommandUiResult {
            message: format_model_switch_report(&previous, &model, message_count),
            persist_after: true,
        })
    }

    pub(crate) fn permissions_command(
        &mut self,
        mode: Option<String>,
    ) -> Result<CommandUiResult, Box<dyn std::error::Error>> {
        let Some(mode) = mode else {
            return Ok(CommandUiResult {
                message: format_permissions_report(self.permission_mode.as_str()),
                persist_after: false,
            });
        };
        let normalized = normalize_permission_mode(&mode).ok_or_else(|| {
            format!("unsupported permission mode '{mode}'. Use read-only, workspace-write, or danger-full-access.")
        })?;
        if normalized == self.permission_mode.as_str() {
            return Ok(CommandUiResult {
                message: format_permissions_report(normalized),
                persist_after: false,
            });
        }
        let previous = self.permission_mode.as_str().to_string();
        let session = self.runtime.session().clone();
        self.permission_mode = permission_mode_from_label(normalized);
        self.runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            self.ui_sender(),
        )?;
        Ok(CommandUiResult {
            message: format_permissions_switch_report(&previous, normalized),
            persist_after: true,
        })
    }

    pub(crate) fn clear_session_command(
        &mut self,
        confirm: bool,
    ) -> Result<CommandUiResult, Box<dyn std::error::Error>> {
        if !confirm {
            return Ok(CommandUiResult {
                message:
                    "clear: confirmation required; run /clear --confirm to start a fresh session."
                        .to_string(),
                persist_after: false,
            });
        }
        self.session = create_managed_session_handle()?;
        self.runtime = build_runtime(
            Session::new(),
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            self.ui_sender(),
        )?;
        Ok(CommandUiResult {
            message: format!(
                "Session cleared\n  Mode             fresh session\n  Preserved model  {}\n  Permission mode  {}\n  Session          {}",
                self.model,
                self.permission_mode.as_str(),
                self.session.id
            ),
            persist_after: true,
        })
    }

    pub(crate) fn cost_report(&self) -> String {
        format_cost_report(self.runtime.usage().cumulative_usage())
    }

    pub(crate) fn resume_session_command(
        &mut self,
        session_path: Option<String>,
    ) -> Result<CommandUiResult, Box<dyn std::error::Error>> {
        let Some(session_ref) = session_path else {
            return Ok(CommandUiResult {
                message: "Usage: /resume <session-path>".to_string(),
                persist_after: false,
            });
        };
        let handle = resolve_session_reference(&session_ref)?;
        let session = Session::load_from_path(&handle.path)?;
        let message_count = session.messages.len();
        let model = session.model.clone().unwrap_or_else(|| self.model.clone());
        self.runtime = build_runtime(
            session,
            model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            self.ui_sender(),
        )?;
        self.model = model;
        self.session = handle;
        Ok(CommandUiResult {
            message: format_resume_report(
                &self.session.path.display().to_string(),
                message_count,
                self.runtime.usage().turns(),
            ),
            persist_after: true,
        })
    }

    pub(crate) fn config_report(
        section: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        render_config_report(section)
    }
    pub(crate) fn version_report() -> String {
        render_version_report()
    }

    pub(crate) fn export_session_report(
        &self,
        requested_path: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let export_path = resolve_export_path(requested_path, self.runtime.session())?;
        fs::write(&export_path, render_export_text(self.runtime.session()))?;
        Ok(format!(
            "Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}",
            export_path.display(),
            self.runtime.session().messages.len()
        ))
    }

    pub(crate) fn session_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<CommandUiResult, Box<dyn std::error::Error>> {
        match action {
            None | Some("list") => Ok(CommandUiResult {
                message: render_session_list(&self.session.id)?,
                persist_after: false,
            }),
            Some("switch") => {
                let Some(target) = target else {
                    return Ok(CommandUiResult {
                        message: "Usage: /session switch <session-id>".to_string(),
                        persist_after: false,
                    });
                };
                let handle = resolve_session_reference(target)?;
                let session = Session::load_from_path(&handle.path)?;
                let message_count = session.messages.len();
                let model = session
                    .model
                    .clone()
                    .unwrap_or_else(|| self.model.clone());
                self.runtime = build_runtime(
                    session,
                    model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    self.ui_sender(),
                )?;
                self.model = model;
                self.session = handle;
                Ok(CommandUiResult {
                    message: format!(
                        "Session switched\n  Active session   {}\n  File             {}\n  Messages         {}",
                        self.session.id,
                        self.session.path.display(),
                        message_count
                    ),
                    persist_after: true,
                })
            }
            Some(other) => Ok(CommandUiResult {
                message: format!(
                    "Unknown /session action '{other}'. Use /session list or /session switch <session-id>."
                ),
                persist_after: false,
            }),
        }
    }

    fn run_auth(&mut self, provider: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let target = match provider {
            Some(p) => parse_provider_arg(p)?,
            None => prompt_provider_choice()?,
        };
        interactive_login_prompt(target)?;
        self.refresh_runtime_auth()?;
        println!(
            "Auth\n  Provider         {}\n  Result           authenticated",
            provider_label(target)
        );
        Ok(())
    }

    pub(crate) fn refresh_runtime_auth(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let session = self.runtime.session().clone();
        self.runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            self.ui_sender(),
        )?;
        Ok(())
    }

    pub(crate) fn compact_command(
        &mut self,
    ) -> Result<CommandUiResult, Box<dyn std::error::Error>> {
        let result = self.runtime.compact(CompactionConfig::default());
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let skipped = removed == 0;
        self.runtime = build_runtime(
            result.compacted_session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            self.ui_sender(),
        )?;
        self.persist_session()?;
        Ok(CommandUiResult {
            message: format_compact_report(removed, kept, skipped),
            persist_after: false,
        })
    }

    pub(crate) fn debug_tool_call_report(&self) -> Result<String, Box<dyn std::error::Error>> {
        render_last_tool_debug_report(self.runtime.session())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResumeCommandOutcome {
    pub(crate) session: Session,
    pub(crate) message: Option<String>,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn run_resume_command(
    session_path: &Path,
    session: &Session,
    command: &SlashCommand,
) -> Result<ResumeCommandOutcome, Box<dyn std::error::Error>> {
    match command {
        SlashCommand::Help => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_repl_help()),
        }),
        SlashCommand::Compact => {
            let result = runtime::compact_session(
                session,
                CompactionConfig {
                    max_estimated_tokens: 0,
                    ..CompactionConfig::default()
                },
            );
            let removed = result.removed_message_count;
            let kept = result.compacted_session.messages.len();
            let skipped = removed == 0;
            result.compacted_session.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: result.compacted_session,
                message: Some(format_compact_report(removed, kept, skipped)),
            })
        }
        SlashCommand::Clear { confirm } => {
            if !confirm {
                return Ok(ResumeCommandOutcome {
                    session: session.clone(),
                    message: Some(
                        "clear: confirmation required; rerun with /clear --confirm".to_string(),
                    ),
                });
            }
            let cleared = Session::new();
            cleared.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: cleared,
                message: Some(format!(
                    "Cleared resumed session file {}.",
                    session_path.display()
                )),
            })
        }
        SlashCommand::Status => {
            let tracker = runtime::UsageTracker::from_session(session);
            let usage = tracker.cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_status_report(
                    session.model.as_deref().unwrap_or("unknown"),
                    StatusUsage {
                        message_count: session.messages.len(),
                        turns: tracker.turns(),
                        latest: tracker.current_turn_usage(),
                        cumulative: usage,
                        estimated_tokens: 0,
                    },
                    default_permission_mode().as_str(),
                    &status_context(Some(session_path))?,
                )),
            })
        }
        SlashCommand::Cost => {
            let usage = runtime::UsageTracker::from_session(session).cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_cost_report(usage)),
            })
        }
        SlashCommand::Config { section } => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_config_report(section.as_deref())?),
        }),
        SlashCommand::Version => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_version_report()),
        }),
        SlashCommand::Export { path } => {
            let export_path = resolve_export_path(path.as_deref(), session)?;
            fs::write(&export_path, render_export_text(session))?;
            Ok(ResumeCommandOutcome { session: session.clone(), message: Some(format!("Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}", export_path.display(), session.messages.len())) })
        }
        SlashCommand::Debug
        | SlashCommand::Resume { .. }
        | SlashCommand::Model { .. }
        | SlashCommand::Permissions { .. }
        | SlashCommand::Session { .. }
        | SlashCommand::Auth { .. }
        | SlashCommand::Headed
        | SlashCommand::Headless
        | SlashCommand::Unknown(_) => Err("unsupported resumed slash command".into()),
    }
}

pub(crate) fn run_login() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let config = ConfigLoader::default_for(&cwd).load()?;
    let default_oauth = default_oauth_config();
    let oauth = config.oauth().unwrap_or(&default_oauth);
    let callback_port = oauth.callback_port.unwrap_or(DEFAULT_OAUTH_CALLBACK_PORT);
    let redirect_uri = runtime::loopback_redirect_uri(callback_port);
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
    let callback = wait_for_oauth_callback(callback_port)?;
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

pub(crate) fn run_logout() -> Result<(), Box<dyn std::error::Error>> {
    clear_oauth_credentials()?;
    println!("OAuth credentials cleared.");
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

fn wait_for_oauth_callback(
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
    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
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

fn build_system_prompt() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut sections = crawler::prompt::build_system_prompt(&mvp_tool_specs());
    sections.extend(load_system_prompt(
        env::current_dir()?,
        DEFAULT_DATE,
        env::consts::OS,
        "unknown",
    )?);
    Ok(sections)
}

fn build_runtime_feature_config(
) -> Result<runtime::RuntimeFeatureConfig, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    Ok(ConfigLoader::default_for(cwd)
        .load()?
        .feature_config()
        .clone())
}

#[allow(clippy::too_many_arguments)]
fn build_runtime(
    mut session: Session,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
) -> Result<ConversationRuntime<LlmRuntimeClient, CliToolExecutor>, Box<dyn std::error::Error>> {
    session.model = Some(model.clone());
    Ok(ConversationRuntime::new_with_features(
        session,
        LlmRuntimeClient::new(
            model,
            enable_tools,
            emit_output,
            allowed_tools.clone(),
            ui_tx.clone(),
        )?,
        CliToolExecutor::new(allowed_tools, emit_output, ui_tx),
        permission_policy(permission_mode),
        system_prompt,
        &build_runtime_feature_config()?,
    ))
}

pub(crate) struct ChannelPermissionPrompter {
    ui_tx: mpsc::Sender<ReplTuiEvent>,
}

impl ChannelPermissionPrompter {
    pub(crate) fn new(ui_tx: mpsc::Sender<ReplTuiEvent>) -> Self {
        Self { ui_tx }
    }
}

impl PermissionPrompter for ChannelPermissionPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        let (tx, rx) = mpsc::channel();
        let _ = self.ui_tx.send(ReplTuiEvent::PermissionNeeded {
            request: request.clone(),
            respond: tx,
        });
        rx.recv().unwrap_or(PermissionPromptDecision::Deny {
            reason: "permission UI closed".into(),
        })
    }
}

struct CliPermissionPrompter {
    current_mode: PermissionMode,
}
impl CliPermissionPrompter {
    fn new(current_mode: PermissionMode) -> Self {
        Self { current_mode }
    }
}
impl runtime::PermissionPrompter for CliPermissionPrompter {
    fn decide(
        &mut self,
        request: &runtime::PermissionRequest,
    ) -> runtime::PermissionPromptDecision {
        println!();
        println!("Permission approval required");
        println!("  Tool             {}", request.tool_name);
        println!("  Current mode     {}", self.current_mode.as_str());
        println!("  Required mode    {}", request.required_mode.as_str());
        println!("  Input            {}", request.input);
        print!("Approve this tool call? [y/N]: ");
        let _ = io::stdout().flush();
        let mut response = String::new();
        match io::stdin().read_line(&mut response) {
            Ok(_) => {
                let normalized = response.trim().to_ascii_lowercase();
                if matches!(normalized.as_str(), "y" | "yes") {
                    runtime::PermissionPromptDecision::Allow
                } else {
                    runtime::PermissionPromptDecision::Deny {
                        reason: format!(
                            "tool '{}' denied by user approval prompt",
                            request.tool_name
                        ),
                    }
                }
            }
            Err(error) => runtime::PermissionPromptDecision::Deny {
                reason: format!("permission approval failed: {error}"),
            },
        }
    }
}

pub(crate) struct LlmRuntimeClient {
    runtime: tokio::runtime::Runtime,
    provider: LlmProvider,
    missing_auth_message: Option<String>,
    model: String,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
}

enum LlmProvider {
    Anthropic(AnthropicClient),
    OpenAi(OpenAiResponsesClient),
    Other(ChatCompletionsClient),
}

impl LlmRuntimeClient {
    fn new(
        model: String,
        enable_tools: bool,
        emit_output: bool,
        allowed_tools: Option<AllowedToolSet>,
        ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let store = api::load_credentials().unwrap_or_default();
        let (provider, missing_auth_message) = match provider_for_model(&model) {
            Provider::Anthropic => {
                let auth = store
                    .providers
                    .get("anthropic")
                    .map(credential_config_to_auth_source)
                    .filter(|auth| !matches!(auth, AuthSource::None))
                    .or_else(auth_source_from_anthropic_env);
                match auth {
                    Some(auth) => (
                        LlmProvider::Anthropic(AnthropicClient::from_auth(auth)),
                        None,
                    ),
                    None => (
                        LlmProvider::Anthropic(AnthropicClient::from_auth(AuthSource::None)),
                        Some(
                            "No Anthropic credentials found. Configure via /auth or run `acrawl auth anthropic`."
                                .to_string(),
                        ),
                    ),
                }
            }
            Provider::OpenAi => {
                let auth = store
                    .providers
                    .get("openai")
                    .map(credential_config_to_auth_source)
                    .filter(|auth| !matches!(auth, AuthSource::None))
                    .or_else(auth_source_from_openai_env);
                match auth {
                    Some(auth) => (
                        LlmProvider::OpenAi(OpenAiResponsesClient::new(auth, &model)),
                        None,
                    ),
                    None => (
                        LlmProvider::OpenAi(OpenAiResponsesClient::new(AuthSource::None, &model)),
                        Some(
                            "No OpenAI credentials found. Configure via /auth or run `acrawl auth openai`."
                                .to_string(),
                        ),
                    ),
                }
            }
            Provider::Other => {
                let default_base_url = "http://localhost:11434/v1".to_string();
                let (auth, base_url) = store
                    .providers
                    .get("other")
                    .map(|config| {
                        (
                            credential_config_to_auth_source(config),
                            config
                                .base_url
                                .clone()
                                .unwrap_or_else(|| default_base_url.clone()),
                        )
                    })
                    .unwrap_or((AuthSource::None, default_base_url));
                (
                    LlmProvider::Other(
                        ChatCompletionsClient::with_no_auth(&model, &base_url)
                            .with_optional_auth(auth),
                    ),
                    None,
                )
            }
        };
        Ok(Self {
            runtime: tokio::runtime::Runtime::new()?,
            provider,
            missing_auth_message,
            model,
            enable_tools,
            emit_output,
            allowed_tools,
            ui_tx,
        })
    }

    fn send_ui_stream(&self, chunk: impl Into<String>) {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(ReplTuiEvent::StreamAnsi(chunk.into()));
        }
    }
}

fn credential_config_to_auth_source(config: &api::StoredProviderConfig) -> AuthSource {
    if config.auth_method == "oauth" {
        if let Some(oauth) = &config.oauth {
            return AuthSource::BearerToken(oauth.access_token.clone());
        }
    }
    if let Some(key) = &config.api_key {
        if !key.is_empty() {
            if config.auth_method == "oauth"
                || matches!(config.auth_method.as_str(), "openai" | "openai_key")
            {
                return AuthSource::BearerToken(key.clone());
            }
            return AuthSource::ApiKey(key.clone());
        }
    }
    AuthSource::None
}

fn read_env_non_empty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn auth_source_from_anthropic_env() -> Option<AuthSource> {
    let api_key = read_env_non_empty("ANTHROPIC_API_KEY");
    let auth_token = read_env_non_empty("ANTHROPIC_AUTH_TOKEN");
    match (api_key, auth_token) {
        (Some(api_key), Some(bearer_token)) => Some(AuthSource::ApiKeyAndBearer {
            api_key,
            bearer_token,
        }),
        (Some(api_key), None) => Some(AuthSource::ApiKey(api_key)),
        (None, Some(bearer_token)) => Some(AuthSource::BearerToken(bearer_token)),
        (None, None) => None,
    }
}

fn auth_source_from_openai_env() -> Option<AuthSource> {
    read_env_non_empty("OPENAI_API_KEY").map(AuthSource::BearerToken)
}

#[allow(dead_code)]
fn load_oauth_config_from_cwd() -> Result<Option<OAuthConfig>, api::ApiError> {
    let cwd = env::current_dir().map_err(api::ApiError::from)?;
    let config = ConfigLoader::default_for(&cwd).load().map_err(|error| {
        api::ApiError::Auth(format!("failed to load runtime OAuth config: {error}"))
    })?;
    Ok(config.oauth().cloned())
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

fn prompt_provider_choice() -> Result<Provider, Box<dyn std::error::Error>> {
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

pub(crate) fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenAi => "openai",
        Provider::Other => "other",
    }
}

fn run_auth_for_provider(provider: Provider) -> Result<(), Box<dyn std::error::Error>> {
    match provider {
        Provider::Anthropic => {
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
                        expires_at: oauth.expires_at.and_then(|value| i64::try_from(value).ok()),
                        scopes: oauth.scopes,
                    }),
                    ..Default::default()
                },
            )
        }
        Provider::OpenAi => {
            eprint!("Paste your OpenAI API key (sk-...): ");
            io::stderr().flush()?;
            let mut key = String::new();
            io::stdin().read_line(&mut key)?;
            let key = key.trim().to_string();
            if key.is_empty() {
                return Err("OPENAI_API_KEY is required for OpenAI models".into());
            }
            persist_provider_credentials(
                Provider::OpenAi,
                api::StoredProviderConfig {
                    auth_method: "openai_key".to_string(),
                    api_key: Some(key),
                    ..Default::default()
                },
            )
        }
        Provider::Other => {
            let existing = api::load_credentials()
                .unwrap_or_default()
                .providers
                .get("other")
                .cloned()
                .unwrap_or_default();
            eprint!(
                "Base URL [{}]: ",
                existing
                    .base_url
                    .as_deref()
                    .unwrap_or("http://localhost:11434/v1")
            );
            io::stderr().flush()?;
            let mut base_url = String::new();
            io::stdin().read_line(&mut base_url)?;
            let base_url = match base_url.trim() {
                "" => existing
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434/v1".to_string()),
                value => value.to_string(),
            };

            eprint!("API key (optional, press Enter to skip): ");
            io::stderr().flush()?;
            let mut key = String::new();
            io::stdin().read_line(&mut key)?;
            let key = key.trim().to_string();

            persist_provider_credentials(
                Provider::Other,
                api::StoredProviderConfig {
                    auth_method: if key.is_empty() {
                        "none".to_string()
                    } else {
                        "api_key".to_string()
                    },
                    api_key: (!key.is_empty()).then_some(key),
                    base_url: Some(base_url),
                    default_model: existing.default_model,
                    ..Default::default()
                },
            )
        }
    }
}

fn interactive_login_prompt(provider: Provider) -> Result<(), Box<dyn std::error::Error>> {
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

impl ApiClient for LlmRuntimeClient {
    #[allow(clippy::too_many_lines)]
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if let Some(message) = &self.missing_auth_message {
            return Err(RuntimeError::new(message.clone()));
        }
        let message_request = MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens_for_model(&self.model),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: self.enable_tools.then(|| {
                filter_tool_specs(self.allowed_tools.as_ref())
                    .into_iter()
                    .map(|spec| ToolDefinition {
                        name: spec.name.to_string(),
                        description: Some(spec.description.to_string()),
                        input_schema: spec.input_schema,
                    })
                    .collect()
            }),
            tool_choice: self.enable_tools.then_some(ToolChoice::Auto),
            stream: true,
        };
        self.runtime.block_on(async {
            let mut stdout = io::stdout();
            let mut sink = io::sink();
            let out: &mut dyn Write = if self.emit_output {
                &mut stdout
            } else {
                &mut sink
            };
            let renderer = TerminalRenderer::new();
            let mut markdown_stream = MarkdownStreamState::default();
            let mut events = Vec::new();
            let mut pending_tool: Option<(String, String, String)> = None;
            let mut saw_stop = false;

            let mut unified = match &self.provider {
                LlmProvider::Anthropic(client) => {
                    let s = client
                        .stream_message(&message_request)
                        .await
                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                    UnifiedStream::Anthropic(s)
                }
                LlmProvider::OpenAi(client) => {
                    let s = client
                        .stream_message(&message_request)
                        .await
                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                    UnifiedStream::OpenAi(s)
                }
                LlmProvider::Other(client) => {
                    let s = client
                        .stream_message(&message_request)
                        .await
                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                    UnifiedStream::Other(s)
                }
            };

            while let Some(event) = unified
                .next_event()
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?
            {
                match event {
                    ApiStreamEvent::MessageStart(start) => {
                        for block in start.message.content {
                            push_output_block(
                                block,
                                out,
                                &mut events,
                                &mut pending_tool,
                                true,
                                self.ui_tx.as_ref(),
                            )?;
                        }
                    }
                    ApiStreamEvent::ContentBlockStart(start) => {
                        push_output_block(
                            start.content_block,
                            out,
                            &mut events,
                            &mut pending_tool,
                            true,
                            self.ui_tx.as_ref(),
                        )?;
                    }
                    ApiStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                        ContentBlockDelta::TextDelta { text } => {
                            if !text.is_empty() {
                                // Send raw delta to TUI immediately for zero-lag typewriter
                                self.send_ui_stream(&text);

                                if let Some(rendered) = markdown_stream.push(&renderer, &text) {
                                    write!(out, "{rendered}")
                                        .and_then(|()| out.flush())
                                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                                }
                                events.push(AssistantEvent::TextDelta(text));
                            }
                        }
                        ContentBlockDelta::InputJsonDelta { partial_json } => {
                            if let Some((_, _, input)) = &mut pending_tool {
                                input.push_str(&partial_json);
                            }
                        }
                    },
                    ApiStreamEvent::ContentBlockStop(_) => {
                        if let Some(rendered) = markdown_stream.flush(&renderer) {
                            write!(out, "{rendered}")
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                            self.send_ui_stream(rendered);
                        }
                        if let Some((id, name, input)) = pending_tool.take() {
                            let input = if input.is_empty() {
                                "{}".to_string()
                            } else {
                                input
                            };
                            let tool_banner = format_tool_call_start(&name, &input);
                            writeln!(out, "\n{tool_banner}")
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                            if let Some(tx) = &self.ui_tx {
                                let _ = tx.send(ReplTuiEvent::ToolCallStart {
                                    name: name.clone(),
                                    input: input.clone(),
                                });
                            } else {
                                self.send_ui_stream(format!("\n{tool_banner}"));
                            }
                            events.push(AssistantEvent::ToolUse { id, name, input });
                        }
                    }
                    ApiStreamEvent::MessageDelta(delta) => {
                        events.push(AssistantEvent::Usage(TokenUsage {
                            input_tokens: delta.usage.input_tokens,
                            output_tokens: delta.usage.output_tokens,
                            cache_creation_input_tokens: 0,
                            cache_read_input_tokens: 0,
                        }));
                    }
                    ApiStreamEvent::MessageStop(_) => {
                        saw_stop = true;
                        if let Some(rendered) = markdown_stream.flush(&renderer) {
                            write!(out, "{rendered}")
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                            self.send_ui_stream(rendered);
                        }
                        events.push(AssistantEvent::MessageStop);
                    }
                }
            }
            if !saw_stop
                && events.iter().any(|event| {
                    matches!(event, AssistantEvent::TextDelta(text) if !text.is_empty())
                        || matches!(event, AssistantEvent::ToolUse { .. })
                })
            {
                events.push(AssistantEvent::MessageStop);
            }
            if events
                .iter()
                .any(|event| matches!(event, AssistantEvent::MessageStop))
            {
                return Ok(events);
            }
            match &self.provider {
                LlmProvider::Anthropic(client) => {
                    let response = client
                        .send_message(&MessageRequest {
                            stream: false,
                            ..message_request.clone()
                        })
                        .await
                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                    response_to_events(response, out, self.ui_tx.as_ref())
                }
                LlmProvider::OpenAi(_) | LlmProvider::Other(_) => Ok(events),
            }
        })
    }
}

enum UnifiedStream {
    Anthropic(api::MessageStream),
    OpenAi(ResponsesMessageStream),
    Other(OpenAiMessageStream),
}

impl UnifiedStream {
    async fn next_event(&mut self) -> Result<Option<ApiStreamEvent>, api::ApiError> {
        match self {
            Self::Anthropic(s) => s.next_event().await,
            Self::OpenAi(s) => s.next_event().await,
            Self::Other(s) => s.next_event().await,
        }
    }
}

#[allow(dead_code)]
fn run_codex_login() -> Result<(), Box<dyn std::error::Error>> {
    let login_request = api::codex_login()?;
    println!("Starting Codex OAuth login...");
    let redirect_uri = &login_request.redirect_uri;
    let port = login_request
        .config
        .callback_port
        .unwrap_or(api::CODEX_CALLBACK_PORT);
    println!("Listening for callback on {redirect_uri}");
    if let Err(error) = open_browser(&login_request.authorization_url) {
        eprintln!("warning: failed to open browser automatically: {error}");
        println!(
            "Open this URL manually:\n{}",
            login_request.authorization_url
        );
    }
    let callback = wait_for_oauth_callback(port)?;
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
    if returned_state != login_request.state {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "oauth state mismatch").into());
    }
    let client = AnthropicClient::from_auth(AuthSource::None);
    let exchange_request = OAuthTokenExchangeRequest::from_config(
        &login_request.config,
        code,
        login_request.state,
        login_request.pkce.verifier,
        login_request.redirect_uri,
    );
    let rt = tokio::runtime::Runtime::new()?;
    let token_set =
        rt.block_on(client.exchange_oauth_code(&login_request.config, &exchange_request))?;
    api::save_codex_credentials(&runtime::OAuthTokenSet {
        access_token: token_set.access_token,
        refresh_token: token_set.refresh_token,
        expires_at: token_set.expires_at,
        scopes: token_set.scopes,
    })?;
    println!("Codex OAuth login complete.");
    Ok(())
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

fn persist_provider_credentials(
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

pub(crate) fn final_assistant_text(summary: &runtime::TurnSummary) -> String {
    summary
        .assistant_messages
        .last()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    runtime::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

pub(crate) fn collect_tool_uses(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            runtime::ContentBlock::ToolUse { id, name, input } => {
                Some(json!({"id": id, "name": name, "input": input}))
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn collect_tool_results(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary.tool_results.iter().flat_map(|message| message.blocks.iter()).filter_map(|block| match block { runtime::ContentBlock::ToolResult { tool_use_id, tool_name, output, is_error } => Some(json!({"tool_use_id": tool_use_id, "tool_name": tool_name, "output": output, "is_error": is_error})), _ => None }).collect()
}

pub(crate) fn push_output_block(
    block: OutputContentBlock,
    out: &mut (impl Write + ?Sized),
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
    streaming_tool_input: bool,
    ui_tx: Option<&mpsc::Sender<ReplTuiEvent>>,
) -> Result<(), RuntimeError> {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                let rendered = TerminalRenderer::new().markdown_to_ansi(&text);
                write!(out, "{rendered}")
                    .and_then(|()| out.flush())
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                if let Some(tx) = ui_tx {
                    let _ = tx.send(ReplTuiEvent::StreamAnsi(rendered));
                }
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
                String::new()
            } else {
                input.to_string()
            };
            *pending_tool = Some((id, name, initial_input));
        }
    }
    Ok(())
}

pub(crate) fn response_to_events(
    response: MessageResponse,
    out: &mut (impl Write + ?Sized),
    ui_tx: Option<&mpsc::Sender<ReplTuiEvent>>,
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let mut events = Vec::new();
    let mut pending_tool = None;
    for block in response.content {
        push_output_block(block, out, &mut events, &mut pending_tool, false, ui_tx)?;
        if let Some((id, name, input)) = pending_tool.take() {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }
    events.push(AssistantEvent::Usage(TokenUsage {
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
        cache_read_input_tokens: response.usage.cache_read_input_tokens,
    }));
    events.push(AssistantEvent::MessageStop);
    Ok(events)
}

struct CliToolExecutor {
    renderer: TerminalRenderer,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    agent: CrawlerAgent,
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
}
impl CliToolExecutor {
    fn new(
        allowed_tools: Option<AllowedToolSet>,
        emit_output: bool,
        ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
    ) -> Self {
        Self {
            renderer: TerminalRenderer::new(),
            emit_output,
            allowed_tools,
            agent: CrawlerAgent::new_lazy(ToolRegistry::new_with_core_tools()),
            ui_tx,
        }
    }

    fn reset_browser(&mut self) {
        self.agent.reset_browser();
    }
}
impl ToolExecutor for CliToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if self
            .allowed_tools
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(tool_name))
        {
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is not enabled by the current --allowedTools setting"
            )));
        }
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(ReplTuiEvent::ToolStarting {
                name: tool_name.to_string(),
                input: input.to_string(),
            });
        }

        match self.agent.execute(tool_name, input) {
            Ok(output) => {
                if self.emit_output {
                    let markdown = format_tool_result(tool_name, &output, false);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|error: io::Error| ToolError::new(error.to_string()))?;
                } else if let Some(tx) = &self.ui_tx {
                    let _ = tx.send(ReplTuiEvent::ToolCallComplete {
                        name: tool_name.to_string(),
                        output: output.clone(),
                        is_error: false,
                    });
                }
                Ok(output)
            }
            Err(error) => {
                if self.emit_output {
                    let rendered_error = error.to_string();
                    let markdown = format_tool_result(tool_name, &rendered_error, true);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|stream_error: io::Error| {
                            ToolError::new(stream_error.to_string())
                        })?;
                } else if let Some(tx) = &self.ui_tx {
                    let _ = tx.send(ReplTuiEvent::ToolCallComplete {
                        name: tool_name.to_string(),
                        output: error.to_string(),
                        is_error: true,
                    });
                }
                Err(error)
            }
        }
    }
}

fn permission_policy(mode: PermissionMode) -> PermissionPolicy {
    mvp_tool_specs()
        .into_iter()
        .fold(PermissionPolicy::new(mode), |policy, spec| {
            policy.with_tool_requirement(spec.name, spec.required_permission)
        })
}

pub(crate) fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };
            let content = message
                .blocks
                .iter()
                .map(|block| match block {
                    runtime::ContentBlock::Text { text } => {
                        InputContentBlock::Text { text: text.clone() }
                    }
                    runtime::ContentBlock::ToolUse { id, name, input } => {
                        InputContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: serde_json::from_str(input)
                                .unwrap_or_else(|_| json!({ "raw": input })),
                        }
                    }
                    runtime::ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => InputContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![ToolResultContentBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    },
                    runtime::ContentBlock::Reasoning { data } => {
                        let parsed = serde_json::from_str::<serde_json::Value>(data)
                            .unwrap_or_else(|_| json!({}));
                        InputContentBlock::Reasoning { data: parsed }
                    }
                })
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| InputMessage {
                role: role.to_string(),
                content,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::{MessageResponse, OutputContentBlock, Usage};
    use runtime::{AssistantEvent, ContentBlock, ConversationMessage, MessageRole};
    use serde_json::json;

    #[test]
    fn resolves_known_model_aliases() {
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251213");
        assert_eq!(resolve_model_alias("claude-opus"), "claude-opus");
    }

    #[test]
    fn routes_claude_models_to_anthropic() {
        assert_eq!(provider_for_model("claude-sonnet-4-6"), Provider::Anthropic);
    }

    #[test]
    fn routes_gpt_models_to_openai() {
        assert_eq!(provider_for_model("gpt-4o"), Provider::OpenAi);
    }

    #[test]
    fn routes_codex_models_to_openai() {
        assert_eq!(provider_for_model("codex-mini-latest"), Provider::OpenAi);
    }

    #[test]
    fn routes_o_series_models_to_openai() {
        assert_eq!(provider_for_model("o3"), Provider::OpenAi);
    }

    #[test]
    fn routes_llama_models_to_other() {
        assert_eq!(provider_for_model("llama3.2"), Provider::Other);
    }

    #[test]
    fn routes_qwen_models_to_other() {
        assert_eq!(provider_for_model("qwen2"), Provider::Other);
    }

    #[test]
    fn filtered_tool_specs_respect_allowlist() {
        let allowed = ["read_file", "grep_search"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let filtered = filter_tool_specs(Some(&allowed));
        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert!(names.iter().all(|n| allowed.contains::<str>(n)));
    }

    #[test]
    fn converts_tool_roundtrip_messages() {
        let messages = vec![
            ConversationMessage::user_text("hello"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: "{\"command\":\"pwd\"}".to_string(),
            }]),
            ConversationMessage {
                role: MessageRole::Tool,
                blocks: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    tool_name: "bash".to_string(),
                    output: "ok".to_string(),
                    is_error: false,
                }],
                usage: None,
            },
        ];
        let converted = convert_messages(&messages);
        assert_eq!(converted.len(), 3);
        assert_eq!(converted[1].role, "assistant");
        assert_eq!(converted[2].role, "user");
    }

    #[test]
    fn reasoning_block_converts_to_input_content_block() {
        use api::InputContentBlock;

        let messages = vec![ConversationMessage::assistant(vec![
            ContentBlock::Reasoning {
                data: r#"{"id":"rs_xyz","content":[]}"#.to_string(),
            },
            ContentBlock::Text {
                text: "done".to_string(),
            },
        ])];
        let converted = convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].content.len(), 2);
        match &converted[0].content[0] {
            InputContentBlock::Reasoning { data } => {
                assert_eq!(data["id"], "rs_xyz");
            }
            other => panic!("expected Reasoning, got {other:?}"),
        }
        assert!(matches!(
            &converted[0].content[1],
            InputContentBlock::Text { text } if text == "done"
        ));
    }

    #[test]
    fn push_output_block_renders_markdown_text() {
        let mut out = Vec::new();
        let mut events = Vec::new();
        let mut pending_tool = None;
        push_output_block(
            OutputContentBlock::Text {
                text: "# Heading".to_string(),
            },
            &mut out,
            &mut events,
            &mut pending_tool,
            false,
            None,
        )
        .expect("text block should render");
        let rendered = String::from_utf8(out).expect("utf8");
        assert!(rendered.contains("Heading"));
        assert!(rendered.contains('\u{1b}'));
    }

    #[test]
    fn push_output_block_skips_empty_object_prefix_for_tool_streams() {
        let mut out = Vec::new();
        let mut events = Vec::new();
        let mut pending_tool = None;
        push_output_block(
            OutputContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "read_file".to_string(),
                input: json!({}),
            },
            &mut out,
            &mut events,
            &mut pending_tool,
            true,
            None,
        )
        .expect("tool block should accumulate");
        assert!(events.is_empty());
        assert_eq!(
            pending_tool,
            Some(("tool-1".to_string(), "read_file".to_string(), String::new()))
        );
    }

    #[test]
    fn response_to_events_preserves_empty_object_json_input_outside_streaming() {
        let mut out = Vec::new();
        let events = response_to_events(
            MessageResponse {
                id: "msg-1".to_string(),
                kind: "message".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![OutputContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({}),
                }],
                stop_reason: Some("tool_use".to_string()),
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
                request_id: None,
            },
            &mut out,
            None,
        )
        .expect("response conversion should succeed");
        assert!(
            matches!(&events[0], AssistantEvent::ToolUse { name, input, .. } if name == "read_file" && input == "{}")
        );
    }

    #[test]
    fn response_to_events_preserves_non_empty_json_input_outside_streaming() {
        let mut out = Vec::new();
        let events = response_to_events(
            MessageResponse {
                id: "msg-2".to_string(),
                kind: "message".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![OutputContentBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "read_file".to_string(),
                    input: json!({ "path": "rust/Cargo.toml" }),
                }],
                stop_reason: Some("tool_use".to_string()),
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
                request_id: None,
            },
            &mut out,
            None,
        )
        .expect("response conversion should succeed");
        assert!(
            matches!(&events[0], AssistantEvent::ToolUse { name, input, .. } if name == "read_file" && input == "{\"path\":\"rust/Cargo.toml\"}")
        );
    }

    #[test]
    fn cancellable_callback_stops_on_cancel() {
        let (cancel_tx, cancel_rx) = mpsc::channel();
        cancel_tx.send(()).expect("send cancel signal");

        let handle = std::thread::spawn(move || wait_for_oauth_callback_cancellable(0, cancel_rx));

        let result = handle.join().expect("thread should not panic");
        let err = result.expect_err("should return error on cancel");
        let msg = err.to_string();
        assert!(
            msg.contains("cancelled") || msg.contains("Interrupted"),
            "expected cancellation error, got: {msg}"
        );
    }

    #[test]
    fn cancellable_callback_returns_on_cancel_while_listening() {
        let (cancel_tx, cancel_rx) = mpsc::channel();

        let handle = std::thread::spawn(move || wait_for_oauth_callback_cancellable(0, cancel_rx));

        // Give the listener time to start polling
        std::thread::sleep(std::time::Duration::from_millis(250));
        cancel_tx.send(()).expect("send cancel signal");

        let result = handle.join().expect("thread should not panic");
        let err = result.expect_err("should return error on cancel");
        let msg = err.to_string();
        assert!(
            msg.contains("cancelled") || msg.contains("Interrupted"),
            "expected cancellation error, got: {msg}"
        );
    }
}
