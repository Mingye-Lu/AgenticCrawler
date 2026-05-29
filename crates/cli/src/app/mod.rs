mod api_client;
mod model_support;
mod resume;
mod runtime_builder;
mod title_namer;
mod tool_executor;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::str::FromStr;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::CliError;
use crate::output_sink::ChannelSink;
use crate::session_mgr::{create_managed_session_handle, SessionHandle};
use acrawl_core::ToolSpec;
use agent::{ChildControlRegistry, ChildEvent, ExtensionBridge};
use browser::{generate_bridge_token, BrowserBackend, BrowserState, SharedBridge, WsBridgeServer};
use render::format::{
    format_auto_compaction_notice, format_compact_report, format_cost_report, format_model_report,
    format_model_switch_report, format_status_report, render_config_report, render_export_text,
    render_repl_help, render_version_report, resolve_export_path, status_context, StatusUsage,
};
use render::markdown::{Spinner, TerminalRenderer};
use render::sink::{OutputSink, StdoutSink};

#[cfg(feature = "tui-crate-context")]
use crate::events::ReplTuiEvent;
#[cfg(not(feature = "tui-crate-context"))]
use acrawl_tui::events::ReplTuiEvent;
use agent::mvp_tool_specs;
use commands::{slash_command_specs, MemoryAction, SlashCommand};
use runtime::{
    aggregate_evidence_from_episodes, build_memory_episode, CompactionConfig, ContentBlock,
    ControlState, ConversationMessage, ConversationRuntime, EpisodeStore,
    EvidenceAggregationConfig, EvidenceStore, MemoryContextBudget, MemoryContextLoader,
    MemoryContextQuery, MemoryEpisodeBuildConfig, MemoryEpisodeBuildInput, MemoryEpisodeResult,
    MessageRole, RuntimeError, Session, TokenUsage,
};
use serde_json::json;

#[cfg(test)]
use api::provider::ProviderRegistry;

use self::api_client::LlmRuntimeClient;
#[cfg(test)]
use self::api_client::{convert_messages, push_output_block, response_to_events};
use self::model_support::{model_reasoning_efforts, model_supports_reasoning};
use self::runtime_builder::{build_runtime, build_runtime_with_options, build_system_prompt};
use self::tool_executor::CliToolExecutor;

pub(crate) use crate::auth::{
    bind_oauth_listener, default_oauth_config, open_browser, parse_provider_arg, run_auth_cli,
    wait_for_oauth_callback_cancellable,
};
use crate::auth::{
    interactive_login_prompt, prompt_provider_choice, provider_choice_label, resolve_provider_arg,
};

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

