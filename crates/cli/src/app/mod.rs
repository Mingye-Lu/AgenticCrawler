mod api_client;
mod model_support;
mod resume;
mod runtime_builder;
#[allow(clippy::wildcard_imports)]
mod session;
#[allow(clippy::wildcard_imports)]
mod slash;
#[cfg(test)]
mod tests;
mod title_namer;
mod tool_executor;
#[allow(clippy::wildcard_imports)]
mod turn;

use std::collections::BTreeSet;
use std::io::{self, IsTerminal};
use std::str::FromStr;
use std::sync::{mpsc, Arc, Mutex};

use crate::error::CliError;
use crate::output_sink::ChannelSink;
use crate::session_mgr::{create_managed_session_handle, SessionHandle};
use acrawl_core::ToolSpec;
use agent::{mvp_tool_specs, ChildControlRegistry, ChildEvent, ExtensionBridge};
use browser::{BrowserBackend, BrowserState, SharedBridge, WsBridgeServer};
use render::sink::{OutputSink, StdoutSink};

#[cfg(feature = "tui-crate-context")]
use crate::events::ReplTuiEvent;
#[cfg(not(feature = "tui-crate-context"))]
use acrawl_tui::events::ReplTuiEvent;
use commands::slash_command_specs;
use runtime::{ControlState, ConversationRuntime, RuntimeError, Session, TokenUsage};

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

    pub(super) fn event_sender(&self) -> Option<mpsc::Sender<ReplTuiEvent>> {
        self.output_mode.sender()
    }

    pub(crate) fn cancel_flag(&self) -> std::sync::Arc<ControlState> {
        self.runtime.cancel_flag()
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

    pub(crate) fn extension_connection_watch(&self) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.ws_bridge_server
            .as_ref()
            .map(WsBridgeServer::connection_watcher)
    }
}
