use std::env;

use super::{AllowedToolSet, CliError, CliToolExecutor, DEFAULT_DATE, LlmRuntimeClient};
use crawler::{mvp_tool_specs, SharedApiClient};
use runtime::{
    load_settings, load_system_prompt, settings_get_max_steps, ConfigLoader,
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
    mut session: Session,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    allowed_tools: Option<AllowedToolSet>,
    observer: Box<dyn RuntimeObserver + Send>,
) -> Result<ConversationRuntime<LlmRuntimeClient, CliToolExecutor>, CliError> {
    session.model = Some(model.clone());
    let max_steps = settings_get_max_steps(&load_settings()) as usize;
    let fork_client = SharedApiClient::new(LlmRuntimeClient::new(model.clone(), enable_tools, allowed_tools.clone()));
    Ok(ConversationRuntime::new_with_features(
        session,
        LlmRuntimeClient::new(model, enable_tools, allowed_tools.clone()),
        CliToolExecutor::new(allowed_tools, fork_client),
        system_prompt,
        &build_runtime_feature_config()?,
    )
    .with_max_iterations(max_steps)
    .with_observer(observer))
}
