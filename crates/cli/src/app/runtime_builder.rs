use std::sync::Arc;

use super::{AllowedToolSet, CliError, CliToolExecutor, LlmRuntimeClient};
use crawler::{mvp_tool_specs, SharedApiClient};
use runtime::{
    load_settings, settings_get_max_steps, ConfigLoader, ControlState, ConversationRuntime,
    RuntimeObserver, Session,
};

pub(super) fn build_system_prompt() -> Vec<String> {
    crawler::build_system_prompt(&mvp_tool_specs())
}

pub(super) fn build_runtime_feature_config() -> Result<runtime::RuntimeFeatureConfig, CliError> {
    Ok(ConfigLoader::default_for().load()?.feature_config().clone())
}

pub(super) fn build_runtime(
    session: Session,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    allowed_tools: Option<AllowedToolSet>,
    observer: Box<dyn RuntimeObserver + Send>,
) -> Result<ConversationRuntime<LlmRuntimeClient, CliToolExecutor>, CliError> {
    build_runtime_with_options(
        session,
        model,
        system_prompt,
        enable_tools,
        allowed_tools,
        observer,
        true,
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_runtime_with_options(
    mut session: Session,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    allowed_tools: Option<AllowedToolSet>,
    observer: Box<dyn RuntimeObserver + Send>,
    is_interactive: bool,
    control_state: Option<Arc<ControlState>>,
    child_event_tx: Option<std::sync::mpsc::Sender<crawler::ChildEvent>>,
    child_control_registry: Option<crawler::ChildControlRegistry>,
) -> Result<ConversationRuntime<LlmRuntimeClient, CliToolExecutor>, CliError> {
    session.model = Some(model.clone());
    let max_steps = settings_get_max_steps(&load_settings()) as usize;
    let shared_control = control_state.unwrap_or_default();
    let fork_client = SharedApiClient::new(LlmRuntimeClient::new(
        model.clone(),
        enable_tools,
        allowed_tools.clone(),
    ));
    let mut api_client = LlmRuntimeClient::new(model, enable_tools, allowed_tools.clone());
    api_client.set_control_state(Arc::clone(&shared_control));
    Ok(ConversationRuntime::new_with_features(
        session,
        api_client,
        CliToolExecutor::new(
            allowed_tools,
            fork_client,
            is_interactive,
            Some(Arc::clone(&shared_control)),
            child_event_tx,
            child_control_registry,
        ),
        system_prompt,
        &build_runtime_feature_config()?,
    )
    .with_control_state(shared_control)
    .with_max_iterations(max_steps)
    .with_observer(observer))
}
