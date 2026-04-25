use std::env;
use std::sync::mpsc;

use super::{AllowedToolSet, CliError, CliToolExecutor, DEFAULT_DATE, LlmRuntimeClient};
use crate::tui::ReplTuiEvent;
use crawler::{mvp_tool_specs, SharedApiClient};
use runtime::{
    load_settings, load_system_prompt, settings_get_max_steps, ConfigLoader,
    ConversationRuntime, Session,
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

#[allow(clippy::too_many_arguments)]
pub(super) fn build_runtime(
    mut session: Session,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    ui_tx: Option<mpsc::Sender<ReplTuiEvent>>,
) -> Result<ConversationRuntime<LlmRuntimeClient, CliToolExecutor>, CliError> {
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