pub(crate) fn filter_tool_specs(allowed_tools: Option<&AllowedToolSet>) -> Vec<ToolSpec> {
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
    if !io::stdout().is_terminal() {
        return Err(CliError::from(
            "acrawl REPL requires an interactive terminal. \
             For headless use, run `acrawl prompt \"<goal>\"` (one-shot) \
             or `acrawl --resume <session.json> <slash-commands>` (session maintenance).",
        ));
    }
    #[cfg(not(feature = "tui-crate-context"))]
    {
        Ok(acrawl_tui::run_tui(model, allowed_tools)?)
    }
    #[cfg(feature = "tui-crate-context")]
    {
        Ok(crate::run_tui(model, allowed_tools)?)
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
    child_event_rx: Option<std::sync::mpsc::Receiver<ChildEvent>>,
    child_control_registry: Option<ChildControlRegistry>,
    pending_title: Arc<Mutex<Option<String>>>,
    title_dispatched: bool,
    ws_bridge_server: Option<WsBridgeServer>,
    pending_extension_state: Option<BrowserState>,
    extension_bridge_initialized: bool,
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
    pub(crate) fn new_non_interactive(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
    ) -> Result<Self, CliError> {
        Self::new_with_interactivity(model, enable_tools, allowed_tools, false)
    }

    fn new_with_interactivity(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        is_interactive: bool,
    ) -> Result<Self, CliError> {
        let settings = runtime::load_settings();
        let system_prompt = build_system_prompt();
        let session = create_managed_session_handle();
        let output_mode = OutputMode::Stdout;
        let runtime = build_runtime_with_options(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            allowed_tools.clone(),
            output_mode.observer(),
            is_interactive,
            None,
            None,
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
            output_mode,
            reasoning_effort: initial_effort,
            debug_mode: false,
            child_event_rx: None,
            child_control_registry: None,
            pending_title: Arc::new(Mutex::new(None)),
            title_dispatched: false,
            ws_bridge_server: None,
            pending_extension_state: None,
            extension_bridge_initialized: false,
        };
        if let Some(effort) = initial_effort {
            cli.runtime
                .api_client_mut()
                .set_reasoning_effort(Some(effort));
        }
        cli.boot_bridge_server_if_needed();
        Ok(cli)
    }

    pub(crate) fn new_with_ui_tx(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        event_tx: mpsc::Sender<ReplTuiEvent>,
    ) -> Result<Self, CliError> {
        let settings = runtime::load_settings();
        let system_prompt = build_system_prompt();
        let session = create_managed_session_handle();
        let output_mode = OutputMode::Channel(event_tx);
        let (child_event_tx, child_event_rx) = std::sync::mpsc::channel::<ChildEvent>();
        let registry = ChildControlRegistry::default();
        let runtime = build_runtime_with_options(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            allowed_tools.clone(),
            output_mode.observer(),
            true,
            None,
            Some(child_event_tx),
            Some(registry.clone()),
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
            child_event_rx: Some(child_event_rx),
            child_control_registry: Some(registry.clone()),
            pending_title: Arc::new(Mutex::new(None)),
            title_dispatched: false,
            ws_bridge_server: None,
            pending_extension_state: None,
            extension_bridge_initialized: false,
        };
        if let Some(effort) = initial_effort {
            cli.runtime
                .api_client_mut()
                .set_reasoning_effort(Some(effort));
        }
        cli.boot_bridge_server_if_needed();
        Ok(cli)
    }

    pub(crate) fn session_id(&self) -> &str {
        self.session.id.as_str()
    }

    pub(crate) fn session_messages(&self) -> Vec<runtime::ConversationMessage> {
        self.runtime.session().messages.clone()
    }

    pub(crate) fn session_child_sessions(&self) -> Vec<runtime::ChildSession> {
        self.runtime.session().child_sessions.clone()
    }

    pub(crate) fn take_child_event_rx(&mut self) -> Option<std::sync::mpsc::Receiver<ChildEvent>> {
        self.child_event_rx.take()
    }

    pub(crate) fn take_child_control_registry(&mut self) -> Option<ChildControlRegistry> {
        self.child_control_registry.take()
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

    pub(crate) fn cancel_flag(&self) -> std::sync::Arc<ControlState> {
        self.runtime.cancel_flag()
    }

    pub(crate) fn run_turn_tui(&mut self, input: &str) -> Result<(), CliError> {
        self.maybe_dispatch_title_generation(input);
        if let Some(tx) = self.event_sender() {
            let _ = tx.send(ReplTuiEvent::TurnStarting);
        }
        let result = block_on_runtime_future(self.runtime.run_turn(input));
        let finish: Result<(), String> = match &result {
            Ok(summary) => {
                self.capture_child_sessions();
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

    pub(crate) fn run_turn(&mut self, input: &str) -> Result<(), CliError> {
        self.maybe_dispatch_title_generation(input);
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
                self.capture_child_sessions();
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
        self.maybe_dispatch_title_generation(input);
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
        capture_child_sessions_into_session(&mut runtime);
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
                println!("{}", self.status_report());
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
            SlashCommand::Clear => {
                let result = self.clear_session_command()?;
                println!("{}", result.message);
                result.persist_after
            }
            SlashCommand::Cost => {
                println!("{}", self.cost_report());
                false
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
            SlashCommand::Sessions => {
                println!("Session picker is only available in the interactive TUI.");
                false
            }
            SlashCommand::Auth { provider } => {
                self.run_auth(provider.as_deref())?;
                false
            }
            SlashCommand::Headed => {
                if self.is_extension_mode_active() {
                    println!("Browser mode\n  Ignored          extension mode is active (browser is already visible)");
                    return Ok(false);
                }
                env::set_var("HEADLESS", "false");
                let _ = runtime::update_settings(|s| {
                    s.headless = Some(false);
                });
                self.reset_browser();
                println!("Browser mode\n  Result           switched to headed (visible)");
                false
            }
            SlashCommand::Headless => {
                if self.is_extension_mode_active() {
                    println!("Browser mode\n  Ignored          extension mode is active (browser is already visible)");
                    return Ok(false);
                }
                env::set_var("HEADLESS", "true");
                let _ = runtime::update_settings(|s| {
                    s.headless = Some(true);
                });
                self.reset_browser();
                println!("Browser mode\n  Result           switched to headless");
                false
            }
            SlashCommand::Extension { stop } => {
                if stop {
                    println!("{}", self.stop_extension_server());
                    return Ok(false);
                }
                if let Some(status) = self.extension_bridge_status() {
                    println!("{status}");
                } else {
                    match self.start_extension_server() {
                        Ok((token, port)) => {
                            println!(
                                "Extension bridge\n  \
                                 Status           server started (port {port})\n  \
                                 Token            {token}"
                            );
                        }
                        Err(e) => eprintln!("{e}"),
                    }
                }
                false
            }
            SlashCommand::CloakBrowser => {
                let message = self.switch_to_cloakbrowser();
                println!("{message}");
                false
            }
            SlashCommand::Memory {
                action: MemoryAction::Save,
            } => {
                println!("{}", self.memory_save_command());
                false
            }
            SlashCommand::Memory {
                action: MemoryAction::Status,
            } => {
                println!(
                    "{}",
                    memory_status_report(
                        &EpisodeStore::default_for_config_home(),
                        &EvidenceStore::default_for_config_home(),
                    )
                );
                false
            }
            SlashCommand::Memory {
                action: MemoryAction::Context,
            } => {
                println!("{}", self.memory_context_command());
                false
            }
            SlashCommand::Memory {
                action: MemoryAction::BuildEvidence,
            } => {
                println!(
                    "{}",
                    memory_build_evidence_report(
                        &EpisodeStore::default_for_config_home(),
                        &EvidenceStore::default_for_config_home(),
                    )
                );
                false
            }
            SlashCommand::Unknown(name) => {
                eprintln!("unknown slash command: /{name}");
                false
            }
        })
    }

    pub(crate) fn persist_session(&mut self) -> Result<(), CliError> {
        if self.runtime.session().messages.is_empty() {
            return Ok(());
        }
        if self.runtime.session().title.is_none() {
            let mut guard = self
                .pending_title
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(title) = guard.take() {
                self.runtime.session_mut().title = Some(title);
            }
        }
        self.runtime.session().save_to_path(&self.session.path)?;
        Ok(())
    }

    pub(crate) fn memory_save_command(&mut self) -> String {
        memory_save_report(
            self.runtime.session(),
            &runtime::EpisodeStore::default_for_config_home(),
        )
    }

    pub(crate) fn memory_context_command(&self) -> String {
        memory_context_report(
            self.runtime.session(),
            &MemoryContextLoader::default_for_config_home(),
        )
    }

    fn capture_child_sessions(&mut self) {
        capture_child_sessions_into_session(&mut self.runtime);
    }

    fn maybe_dispatch_title_generation(&mut self, user_input: &str) {
        if self.title_dispatched {
            return;
        }
        if self.runtime.session().title.is_some() {
            self.title_dispatched = true;
            return;
        }
        if !self.runtime.session().messages.is_empty() {
            self.title_dispatched = true;
            return;
        }
        let trimmed = user_input.trim();
        if trimmed.is_empty() {
            return;
        }
        self.title_dispatched = true;
        title_namer::spawn_title_generation(
            self.model.clone(),
            trimmed.to_string(),
            Arc::clone(&self.pending_title),
        );
    }

    pub(crate) fn reset_browser(&mut self) {
        self.runtime.tool_executor_mut().reset_browser();
    }

    pub(crate) fn is_extension_mode_active(&self) -> bool {
        self.extension_bridge_initialized || self.ws_bridge_server.is_some()
    }

    pub(crate) fn prepare_extension_bridge_activation(
        &mut self,
    ) -> Result<(SharedBridge, Option<BrowserState>), String> {
        if self.extension_bridge_initialized {
            return Err("extension bridge is already initialized".to_string());
        }

        let Some(server) = self.ws_bridge_server.as_ref() else {
            return Err("extension bridge server is not running".to_string());
        };

        let sender = server.command_sender();
        let connected = server.connection_watcher();
        let bridge = ExtensionBridge::new(sender, connected);
        let shared: SharedBridge = Arc::new(tokio::sync::Mutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        ));

        Ok((shared, self.pending_extension_state.take()))
    }

    pub(crate) fn activate_extension_bridge(&mut self, shared: SharedBridge) {
        self.runtime
            .tool_executor_mut()
            .set_extension_bridge(shared);
        self.runtime.tool_executor_mut().set_extension_mode(true);
        self.extension_bridge_initialized = true;
        let _ = runtime::update_settings(|s| {
            s.browser_backend = Some("extension".to_string());
        });
    }

    pub(crate) fn restore_pending_extension_state(&mut self, state: Option<BrowserState>) {
        if self.pending_extension_state.is_none() {
            self.pending_extension_state = state;
        }
    }

    pub(crate) fn switch_to_cloakbrowser(&mut self) -> String {
        let saved_state = block_on_runtime_future(async {
            Ok::<Option<BrowserState>, RuntimeError>(
                self.runtime
                    .tool_executor_mut()
                    .export_current_state()
                    .await,
            )
        })
        .unwrap_or(None);

        self.ws_bridge_server = None;
        self.pending_extension_state = None;
        self.extension_bridge_initialized = false;
        self.runtime.tool_executor_mut().set_extension_mode(false);
        self.reset_browser();

        let _ = runtime::update_settings(|s| {
            s.browser_backend = None;
        });

        if let Some(state) = saved_state.as_ref() {
            self.pending_extension_state = Some(state.clone());
        }

        "Browser mode\n  Result           switched back to CloakBrowser (headless)".to_string()
    }

    pub(crate) fn stop_extension_server(&mut self) -> String {
        if self.ws_bridge_server.is_none() {
            return "Extension mode\n  Result           bridge server already stopped".to_string();
        }

        self.ws_bridge_server = None;
        self.pending_extension_state = None;
        self.extension_bridge_initialized = false;
        self.runtime.tool_executor_mut().clear_extension_bridge();
        self.runtime.tool_executor_mut().set_extension_mode(false);

        let _ = runtime::update_settings(|s| {
            if s.browser_backend.as_deref() == Some("extension") {
                s.browser_backend = None;
            }
        });

        "Extension mode\n  Result           bridge server stopped".to_string()
    }

    pub(crate) fn extension_bridge_status(&self) -> Option<String> {
        let server = self.ws_bridge_server.as_ref()?;
        let settings = runtime::load_settings();
        let token = settings.extension_bridge_token.unwrap_or_default();
        let port = server.port();
        let status = if server.is_client_connected() && !self.extension_bridge_initialized {
            "browser connected; initializing"
        } else if server.is_client_connected() {
            "connected"
        } else {
            "waiting for browser"
        };
        Some(format!(
            "Extension mode\n  \
             Status           {status} (port {port})\n  \
             Token            {token}"
        ))
    }

    pub(crate) fn extension_connection_watch(&self) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.ws_bridge_server
            .as_ref()
            .map(WsBridgeServer::connection_watcher)
    }

    pub(crate) fn start_extension_server(&mut self) -> Result<(String, u16), String> {
        if self.ws_bridge_server.is_some() {
            self.ws_bridge_server = None;
            self.runtime.tool_executor_mut().clear_extension_bridge();
        }
        self.pending_extension_state = None;
        self.extension_bridge_initialized = false;

        let saved_state = block_on_runtime_future(async {
            Ok::<Option<BrowserState>, RuntimeError>(
                self.runtime
                    .tool_executor_mut()
                    .export_current_state()
                    .await,
            )
        })
        .unwrap_or(None);

        let settings = runtime::load_settings();
        let token = settings
            .extension_bridge_token
            .unwrap_or_else(generate_bridge_token);

        let _ = runtime::update_settings(|s| {
            s.extension_bridge_token = Some(token.clone());
        });

        let port: u16 = settings.extension_bridge_port.unwrap_or(19876);

        let server = block_on_runtime_future(async {
            WsBridgeServer::start(port, token.clone())
                .await
                .map_err(|e| RuntimeError::new(e.to_string()))
        })
        .map_err(|e| format!("Extension bridge server\n  Error            {e}"))?;

        self.pending_extension_state = saved_state;
        self.ws_bridge_server = Some(server);

        Ok((token, port))
    }

    pub(crate) fn boot_bridge_server_if_needed(&mut self) {
        if self.ws_bridge_server.is_some() {
            return;
        }
        let settings = runtime::load_settings();
        if settings.browser_backend.as_deref() != Some("extension") {
            return;
        }
        if let Err(e) = self.start_extension_server() {
            eprintln!("[acrawl] bridge server auto-start failed: {e}");
        }
    }

    pub(crate) fn status_report(&self) -> String {
        let cumulative = self.runtime.usage().cumulative_usage();
        let latest = self.runtime.usage().current_turn_usage();
        format_status_report(
            &self.model,
            StatusUsage {
                message_count: self.runtime.session().messages.len(),
                turns: self.runtime.usage().turns(),
                latest,
                cumulative,
                estimated_tokens: self.runtime.estimated_tokens(),
            },
            &status_context(Some(&self.session.path)),
        )
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

    pub(crate) fn clear_session_command(&mut self) -> Result<CommandUiResult, CliError> {
        self.session = create_managed_session_handle();
        self.title_dispatched = false;
        if let Ok(mut guard) = self.pending_title.lock() {
            *guard = None;
        }
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
            persist_after: false,
        })
    }

    pub(crate) fn cost_report(&self) -> String {
        format_cost_report(self.runtime.usage().cumulative_usage())
    }

    pub(crate) fn switch_to_session_handle(
        &mut self,
        handle: SessionHandle,
    ) -> Result<usize, CliError> {
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
        self.title_dispatched = true;
        if let Ok(mut guard) = self.pending_title.lock() {
            *guard = None;
        }
        Ok(message_count)
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

fn capture_child_sessions_into_session(
    runtime: &mut ConversationRuntime<LlmRuntimeClient, CliToolExecutor>,
) {
    let child_sessions = runtime.tool_executor_mut().take_captured_child_sessions();
    merge_child_sessions(runtime.session_mut(), child_sessions);
}

fn merge_child_sessions(session: &mut Session, child_sessions: Vec<runtime::ChildSession>) {
    if child_sessions.is_empty() {
        return;
    }
    session.child_sessions.extend(child_sessions);
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

pub(crate) fn first_user_text(messages: &[ConversationMessage]) -> String {
    messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .and_then(|m| {
            m.blocks.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn last_assistant_text(messages: &[ConversationMessage]) -> Option<String> {
    let text = messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::Assistant)
        .map(|m| {
            m.blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(crate) fn memory_save_report(session: &Session, store: &EpisodeStore) -> String {
    if session.messages.is_empty() {
        return "Memory\n  Result           skipped".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let now_epoch_secs = now.as_secs();
    let id = format!("episode-{}", now.as_millis());
    let user_goal = first_user_text(&session.messages);
    let output_summary = last_assistant_text(&session.messages);
    let input = MemoryEpisodeBuildInput {
        id,
        task_class: None,
        user_goal,
        result: MemoryEpisodeResult::Success,
        output_summary,
        messages: &session.messages,
        created_at_epoch_secs: now_epoch_secs,
        promote_candidate: true,
    };
    let episode = build_memory_episode(input, MemoryEpisodeBuildConfig::default());
    match store.append_episode(&episode) {
        Ok(()) => format!(
            "Memory\n  Result           saved\n  Episode          {}\n  Domains          {}\n  Tools            {}\n  Route steps      {}",
            episode.id,
            episode.domains.len(),
            episode.tools.len(),
            episode.route.len(),
        ),
        Err(err) => format!("Memory\n  Result           failed ({err})"),
    }
}

pub(crate) fn memory_status_report(
    episode_store: &EpisodeStore,
    evidence_store: &EvidenceStore,
) -> String {
    let episodes_file = if episode_store.episodes_path().exists() {
        "present"
    } else {
        "missing"
    };

    let recent_result = episode_store.load_recent_episodes(3);

    let task_count = count_json_files(&evidence_store.evidence_dir().join("tasks"));
    let domain_count = count_json_files(&evidence_store.evidence_dir().join("domains"));
    let access_count = count_json_files(&evidence_store.evidence_dir().join("access"));

    let (recent, recent_status) = match recent_result {
        Ok(recent) => {
            let count = recent.len();
            (recent, count.to_string())
        }
        Err(err) => (Vec::new(), format!("failed ({err})")),
    };

    let mut lines = vec![format!(
        "Memory\n  Episodes file    {episodes_file}\n  Recent episodes  {recent_status}\n  Task evidence    {task_count}\n  Domain evidence  {domain_count}\n  Access evidence  {access_count}",
    )];

    if !recent.is_empty() {
        lines.push(String::new());
        lines.push("Recent".to_string());
        for ep in &recent {
            let truncated = truncate_str(&ep.user_goal, 60);
            let result_str = match ep.result {
                MemoryEpisodeResult::Success => "success",
                MemoryEpisodeResult::Partial => "partial",
                MemoryEpisodeResult::Failure => "failure",
            };
            lines.push(format!("  {:<14} {:<8} {}", ep.id, result_str, truncated));
        }
    }

    lines.join("\n")
}

pub(crate) fn memory_context_report(session: &Session, loader: &MemoryContextLoader) -> String {
    if session.messages.is_empty() {
        return "Memory\n  Result           skipped\n  Reason           empty session".to_string();
    }

    let episode = build_memory_episode(
        MemoryEpisodeBuildInput {
            id: "preview".to_string(),
            task_class: None,
            user_goal: first_user_text(&session.messages),
            result: MemoryEpisodeResult::Success,
            output_summary: last_assistant_text(&session.messages),
            messages: &session.messages,
            created_at_epoch_secs: 0,
            promote_candidate: false,
        },
        MemoryEpisodeBuildConfig::default(),
    );

    let query = MemoryContextQuery {
        task_class: None,
        domains: episode.domains,
        include_access: true,
        recent_episode_limit: 2,
    };

    let budget = MemoryContextBudget::default();

    match loader.load(&query, budget) {
        Ok(context) => {
            let rendered = loader.render_context(&context, budget);
            if rendered.is_empty() {
                "Memory\n  Result           empty\n  Reason           no relevant memory"
                    .to_string()
            } else {
                format!("Memory\n  Result           context\n\n{rendered}")
            }
        }
        Err(err) => format!("Memory\n  Result           failed ({err})"),
    }
}

fn count_json_files(dir: &std::path::Path) -> usize {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .count(),
        Err(_) => 0,
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max.saturating_sub(3);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

const MEMORY_BUILD_EPISODE_LIMIT: usize = 500;

pub(crate) fn memory_build_evidence_report(
    episode_store: &EpisodeStore,
    evidence_store: &EvidenceStore,
) -> String {
    let path = episode_store.episodes_path();
    if !path.exists() {
        return "Memory\n  Result           skipped".to_string();
    }

    let episodes = match episode_store.load_recent_episodes(MEMORY_BUILD_EPISODE_LIMIT) {
        Ok(eps) => eps,
        Err(err) => return format!("Memory\n  Result           failed ({err})"),
    };

    let config = EvidenceAggregationConfig::default();
    let result = aggregate_evidence_from_episodes(&episodes, config);

    let task_count = result.task_evidence.len();
    let domain_count = result.domain_evidence.len();

    if task_count == 0 && domain_count == 0 {
        return format!(
            "Memory\n  Result           skipped\n  Episodes read    {}",
            episodes.len()
        );
    }

    for evidence in &result.task_evidence {
        if let Err(err) = evidence_store.save_task_evidence(evidence) {
            return format!("Memory\n  Result           failed ({err})");
        }
    }

    for evidence in &result.domain_evidence {
        if let Err(err) = evidence_store.save_domain_evidence(evidence) {
            return format!("Memory\n  Result           failed ({err})");
        }
    }

    format!(
        "Memory\n  Result           built evidence\n  Episodes read    {}\n  Task evidence    {}\n  Domain evidence  {}",
        episodes.len(),
        task_count,
        domain_count,
    )
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
    fn model_supports_reasoning_catalog_known_models_are_deterministic() {
        assert!(model_supports_reasoning("o3"));
        assert!(model_supports_reasoning("o4-mini"));
        assert!(!model_supports_reasoning("gpt-4o"));
        assert!(!model_supports_reasoning("claude-sonnet-4-6"));
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

    #[test]
    fn merge_child_sessions_extends_session() {
        let mut session = Session::new();

        merge_child_sessions(
            &mut session,
            vec![runtime::ChildSession {
                id: "child-1".to_string(),
                goal: "scrape prices".to_string(),
                messages: vec![ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "done".to_string(),
                }])],
            }],
        );

        assert_eq!(session.child_sessions.len(), 1);
        assert_eq!(session.child_sessions[0].id, "child-1");
    }

    #[test]
    fn first_user_text_returns_first_user_message() {
        let messages = vec![
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "assistant first".to_string(),
            }]),
            ConversationMessage::user_text("hello"),
            ConversationMessage::user_text("second user"),
        ];
        assert_eq!(first_user_text(&messages), "hello");
    }

    #[test]
    fn first_user_text_returns_unknown_when_no_user() {
        let messages = vec![ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "only assistant".to_string(),
        }])];
        assert_eq!(first_user_text(&messages), "unknown");
    }

    #[test]
    fn last_assistant_text_returns_last_assistant_text() {
        let messages = vec![
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "first reply".to_string(),
            }]),
            ConversationMessage::user_text("user says"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "final reply".to_string(),
            }]),
        ];
        assert_eq!(
            last_assistant_text(&messages),
            Some("final reply".to_string())
        );
    }

    #[test]
    fn last_assistant_text_returns_none_when_no_assistant() {
        let messages = vec![ConversationMessage::user_text("only user")];
        assert_eq!(last_assistant_text(&messages), None);
    }

    #[test]
    fn memory_save_report_empty_session_skips() {
        with_clean_config_env(|| {
            let session = Session::new();
            let store = EpisodeStore::default_for_config_home();
            let report = memory_save_report(&session, &store);
            assert!(report.contains("skipped"));
            assert!(!store.episodes_path().exists());
        });
    }

    #[test]
    fn memory_save_report_writes_episode() {
        with_clean_config_env(|| {
            let mut session = Session::new();
            session.messages = vec![
                ConversationMessage::user_text("extract titles from example.com"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Got 5 titles".to_string(),
                }]),
            ];
            let store = EpisodeStore::default_for_config_home();
            let report = memory_save_report(&session, &store);
            assert!(report.contains("saved"));
            assert!(report.contains("Episode"));
            assert!(store.episodes_path().exists());
            let episodes = store.load_recent_episodes(1).expect("episode should load");
            assert_eq!(episodes.len(), 1);
            assert!(episodes[0].id.starts_with("episode-"));
            assert_eq!(episodes[0].user_goal, "extract titles from example.com");
            assert_eq!(episodes[0].output_summary.as_deref(), Some("Got 5 titles"));
        });
    }

    #[test]
    fn memory_save_report_uses_unknown_without_user_text() {
        with_clean_config_env(|| {
            let mut session = Session::new();
            session.messages = vec![ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "only assistant".to_string(),
            }])];
            let store = EpisodeStore::default_for_config_home();
            let report = memory_save_report(&session, &store);
            assert!(report.contains("saved"));
            assert!(store.episodes_path().exists());
            let episodes = store.load_recent_episodes(1).expect("episode should load");
            assert_eq!(episodes[0].user_goal, "unknown");
        });
    }

    #[test]
    fn memory_status_report_missing_dir_reports_zero() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            let evidence_store = EvidenceStore::default_for_config_home();
            let report = memory_status_report(&episode_store, &evidence_store);
            assert!(report.contains("Episodes file"), "report: {report}");
            assert!(report.contains("missing"), "report: {report}");
            assert!(report.contains("Recent episodes  0"), "report: {report}");
            assert!(report.contains("Task evidence    0"), "report: {report}");
            assert!(report.contains("Domain evidence  0"), "report: {report}");
            assert!(report.contains("Access evidence  0"), "report: {report}");
        });
    }

    #[test]
    fn memory_status_report_counts_episodes_after_save() {
        with_clean_config_env(|| {
            let mut session = Session::new();
            session.messages = vec![
                ConversationMessage::user_text("scrape example.com"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Done".to_string(),
                }]),
            ];
            let episode_store = EpisodeStore::default_for_config_home();
            let evidence_store = EvidenceStore::default_for_config_home();

            let _save_report = memory_save_report(&session, &episode_store);

            let report = memory_status_report(&episode_store, &evidence_store);
            assert!(
                report.contains("Episodes file    present"),
                "report: {report}"
            );
            assert!(report.contains("Recent episodes  1"), "report: {report}");
            assert!(report.contains("Recent"), "report: {report}");
            assert!(report.contains("scrape example.com"), "report: {report}");
        });
    }

    #[test]
    fn memory_status_report_reports_episode_load_failure() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            fs::create_dir_all(episode_store.episodes_path())
                .expect("create directory at episodes path");
            let evidence_store = EvidenceStore::default_for_config_home();

            let report = memory_status_report(&episode_store, &evidence_store);
            assert!(
                report.contains("Episodes file    present"),
                "report: {report}"
            );
            assert!(
                report.contains("Recent episodes  failed"),
                "report: {report}"
            );
        });
    }

    #[test]
    fn memory_status_report_evidence_counts_work() {
        with_clean_config_env(|| {
            let evidence_store = EvidenceStore::default_for_config_home();

            let task = runtime::TaskEvidence {
                task_class: "test-task".to_string(),
                successful_routes: vec![],
                tools: vec![],
                output_fields: vec![],
                success_count: 1,
                failure_count: 0,
                last_used_epoch_secs: 1,
            };
            let domain = runtime::DomainEvidence {
                domain: "example.com".to_string(),
                task_classes: vec![],
                successful_routes: vec![],
                field_hints: vec![],
                success_count: 1,
                failure_count: 0,
                last_verified_epoch_secs: 1,
            };
            let access = runtime::AccessEvidence {
                domain: "example.com".to_string(),
                status: runtime::AccessStatus::LoggedInObserved,
                extension_mode: false,
                last_confirmed_epoch_secs: 1,
                notes: None,
            };

            evidence_store.save_task_evidence(&task).unwrap();
            evidence_store.save_domain_evidence(&domain).unwrap();
            evidence_store.save_access_evidence(&access).unwrap();

            let episode_store = EpisodeStore::default_for_config_home();
            let report = memory_status_report(&episode_store, &evidence_store);
            assert!(report.contains("Task evidence    1"), "report: {report}");
            assert!(report.contains("Domain evidence  1"), "report: {report}");
            assert!(report.contains("Access evidence  1"), "report: {report}");
        });
    }

    #[test]
    fn memory_context_report_empty_session_skips() {
        with_clean_config_env(|| {
            let session = Session::new();
            let loader = MemoryContextLoader::default_for_config_home();
            let report = memory_context_report(&session, &loader);
            assert!(report.contains("skipped"));
            assert!(report.contains("empty session"));
        });
    }

    #[test]
    fn memory_context_report_no_relevant_memory() {
        with_clean_config_env(|| {
            let mut session = Session::new();
            session.messages = vec![
                ConversationMessage::user_text("navigate to https://example.com"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Done".to_string(),
                }]),
            ];
            let loader = MemoryContextLoader::default_for_config_home();
            let report = memory_context_report(&session, &loader);
            assert!(report.contains("empty"));
            assert!(report.contains("no relevant memory"));
        });
    }

    #[test]
    fn memory_context_report_with_domain_evidence() {
        with_clean_config_env(|| {
            let evidence_store = EvidenceStore::default_for_config_home();
            let domain = runtime::DomainEvidence {
                domain: "example.com".to_string(),
                task_classes: vec!["scrape".to_string()],
                successful_routes: vec![vec!["navigate".to_string()]],
                field_hints: vec!["title".to_string()],
                success_count: 3,
                failure_count: 1,
                last_verified_epoch_secs: 1,
            };
            evidence_store.save_domain_evidence(&domain).unwrap();

            let mut session = Session::new();
            session.messages = vec![
                ConversationMessage::user_text("navigate to https://example.com"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Done".to_string(),
                }]),
            ];
            let loader = MemoryContextLoader::default_for_config_home();
            let report = memory_context_report(&session, &loader);
            assert!(report.contains("context"), "report: {report}");
            assert!(report.contains("example.com"), "report: {report}");
            assert!(report.contains("Domain evidence"), "report: {report}");
        });
    }

    #[test]
    fn memory_context_report_includes_access_evidence() {
        with_clean_config_env(|| {
            let evidence_store = EvidenceStore::default_for_config_home();
            let access = runtime::AccessEvidence {
                domain: "example.com".to_string(),
                status: runtime::AccessStatus::LoggedInObserved,
                extension_mode: false,
                last_confirmed_epoch_secs: 1,
                notes: None,
            };
            evidence_store.save_access_evidence(&access).unwrap();

            let domain = runtime::DomainEvidence {
                domain: "example.com".to_string(),
                task_classes: vec![],
                successful_routes: vec![],
                field_hints: vec![],
                success_count: 1,
                failure_count: 0,
                last_verified_epoch_secs: 1,
            };
            evidence_store.save_domain_evidence(&domain).unwrap();

            let mut session = Session::new();
            session.messages = vec![
                ConversationMessage::user_text("navigate to https://example.com"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Done".to_string(),
                }]),
            ];
            let loader = MemoryContextLoader::default_for_config_home();
            let report = memory_context_report(&session, &loader);
            assert!(report.contains("Access evidence"), "report: {report}");
            assert!(report.contains("LoggedInObserved"), "report: {report}");
        });
    }

    #[test]
    fn memory_context_report_episode_limit_bounded() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            for i in 1..=5 {
                episode_store
                    .append_episode(&runtime::MemoryEpisode {
                        id: format!("ep{i}"),
                        task_class: None,
                        user_goal: format!("goal {i}"),
                        route: vec![],
                        domains: vec![],
                        tools: vec![],
                        result: MemoryEpisodeResult::Success,
                        output_summary: None,
                        created_at_epoch_secs: i * 100,
                        promote_candidate: false,
                    })
                    .unwrap();
            }

            let mut session = Session::new();
            session.messages = vec![
                ConversationMessage::user_text("navigate to https://example.com"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Done".to_string(),
                }]),
            ];
            let loader = MemoryContextLoader::default_for_config_home();
            let report = memory_context_report(&session, &loader);
            assert!(report.contains("Recent episodes"), "report: {report}");
            assert!(!report.contains("ep1"), "report: {report}");
            assert!(report.contains("ep4"), "report: {report}");
            assert!(report.contains("ep5"), "report: {report}");
        });
    }

    #[test]
    fn memory_build_evidence_missing_episodes_file_skips() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            let evidence_store = EvidenceStore::default_for_config_home();
            let report = memory_build_evidence_report(&episode_store, &evidence_store);
            assert!(report.contains("skipped"), "report: {report}");
            assert!(
                !evidence_store.evidence_dir().join("tasks").exists(),
                "task dir should not exist"
            );
            assert!(
                !evidence_store.evidence_dir().join("domains").exists(),
                "domain dir should not exist"
            );
        });
    }

    #[test]
    fn memory_build_evidence_no_promotable_episodes_skips() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            let evidence_store = EvidenceStore::default_for_config_home();
            episode_store
                .append_episode(&runtime::MemoryEpisode {
                    id: "ep1".to_string(),
                    task_class: Some("scrape".to_string()),
                    user_goal: "scrape titles".to_string(),
                    route: vec!["navigate: example.com".to_string()],
                    domains: vec!["example.com".to_string()],
                    tools: vec!["navigate".to_string()],
                    result: MemoryEpisodeResult::Success,
                    output_summary: None,
                    created_at_epoch_secs: 1,
                    promote_candidate: false,
                })
                .unwrap();
            let report = memory_build_evidence_report(&episode_store, &evidence_store);
            assert!(report.contains("skipped"), "report: {report}");
            assert!(report.contains("Episodes read    1"), "report: {report}");
        });
    }

    #[test]
    fn memory_build_evidence_promotable_writes_task_and_domain_evidence() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            let evidence_store = EvidenceStore::default_for_config_home();
            episode_store
                .append_episode(&runtime::MemoryEpisode {
                    id: "ep1".to_string(),
                    task_class: Some("scrape".to_string()),
                    user_goal: "scrape titles".to_string(),
                    route: vec!["navigate: example.com".to_string()],
                    domains: vec!["example.com".to_string()],
                    tools: vec!["navigate".to_string()],
                    result: MemoryEpisodeResult::Success,
                    output_summary: None,
                    created_at_epoch_secs: 1,
                    promote_candidate: true,
                })
                .unwrap();
            let report = memory_build_evidence_report(&episode_store, &evidence_store);
            assert!(report.contains("built evidence"), "report: {report}");
            assert!(report.contains("Episodes read    1"), "report: {report}");
            assert!(report.contains("Task evidence    1"), "report: {report}");
            assert!(report.contains("Domain evidence  1"), "report: {report}");
            assert!(
                evidence_store
                    .evidence_dir()
                    .join("tasks")
                    .join("scrape.json")
                    .exists(),
                "task evidence file should exist"
            );
            assert!(
                evidence_store
                    .evidence_dir()
                    .join("domains")
                    .join("example.com.json")
                    .exists(),
                "domain evidence file should exist"
            );
        });
    }

    #[test]
    fn memory_build_evidence_does_not_write_access_evidence() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            let evidence_store = EvidenceStore::default_for_config_home();
            episode_store
                .append_episode(&runtime::MemoryEpisode {
                    id: "ep1".to_string(),
                    task_class: Some("scrape".to_string()),
                    user_goal: "scrape titles".to_string(),
                    route: vec!["navigate: example.com".to_string()],
                    domains: vec!["example.com".to_string()],
                    tools: vec!["navigate".to_string()],
                    result: MemoryEpisodeResult::Success,
                    output_summary: None,
                    created_at_epoch_secs: 1,
                    promote_candidate: true,
                })
                .unwrap();
            let _ = memory_build_evidence_report(&episode_store, &evidence_store);
            assert!(
                !evidence_store.evidence_dir().join("access").exists(),
                "access evidence dir should not exist"
            );
        });
    }

    #[test]
    fn memory_build_evidence_failed_episode_load_reports_failed() {
        with_clean_config_env(|| {
            let episode_store = EpisodeStore::default_for_config_home();
            let evidence_store = EvidenceStore::default_for_config_home();
            // Create a directory where the episodes file would be to cause a read failure
            fs::create_dir_all(episode_store.episodes_path())
                .expect("create directory at episodes path");
            let report = memory_build_evidence_report(&episode_store, &evidence_store);
            assert!(report.contains("failed"), "report: {report}");
        });
    }
}
