use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::mpsc;

use crate::render::{MarkdownStreamState, Spinner, TerminalRenderer};
use api::{
    provider::{ProviderClient, ProviderRegistry},
    ContentBlockDelta, InputContentBlock, InputMessage, MessageRequest, MessageResponse,
    OutputContentBlock, StreamEvent as ApiStreamEvent, ToolChoice, ToolDefinition,
    ToolResultContentBlock,
};
use commands::{slash_command_specs, SlashCommand};
use crawler::{mvp_tool_specs, CrawlerAgent, SharedApiClient, ToolRegistry};
use runtime::{
    load_settings, load_system_prompt, settings_get_max_steps, ApiClient, ApiRequest,
    AssistantEvent, CompactionConfig, ConfigLoader, ConversationMessage, ConversationRuntime,
    MessageRole, RuntimeError, Session, TokenUsage, ToolError, ToolExecutor,
};
use serde_json::json;

fn block_on_runtime_future<F, T>(future: F) -> Result<T, RuntimeError>
where
    F: std::future::Future<Output = Result<T, RuntimeError>>,
{
    crate::TOKIO_RUNTIME
        .get()
        .ok_or_else(|| RuntimeError::new("tokio runtime not initialized"))?
        .block_on(future)
}

use crate::format::{
    format_auto_compaction_notice, format_compact_report, format_cost_report, format_model_report,
    format_model_switch_report, format_resume_report, format_status_report, render_config_report,
    render_export_text, render_last_tool_debug_report, render_repl_help, render_version_report,
    resolve_export_path, status_context, StatusUsage, DEFAULT_DATE,
};
use crate::input;
use crate::session_mgr::{
    create_managed_session_handle, render_session_list, resolve_session_reference, SessionHandle,
};
use crate::tui::tool_panel::{format_tool_call_start, format_tool_result};
use crate::tui::ReplTuiEvent;

#[path = "auth/mod.rs"]
mod auth;

pub(crate) use self::auth::{
    bind_oauth_listener, default_oauth_config, open_browser, parse_provider_arg, run_auth_cli,
    run_login, run_logout, wait_for_oauth_callback_cancellable,
};
use self::auth::{
    interactive_login_prompt, prompt_provider_choice, provider_choice_label, resolve_provider_arg,
};

pub(crate) type AllowedToolSet = BTreeSet<String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    Anthropic,
    OpenAi,
    Other,
}

pub(crate) fn initial_model_from_credentials() -> Option<String> {
    let settings = runtime::load_settings();
    settings.model.filter(|m| !m.is_empty() && m.contains('/'))
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
) -> Result<(), Box<dyn std::error::Error>> {
    let classic =
        runtime::load_settings().classic_repl.unwrap_or(false) || !io::stdout().is_terminal();
    if classic {
        run_repl_classic(model, allowed_tools)
    } else {
        crate::tui::run_repl_ratatui(model, allowed_tools)
    }
}

