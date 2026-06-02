use std::env;
use std::fs;

use super::*;
use crate::auth::{
    interactive_login_prompt, prompt_provider_choice, provider_choice_label, resolve_provider_arg,
};
use browser::generate_bridge_token;
use commands::SlashCommand;
use render::format::{
    format_compact_report, format_cost_report, format_model_report, format_model_switch_report,
    format_status_report, render_config_report, render_export_text, render_repl_help,
    render_version_report, resolve_export_path, status_context, StatusUsage,
};
use runtime::CompactionConfig;

#[allow(clippy::too_many_lines)]
impl LiveCli {
    pub fn handle_repl_command(&mut self, command: SlashCommand) -> Result<bool, CliError> {
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
            SlashCommand::Unknown(name) => {
                eprintln!("unknown slash command: /{name}");
                false
            }
        })
    }

    pub fn reset_browser(&mut self) {
        self.runtime.tool_executor_mut().reset_browser();
    }

    pub fn switch_to_cloakbrowser(&mut self) -> String {
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

    pub fn stop_extension_server(&mut self) -> String {
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

    pub fn extension_bridge_status(&self) -> Option<String> {
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

    pub fn start_extension_server(&mut self) -> Result<(String, u16), String> {
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

    pub(super) fn boot_bridge_server_if_needed(&mut self) {
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

    pub fn status_report(&self) -> String {
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

    pub fn model_command(&mut self, model: Option<String>) -> Result<CommandUiResult, CliError> {
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

    pub fn clear_session_command(&mut self) -> Result<CommandUiResult, CliError> {
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
                self.model, self.session.id
            ),
            persist_after: false,
        })
    }

    pub fn cost_report(&self) -> String {
        format_cost_report(self.runtime.usage().cumulative_usage())
    }

    pub fn config_report(section: Option<&str>) -> Result<String, CliError> {
        Ok(render_config_report(section)?)
    }

    #[must_use]
    pub fn version_report() -> String {
        render_version_report()
    }

    pub fn export_session_report(&self, requested_path: Option<&str>) -> Result<String, CliError> {
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

    pub fn refresh_runtime_auth(&mut self) -> Result<(), CliError> {
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

    pub fn compact_command(&mut self) -> Result<CommandUiResult, CliError> {
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
