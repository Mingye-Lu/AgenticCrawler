use std::env;
use std::sync::Arc;

use super::{AllowedToolSet, CliError, CliToolExecutor, LlmRuntimeClient, DEFAULT_DATE};
use crawler::{mvp_tool_specs, SharedApiClient};
use runtime::{
    load_settings, load_system_prompt, settings_get_max_steps, ConfigLoader, ControlState,
    ConversationRuntime, RuntimeObserver, Session,
};

pub(super) fn build_system_prompt() -> Result<Vec<String>, CliError> {
    let mut sections = crawler::build_system_prompt(&mvp_tool_specs());
    sections.extend(load_system_prompt(
        env::current_dir()?,
        DEFAULT_DATE,
        env::consts::OS,
        "unknown",
    )?);
    Ok(sections)
}

pub(super) fn build_runtime_feature_config() -> Result<runtime::RuntimeFeatureConfig, CliError> {
    let cwd = env::current_dir()?;
    Ok(ConfigLoader::default_for(cwd)
        .load()?
        .feature_config()
        .clone())
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
) -> Result<ConversationRuntime<LlmRuntimeClient, CliToolExecutor>, CliError> {
    session.model = Some(model.clone());
    let max_steps = settings_get_max_steps(&load_settings()) as usize;
    let shared_control = control_state.unwrap_or_default();
    let fork_client = SharedApiClient::new(LlmRuntimeClient::new(
        model.clone(),
        enable_tools,
        allowed_tools.clone(),
    ));
    Ok(ConversationRuntime::new_with_features(
        session,
        LlmRuntimeClient::new(model, enable_tools, allowed_tools.clone()),
        CliToolExecutor::new(
            allowed_tools,
            fork_client,
            is_interactive,
            Some(Arc::clone(&shared_control)),
        ),
        system_prompt,
        &build_runtime_feature_config()?,
    )
    .with_control_state(shared_control)
    .with_max_iterations(max_steps)
    .with_observer(observer))
}