fn run_repl_classic(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cli = LiveCli::new(model, true, allowed_tools)?;
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
    system_prompt: Vec<String>,
    runtime: ConversationRuntime<LlmRuntimeClient, CliToolExecutor>,
    session: SessionHandle,
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
    /// `false` when the TUI owns the terminal (raw-mode / alternate screen).
    /// When `false`, LLM streaming and tool output are routed through `ui_tx`
    /// instead of being written directly to stdout.
    emit_output: bool,
    reasoning_effort: Option<api::ReasoningEffort>,
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
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let settings = runtime::load_settings();
        let system_prompt = build_system_prompt()?;
        let session = create_managed_session_handle()?;
        let runtime = build_runtime(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            true,
            allowed_tools.clone(),
            None,
        )?;
        let initial_effort = if model_supports_reasoning(&model) {
            let saved = settings
                .reasoning_effort
                .as_deref()
                .and_then(|effort| api::ReasoningEffort::from_str(effort).ok());
            Some(saved.unwrap_or(api::ReasoningEffort::High))
        } else {
            None
        };
        let mut cli = Self {
            model,
            allowed_tools,
            system_prompt,
            runtime,
            session,
            ui_tx: None,
            emit_output: true,
            reasoning_effort: initial_effort,
        };
        if let Some(effort) = initial_effort {
            cli.runtime.api_client_mut().reasoning_effort = Some(effort);
        }
        cli.persist_session()?;
        Ok(cli)
    }

    pub(crate) fn new_with_ui_tx(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        ui_tx: mpsc::Sender<ReplTuiEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let settings = runtime::load_settings();
        let system_prompt = build_system_prompt()?;
        let session = create_managed_session_handle()?;
        let runtime = build_runtime(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            false,
            allowed_tools.clone(),
            Some(ui_tx.clone()),
        )?;
        let initial_effort = if model_supports_reasoning(&model) {
            let saved = settings
                .reasoning_effort
                .as_deref()
                .and_then(|effort| api::ReasoningEffort::from_str(effort).ok());
            Some(saved.unwrap_or(api::ReasoningEffort::High))
        } else {
            None
        };
        let mut cli = Self {
            model,
            allowed_tools,
            system_prompt,
            runtime,
            session,
            ui_tx: Some(ui_tx),
            emit_output: false,
            reasoning_effort: initial_effort,
        };
        if let Some(effort) = initial_effort {
            cli.runtime.api_client_mut().reasoning_effort = Some(effort);
        }
        cli.persist_session()?;
        Ok(cli)
    }

    pub(crate) fn session_id(&self) -> &str {
        self.session.id.as_str()
    }

    pub(crate) fn model_name(&self) -> &str {
        &self.model
    }

    pub(crate) fn reasoning_effort(&self) -> Option<api::ReasoningEffort> {
        self.reasoning_effort
    }

    pub(crate) fn supports_reasoning(&self) -> bool {
        model_supports_reasoning(&self.model)
    }

    pub(crate) fn cycle_reasoning_effort(&mut self) -> Option<api::ReasoningEffort> {
        if !self.supports_reasoning() {
            return None;
        }
        let available = model_reasoning_efforts(&self.model);
        let next = match self.reasoning_effort {
            Some(e) => e.cycle(&available),
            None => *available.last().unwrap_or(&api::ReasoningEffort::High),
        };
        self.reasoning_effort = Some(next);
        self.runtime.api_client_mut().reasoning_effort = Some(next);
        let effort_str = next.as_str().to_string();
        let _ = runtime::update_settings(|s| {
            s.reasoning_effort = Some(effort_str);
        });
        Some(next)
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

    pub(crate) fn run_turn_tui(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(ReplTuiEvent::TurnStarting);
        }
        let result = block_on_runtime_future(self.runtime.run_turn(input));
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
  \x1b[2mDirectory\x1b[0m        {}\n\
  \x1b[2mSession\x1b[0m          {}\n\n\
  Type \x1b[1m/help\x1b[0m for commands · \x1b[2mShift+Enter\x1b[0m for newline",
            self.model, cwd, self.session.id,
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
        let result = block_on_runtime_future(self.runtime.run_turn(input));
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
            self.ui_sender(),
        )?;
        let summary = block_on_runtime_future(runtime.run_turn(input))?;
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
                let _ = runtime::update_settings(|s| {
                    s.headless = Some(false);
                });
                self.reset_browser();
                println!("Browser mode\n  Result           switched to headed (visible)");
                false
            }
            SlashCommand::Headless => {
                env::set_var("HEADLESS", "true");
                let _ = runtime::update_settings(|s| {
                    s.headless = Some(true);
                });
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
            self.emit_output,
            self.allowed_tools.clone(),
            self.ui_sender(),
        )?;
        self.model.clone_from(&model);
        if model_supports_reasoning(&model) {
            let effort = self.reasoning_effort.unwrap_or(api::ReasoningEffort::High);
            self.reasoning_effort = Some(effort);
            self.runtime.api_client_mut().reasoning_effort = Some(effort);
        } else {
            self.reasoning_effort = None;
        }
        let _ = runtime::update_settings(|s| {
            s.model = Some(model.clone());
            s.reasoning_effort = self.reasoning_effort.map(|e| e.as_str().to_string());
        });
        Ok(CommandUiResult {
            message: format_model_switch_report(&previous, &model, message_count),
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
            self.emit_output,
            self.allowed_tools.clone(),
            self.ui_sender(),
        )?;
        Ok(CommandUiResult {
            message: format!(
                "Session cleared\n  Mode             fresh session\n  Preserved model  {}\n  Session          {}",
                self.model,
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
            self.emit_output,
            self.allowed_tools.clone(),
            self.ui_sender(),
        )?;
        self.model = model;
        let _ = runtime::update_settings(|s| {
            s.model = Some(self.model.clone());
        });
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
                    self.emit_output,
                    self.allowed_tools.clone(),
                    self.ui_sender(),
                )?;
                self.model = model;
                let _ = runtime::update_settings(|s| {
                    s.model = Some(self.model.clone());
                });
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
        let choice = match provider {
            Some(p) => resolve_provider_arg(p)?,
            None => prompt_provider_choice()?,
        };
        let label = provider_choice_label(&choice).to_string();
        interactive_login_prompt(&choice)?;
        let session = self.runtime.session().clone();
        self.runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            self.emit_output,
            self.allowed_tools.clone(),
            self.ui_sender(),
        )?;
        println!("Auth\n  Provider         {label}\n  Result           authenticated");
        Ok(())
    }

    pub(crate) fn refresh_runtime_auth(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let session = self.runtime.session().clone();
        self.runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            self.emit_output,
            self.allowed_tools.clone(),
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
            self.emit_output,
            self.allowed_tools.clone(),
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
        | SlashCommand::Session { .. }
        | SlashCommand::Auth { .. }
        | SlashCommand::Headed
        | SlashCommand::Headless
        | SlashCommand::Unknown(_) => Err("unsupported resumed slash command".into()),
    }
}

fn model_supports_reasoning(model: &str) -> bool {
    let api_model = api::provider::model_api_id(model);
    let store = api::load_credentials().unwrap_or_default();
    let registry = ProviderRegistry::from_credentials(&store);
    if let Some(info) = registry.resolve_model(api_model) {
        return info.capabilities.reasoning;
    }
    models_dev_reasoning_cache()
        .get(api_model)
        .copied()
        .unwrap_or(false)
}

fn model_reasoning_efforts(model: &str) -> Vec<api::ReasoningEffort> {
    let api_model = api::provider::model_api_id(model);
    let store = api::load_credentials().unwrap_or_default();
    let registry = ProviderRegistry::from_credentials(&store);
    if let Some(info) = registry.resolve_model(api_model) {
        return info.capabilities.reasoning_efforts.clone();
    }
    if model_supports_reasoning(model) {
        api::ReasoningEffort::OPENAI.to_vec()
    } else {
        vec![]
    }
}

fn models_dev_reasoning_cache() -> &'static std::collections::HashMap<String, bool> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    static CACHE: OnceLock<HashMap<String, bool>> = OnceLock::new();
    CACHE.get_or_init(|| {
        if let Some(rt) = crate::TOKIO_RUNTIME.get() {
            rt.block_on(api::provider::catalog::fetch_models_dev_reasoning())
                .ok()
        } else {
            tokio::runtime::Runtime::new().ok().and_then(|rt| {
                rt.block_on(api::provider::catalog::fetch_models_dev_reasoning())
                    .ok()
            })
        }
        .unwrap_or_default()
    })
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
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
) -> Result<ConversationRuntime<LlmRuntimeClient, CliToolExecutor>, Box<dyn std::error::Error>> {
    session.model = Some(model.clone());
    let max_steps = settings_get_max_steps(&load_settings()) as usize;
    let fork_client = SharedApiClient::new(LlmRuntimeClient::new(
        model.clone(),
        enable_tools,
        false,
        allowed_tools.clone(),
        None,
    ));
    Ok(ConversationRuntime::new_with_features(
        session,
        LlmRuntimeClient::new(
            model,
            enable_tools,
            emit_output,
            allowed_tools.clone(),
            ui_tx.clone(),
        ),
        CliToolExecutor::new(allowed_tools, emit_output, ui_tx, fork_client),
        system_prompt,
        &build_runtime_feature_config()?,
    )
    .with_max_iterations(max_steps))
}

pub(crate) struct LlmRuntimeClient {
    registry: ProviderRegistry,
    provider: ProviderClient,
    model: String,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
    reasoning_effort: Option<api::ReasoningEffort>,
}

impl LlmRuntimeClient {
    fn new(
        model: String,
        enable_tools: bool,
        emit_output: bool,
        allowed_tools: Option<AllowedToolSet>,
        ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
    ) -> Self {
        let store = api::load_credentials().unwrap_or_default();
        let registry = ProviderRegistry::from_credentials(&store);
        let provider = if model.is_empty() {
            ProviderClient::no_auth_placeholder()
        } else {
            match registry.build_client(&model, &store) {
                Ok(client) => client,
                Err(e) => {
                    eprintln!("Warning: {e}");
                    ProviderClient::no_auth_placeholder()
                }
            }
        };
        Self {
            registry,
            provider,
            model,
            enable_tools,
            emit_output,
            allowed_tools,
            ui_tx,
            reasoning_effort: None,
        }
    }

    fn send_ui_stream(&self, chunk: impl Into<String>) {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(ReplTuiEvent::StreamAnsi(chunk.into()));
        }
    }
}

impl ApiClient for LlmRuntimeClient {
    #[allow(clippy::too_many_lines)]
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let message_request = MessageRequest {
            model: api::provider::model_api_id(&self.model).to_string(),
            max_tokens: self.registry.max_tokens(&self.model),
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
            reasoning_effort: self.reasoning_effort,
        };
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
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

                let mut stream = self
                    .provider
                    .stream_message(&message_request)
                    .await
                    .map_err(|error| RuntimeError::new(error.to_string()))?;

                while let Some(event) = stream
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
                                            .map_err(|error| {
                                                RuntimeError::new(error.to_string())
                                            })?;
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
                if self.provider.supports_send_message() {
                    let response = self
                        .provider
                        .send_message(&MessageRequest {
                            stream: false,
                            ..message_request.clone()
                        })
                        .await
                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                    response_to_events(response, out, self.ui_tx.as_ref())
                } else {
                    Ok(events)
                }
            })
        })
    }
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
        fork_client: SharedApiClient,
    ) -> Self {
        Self {
            renderer: TerminalRenderer::new(),
            emit_output,
            allowed_tools,
            agent: CrawlerAgent::new_lazy(ToolRegistry::new_with_core_tools())
                .with_api_client(fork_client),
            ui_tx,
        }
    }

    fn reset_browser(&mut self) {
        self.agent.reset_browser();
    }
}
impl ToolExecutor for CliToolExecutor {
    async fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
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

