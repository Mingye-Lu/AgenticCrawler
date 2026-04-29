mod api_client;
mod classic_repl;
mod model_support;
mod resume;
mod runtime_builder;
mod tool_executor;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::str::FromStr;
use std::sync::mpsc;

use crate::error::CliError;
use crate::format::{
    format_auto_compaction_notice, format_compact_report, format_cost_report, format_model_report,
    format_model_switch_report, format_resume_report, format_status_report, render_config_report,
    render_export_text, render_repl_help, render_version_report, resolve_export_path,
    status_context, StatusUsage, DEFAULT_DATE,
};
use crate::markdown::{Spinner, TerminalRenderer};
use crate::output_sink::{ChannelSink, OutputSink, StdoutSink};
use crate::session_mgr::{
    create_managed_session_handle, render_session_list, resolve_session_reference, SessionHandle,
};
use crate::tui::ReplTuiEvent;
use commands::{slash_command_specs, SlashCommand};
use crawler::mvp_tool_specs;
use runtime::{CompactionConfig, ConversationRuntime, RuntimeError, Session, TokenUsage};
use serde_json::json;

#[cfg(test)]
use api::provider::ProviderRegistry;

use self::api_client::LlmRuntimeClient;
#[cfg(test)]
use self::api_client::{convert_messages, push_output_block, response_to_events};
use self::model_support::{model_reasoning_efforts, model_supports_reasoning};
use self::runtime_builder::{build_runtime, build_system_prompt};
use self::tool_executor::CliToolExecutor;

pub(crate) use crate::auth::{
    bind_oauth_listener, default_oauth_config, open_browser, parse_provider_arg, run_auth_cli,
    run_login, run_logout, wait_for_oauth_callback_cancellable,
};
use crate::auth::{
    interactive_login_prompt, prompt_provider_choice, provider_choice_label, resolve_provider_arg,
};

pub(crate) use classic_repl::run_repl_classic;
pub(crate) use resume::run_resume_command;

fn block_on_runtime_future<F, T>(future: F) -> Result<T, RuntimeError>
where
    F: std::future::Future<Output = Result<T, RuntimeError>>,
{
    crate::TOKIO_RUNTIME
        .get()
        .ok_or_else(|| RuntimeError::new("tokio runtime not initialized"))?
        .block_on(future)
}

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
) -> Result<(), CliError> {
    let classic =
        runtime::load_settings().classic_repl.unwrap_or(false) || !io::stdout().is_terminal();
    if classic {
        run_repl_classic(model, allowed_tools)
    } else {
        Ok(crate::tui::run_repl_ratatui(model, allowed_tools)?)
    }
}

pub(crate) struct LiveCli {
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    system_prompt: Vec<String>,
    runtime: ConversationRuntime<LlmRuntimeClient, CliToolExecutor>,
    session: SessionHandle,
    output_mode: OutputMode,
    reasoning_effort: Option<api::ReasoningEffort>,
    debug_mode: bool,
}

#[derive(Clone)]
enum OutputMode {
    Stdout,
    Channel(mpsc::Sender<ReplTuiEvent>),
}

impl OutputMode {
    fn observer(&self) -> Box<dyn runtime::RuntimeObserver + Send> {
        let sink: Box<dyn OutputSink + Send> = match self {
            Self::Stdout => Box::new(StdoutSink::new()),
            Self::Channel(tx) => Box::new(ChannelSink::new(tx.clone())),
        };
        Box::new(sink)
    }

    fn sender(&self) -> Option<mpsc::Sender<ReplTuiEvent>> {
        match self {
            Self::Stdout => None,
            Self::Channel(tx) => Some(tx.clone()),
        }
    }
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
    ) -> Result<Self, CliError> {
        let settings = runtime::load_settings();
        let system_prompt = build_system_prompt()?;
        let session = create_managed_session_handle()?;
        let output_mode = OutputMode::Stdout;
        let runtime = build_runtime(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            allowed_tools.clone(),
            output_mode.observer(),
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
            output_mode,
            reasoning_effort: initial_effort,
            debug_mode: false,
        };
        if let Some(effort) = initial_effort {
            cli.runtime
                .api_client_mut()
                .set_reasoning_effort(Some(effort));
        }
        cli.persist_session()?;
        Ok(cli)
    }

    pub(crate) fn new_with_ui_tx(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        event_tx: mpsc::Sender<ReplTuiEvent>,
    ) -> Result<Self, CliError> {
        let settings = runtime::load_settings();
        let system_prompt = build_system_prompt()?;
        let session = create_managed_session_handle()?;
        let output_mode = OutputMode::Channel(event_tx);
        let runtime = build_runtime(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            allowed_tools.clone(),
            output_mode.observer(),
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
            output_mode,
            reasoning_effort: initial_effort,
            debug_mode: false,
        };
        if let Some(effort) = initial_effort {
            cli.runtime
                .api_client_mut()
                .set_reasoning_effort(Some(effort));
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
        self.runtime
            .api_client_mut()
            .set_reasoning_effort(Some(next));
        let effort_str = next.as_str().to_string();
        let _ = runtime::update_settings(|s| {
            s.reasoning_effort = Some(effort_str);
        });
        Some(next)
    }

    pub(crate) fn cumulative_usage(&self) -> TokenUsage {
        self.runtime.usage().cumulative_usage()
    }

    fn event_sender(&self) -> Option<mpsc::Sender<ReplTuiEvent>> {
        self.output_mode.sender()
    }

    pub(crate) fn cancel_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.runtime.cancel_flag()
    }

    pub(crate) fn run_turn_tui(&mut self, input: &str) -> Result<(), CliError> {
        if let Some(tx) = self.event_sender() {
            let _ = tx.send(ReplTuiEvent::TurnStarting);
        }
        let result = block_on_runtime_future(self.runtime.run_turn(input));
        let finish: Result<(), String> = match &result {
            Ok(summary) => {
                if let Some(ev) = summary.auto_compaction {
                    let msg = format_auto_compaction_notice(ev.removed_message_count);
                    if let Some(tx) = self.event_sender() {
                        let _ = tx.send(ReplTuiEvent::SystemMessage(msg));
                    }
                }
                self.persist_session().map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        };
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

    pub(crate) fn run_turn(&mut self, input: &str) -> Result<(), CliError> {
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
                Err(CliError::from(error))
            }
        }
    }

    pub(crate) fn run_turn_with_output(
        &mut self,
        input: &str,
        output_format: super::CliOutputFormat,
    ) -> Result<(), CliError> {
        match output_format {
            super::CliOutputFormat::Text => self.run_turn(input),
            super::CliOutputFormat::Json => self.run_prompt_json(input),
        }
    }

    fn run_prompt_json(&mut self, input: &str) -> Result<(), CliError> {
        let session = self.runtime.session().clone();
        let mut runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            self.allowed_tools.clone(),
            self.output_mode.observer(),
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
    pub(crate) fn handle_repl_command(&mut self, command: SlashCommand) -> Result<bool, CliError> {
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
                self.debug_mode = !self.debug_mode;
                let label = if self.debug_mode { "ON" } else { "OFF" };
                println!("Debug mode {label}");
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

    pub(crate) fn persist_session(&self) -> Result<(), CliError> {
        self.runtime.session().save_to_path(&self.session.path)?;
        Ok(())
    }

    pub(crate) fn reset_browser(&mut self) {
        self.runtime.tool_executor_mut().reset_browser();
    }

    pub(crate) fn status_report(&self) -> Result<String, CliError> {
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
    ) -> Result<CommandUiResult, CliError> {
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
            self.allowed_tools.clone(),
            self.output_mode.observer(),
        )?;
        self.model.clone_from(&model);
        if model_supports_reasoning(&model) {
            let effort = self.reasoning_effort.unwrap_or(api::ReasoningEffort::High);
            self.reasoning_effort = Some(effort);
            self.runtime
                .api_client_mut()
                .set_reasoning_effort(Some(effort));
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
    ) -> Result<CommandUiResult, CliError> {
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
            self.allowed_tools.clone(),
            self.output_mode.observer(),
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
    ) -> Result<CommandUiResult, CliError> {
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
            self.allowed_tools.clone(),
            self.output_mode.observer(),
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

    pub(crate) fn config_report(section: Option<&str>) -> Result<String, CliError> {
        Ok(render_config_report(section)?)
    }

    pub(crate) fn version_report() -> String {
        render_version_report()
    }

    pub(crate) fn export_session_report(
        &self,
        requested_path: Option<&str>,
    ) -> Result<String, CliError> {
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
    ) -> Result<CommandUiResult, CliError> {
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
                    self.allowed_tools.clone(),
                    self.output_mode.observer(),
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

    fn run_auth(&mut self, provider: Option<&str>) -> Result<(), CliError> {
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
            self.allowed_tools.clone(),
            self.output_mode.observer(),
        )?;
        println!("Auth\n  Provider         {label}\n  Result           authenticated");
        Ok(())
    }

    pub(crate) fn refresh_runtime_auth(&mut self) -> Result<(), CliError> {
        let session = self.runtime.session().clone();
        self.runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            self.allowed_tools.clone(),
            self.output_mode.observer(),
        )?;
        Ok(())
    }

    pub(crate) fn compact_command(&mut self) -> Result<CommandUiResult, CliError> {
        let result = self.runtime.compact(CompactionConfig::default());
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let skipped = removed == 0;
        self.runtime = build_runtime(
            result.compacted_session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            self.allowed_tools.clone(),
            self.output_mode.observer(),
        )?;
        self.persist_session()?;
        Ok(CommandUiResult {
            message: format_compact_report(removed, kept, skipped),
            persist_after: false,
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
    summary
        .tool_results
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            runtime::ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } => Some(json!({
                "tool_use_id": tool_use_id,
                "tool_name": tool_name,
                "output": output,
                "is_error": is_error
            })),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::{MessageResponse, OutputContentBlock, Usage};
    use runtime::{AssistantEvent, ContentBlock, ConversationMessage, MessageRole};
    use serde_json::json;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn with_clean_config_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = test_env_lock();
        let saved_config_home = std::env::var_os("ACRAWL_CONFIG_HOME");
        let temp_dir = std::env::temp_dir().join(format!(
            "app-tests-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).expect("create temp config home");
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        let result = f();
        match saved_config_home {
            Some(value) => std::env::set_var("ACRAWL_CONFIG_HOME", value),
            None => std::env::remove_var("ACRAWL_CONFIG_HOME"),
        }
        fs::remove_dir_all(temp_dir).expect("cleanup temp config home");
        result
    }

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
    fn screenshot_tool_result_produces_image_content_block() {
        use api::InputContentBlock;
        let output = r#"{"screenshot_base64":"iVBORw0KGgo=","size_bytes":42}"#;
        let messages = vec![ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                tool_name: "screenshot".to_string(),
                output: output.to_string(),
                is_error: false,
            }],
            usage: None,
        }];
        let converted = convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        let content = &converted[0].content;
        assert_eq!(content.len(), 1);
        match &content[0] {
            InputContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(!is_error);
                assert_eq!(content.len(), 2);
                match &content[0] {
                    api::ToolResultContentBlock::Image { source } => {
                        assert_eq!(source.source_type, "base64");
                        assert_eq!(source.media_type, "image/png");
                        assert_eq!(source.data, "iVBORw0KGgo=");
                    }
                    other => panic!("expected Image block, got {other:?}"),
                }
                match &content[1] {
                    api::ToolResultContentBlock::Text { text } => {
                        assert!(text.contains("42 bytes"), "got: {text}");
                    }
                    other => panic!("expected Text block, got {other:?}"),
                }
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn non_screenshot_tool_result_stays_as_text() {
        use api::InputContentBlock;
        let messages = vec![ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                tool_name: "navigate".to_string(),
                output: r#"{"url":"https://example.com","status":200}"#.to_string(),
                is_error: false,
            }],
            usage: None,
        }];
        let converted = convert_messages(&messages);
        let content = &converted[0].content;
        assert_eq!(content.len(), 1);
        match &content[0] {
            InputContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.len(), 1);
                assert!(matches!(
                    &content[0],
                    api::ToolResultContentBlock::Text { .. }
                ));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
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
    fn push_output_block_emits_text_delta() {
        let mut events = Vec::new();
        let mut pending_tool = None;
        push_output_block(
            OutputContentBlock::Text {
                text: "# Heading".to_string(),
            },
            &mut events,
            &mut pending_tool,
            false,
        );
        assert!(matches!(
            &events[0],
            AssistantEvent::TextDelta(text) if text == "# Heading"
        ));
    }

    #[test]
    fn push_output_block_skips_empty_object_prefix_for_tool_streams() {
        let mut events = Vec::new();
        let mut pending_tool = None;
        push_output_block(
            OutputContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "read_file".to_string(),
                input: json!({}),
            },
            &mut events,
            &mut pending_tool,
            true,
        );
        assert!(events.is_empty());
        assert_eq!(
            pending_tool,
            Some(("tool-1".to_string(), "read_file".to_string(), String::new()))
        );
    }

    #[test]
    fn response_to_events_preserves_empty_object_json_input_outside_streaming() {
        let events = response_to_events(MessageResponse {
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
        });
        assert!(
            matches!(&events[0], AssistantEvent::ToolUse { name, input, .. } if name == "read_file" && input == "{}")
        );
    }

    #[test]
    fn response_to_events_preserves_non_empty_json_input_outside_streaming() {
        let events = response_to_events(MessageResponse {
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
        });
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

    #[test]
    fn model_supports_reasoning_matches_catalog_capabilities() {
        with_clean_config_env(|| {
            assert!(model_supports_reasoning("o3"));
            assert!(!model_supports_reasoning("claude-sonnet-4-6"));
        });
    }

    #[test]
    fn model_reasoning_efforts_returns_expected_efforts_for_reasoning_models() {
        with_clean_config_env(|| {
            assert_eq!(
                model_reasoning_efforts("o4-mini"),
                api::ReasoningEffort::OPENAI.to_vec()
            );
            assert!(model_reasoning_efforts("gpt-4o").is_empty());
        });
    }

    #[test]
    fn filter_tool_specs_without_allowlist_returns_all_specs() {
        let filtered = filter_tool_specs(None);
        let all = mvp_tool_specs();

        assert_eq!(filtered.len(), all.len());
        assert_eq!(
            filtered.iter().map(|spec| spec.name).collect::<Vec<_>>(),
            all.iter().map(|spec| spec.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn filter_tool_specs_with_specific_names_returns_only_matching_specs() {
        let allowed = ["navigate", "read_content"]
            .into_iter()
            .map(str::to_string)
            .collect();

        let filtered = filter_tool_specs(Some(&allowed));

        assert_eq!(
            filtered.iter().map(|spec| spec.name).collect::<Vec<_>>(),
            vec!["navigate", "read_content"]
        );
    }

    #[test]
    fn final_assistant_text_returns_joined_text_from_last_assistant_message() {
        let summary = runtime::TurnSummary {
            assistant_messages: vec![
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "ignored".to_string(),
                }]),
                ConversationMessage::assistant(vec![
                    ContentBlock::Text {
                        text: "hello".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "navigate".to_string(),
                        input: "{}".to_string(),
                    },
                    ContentBlock::Text {
                        text: " world".to_string(),
                    },
                ]),
            ],
            tool_results: vec![],
            iterations: 2,
            usage: runtime::TokenUsage::default(),
            auto_compaction: None,
        };

        assert_eq!(final_assistant_text(&summary), "hello world");
    }
}