        match self.agent.execute(tool_name, input).await {
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
    fn resolves_known_models_by_id() {
        let registry = ProviderRegistry::from_credentials(&api::CredentialStore::default());
        assert!(registry.resolve_model("claude-opus-4-6").is_some());
        assert!(registry.resolve_model("claude-sonnet-4-6").is_some());
        assert!(registry
            .resolve_model("claude-haiku-4-5-20251213")
            .is_some());
        assert!(registry.resolve_model("not-a-real-model").is_none());
    }

    #[test]
    fn provider_for_model_requires_provider_prefix() {
        let registry = ProviderRegistry::from_credentials(&api::CredentialStore::default());
        assert!(registry.provider_for_model("claude-sonnet-4-6").is_err());
    }

    #[test]
    fn provider_for_model_accepts_prefixed_model() {
        let registry = ProviderRegistry::from_credentials(&api::CredentialStore::default());
        assert_eq!(
            registry
                .provider_for_model("anthropic/claude-sonnet-4-6")
                .unwrap(),
            "anthropic"
        );
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

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
        let handle =
            std::thread::spawn(move || wait_for_oauth_callback_cancellable(listener, cancel_rx));

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

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
        let handle =
            std::thread::spawn(move || wait_for_oauth_callback_cancellable(listener, cancel_rx));

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

    #[test]
    fn model_supports_reasoning_for_catalog_models() {
        assert!(model_supports_reasoning("o3"));
        assert!(model_supports_reasoning("o4-mini"));
        assert!(model_supports_reasoning("codex-mini-latest"));
    }

    #[test]
    fn model_supports_reasoning_false_for_non_reasoning_models() {
        assert!(!model_supports_reasoning("gpt-4o"));
        assert!(!model_supports_reasoning("claude-sonnet-4-6"));
    }

    #[test]
    fn model_supports_reasoning_for_unknown_reasoning_models() {
        assert!(
            model_supports_reasoning("gpt-5.3-codex"),
            "gpt-5.3-codex should be detected as a reasoning model"
        );
    }
}
