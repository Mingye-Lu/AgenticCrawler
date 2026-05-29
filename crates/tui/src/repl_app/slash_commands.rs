use super::oauth_spawn::{
    spawn_anthropic_oauth_thread, spawn_extension_connection_watch_from_receiver,
};
use super::*;

impl ReplTuiState {
    pub(super) fn refresh_slash_overlay(&mut self) {
        if !self.input.text.trim_start().starts_with('/') {
            self.slash_overlay = None;
            self.last_slash_overlay_rect = None;
            return;
        }
        let trimmed = self.input.text.trim();
        if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
            self.slash_overlay = None;
            self.last_slash_overlay_rect = None;
            return;
        }

        let trimmed_lower = trimmed.to_ascii_lowercase();
        let candidates = slash_command_specs()
            .iter()
            .map(|spec| SlashOverlayItem {
                command: format!("/{}", spec.name),
                summary: spec.summary,
            })
            .filter(|item| item.command.starts_with(&trimmed_lower))
            .collect::<Vec<_>>();

        if candidates.is_empty() {
            self.slash_overlay = None;
            self.last_slash_overlay_rect = None;
            return;
        }

        let (selected, mut scroll_offset) = self.slash_overlay.as_ref().map_or((0, 0), |prev| {
            (prev.selected.min(candidates.len() - 1), prev.scroll_offset)
        });
        let visible_count = min(candidates.len(), SLASH_OVERLAY_VISIBLE_ITEMS);
        if selected < scroll_offset {
            scroll_offset = selected;
        } else if selected >= scroll_offset + visible_count {
            scroll_offset = selected - visible_count + 1;
        }
        let max_offset = candidates.len().saturating_sub(visible_count);
        scroll_offset = scroll_offset.min(max_offset);

        self.slash_overlay = Some(SlashOverlay {
            items: candidates,
            selected,
            scroll_offset,
        });
    }

    pub(super) fn selected_slash_command(&self) -> Option<String> {
        self.slash_overlay.as_ref().and_then(|overlay| {
            overlay
                .items
                .get(overlay.selected)
                .map(|item| item.command.clone())
        })
    }

    pub(super) fn slash_overlay_select_prev(&mut self) {
        if let Some(overlay) = self.slash_overlay.as_mut() {
            if overlay.selected > 0 {
                overlay.selected -= 1;
                if overlay.selected < overlay.scroll_offset {
                    overlay.scroll_offset = overlay.selected;
                }
            }
        }
    }

    pub(super) fn slash_overlay_select_next(&mut self) {
        if let Some(overlay) = self.slash_overlay.as_mut() {
            overlay.selected = min(overlay.selected + 1, overlay.items.len() - 1);
            let visible_count = min(overlay.items.len(), SLASH_OVERLAY_VISIBLE_ITEMS);
            if overlay.selected >= overlay.scroll_offset + visible_count {
                overlay.scroll_offset = overlay.selected - visible_count + 1;
            }
        }
    }
}

pub(super) fn build_session_modal_entries(
    current_id: &str,
) -> Result<Vec<SessionModalEntry>, Box<dyn std::error::Error>> {
    let summaries = crate::session_mgr::list_managed_sessions()?;
    let dir = crate::session_mgr::sessions_dir();
    Ok(summaries
        .into_iter()
        .map(|s| SessionModalEntry {
            path: dir.join(format!("{}.json", s.id)),
            is_current: s.id == current_id,
            id: s.id,
            title: s.title,
            modified_epoch_secs: s.modified_epoch_secs,
            message_count: s.message_count,
        })
        .collect())
}

#[allow(clippy::too_many_lines)]
pub(super) fn handle_session_modal_outcome(
    state: &mut ReplTuiState,
    cli: &Arc<Mutex<LiveCli>>,
    outcome: crate::tui::session_modal::SessionModalOutcome,
) {
    use crate::tui::session_modal::SessionModalOutcome;
    match outcome {
        SessionModalOutcome::None => {}
        SessionModalOutcome::Switch { id, path } => {
            state.active_modal = None;
            match cli.lock() {
                Ok(mut guard) => {
                    let handle = crate::session_mgr::SessionHandle {
                        id: id.clone(),
                        path: path.clone(),
                    };
                    match guard.switch_to_session_handle(handle) {
                        Ok(message_count) => {
                            let _ = guard.persist_session();
                            // Bulk-load messages from the newly switched session
                            let loaded_messages = guard.session_messages();
                            state.messages = loaded_messages;
                            state.live_tool_calls.clear();
                            state.typewriter.chars.clear();
                            state.typewriter.live.clear();
                            state.busy = false;
                            state.cancelling = false;
                            state.current_tool = None;
                            state.follow_bottom = true;
                            let child_sessions_data = guard.session_child_sessions();
                            state.child_tab_panel = if child_sessions_data.is_empty() {
                                child_tabs::ChildTabPanel::default()
                            } else {
                                child_tabs::hydrate_from_child_sessions(&child_sessions_data)
                            };
                            state.status_line = "Ready".to_string();
                            state.push_system_card(
                                "Session",
                                &format!(
                                    "Session switched\n  Active session   {}\n  File             {}\n  Messages         {}",
                                    id,
                                    path.display(),
                                    message_count
                                ),
                            );
                        }
                        Err(e) => {
                            state.push_system_card(
                                "Session Error",
                                &format!("Failed to switch session: {e}"),
                            );
                        }
                    }
                }
                Err(_) => {
                    state.push_system_card("Session Error", "Failed to acquire CLI lock.");
                }
            }
        }
        SessionModalOutcome::Delete {
            id,
            path,
            is_current,
        } => {
            if let Err(e) = crate::session_mgr::delete_session(&path) {
                state.push_system_card("Session Error", &format!("Failed to delete session: {e}"));
            }
            let new_current_id = if is_current {
                match cli.lock() {
                    Ok(mut guard) => {
                        if let Err(e) = guard.clear_session_command() {
                            state.push_system_card(
                                "Session Error",
                                &format!("Deleted current session but failed to reset: {e}"),
                            );
                        }
                        state.messages.clear();
                        state.live_tool_calls.clear();
                        state.typewriter.chars.clear();
                        state.typewriter.live.clear();
                        state.busy = false;
                        state.current_tool = None;
                        state.follow_bottom = true;
                        guard.session_id().to_string()
                    }
                    Err(_) => id.clone(),
                }
            } else {
                cli.lock().map(|g| g.session_id().to_string()).unwrap_or(id)
            };
            match build_session_modal_entries(&new_current_id) {
                Ok(entries) => {
                    if let Some(modal) = state
                        .active_modal
                        .as_mut()
                        .and_then(ActiveModal::as_session_mut)
                    {
                        modal.set_entries(entries);
                    }
                }
                Err(e) => {
                    state.push_system_card(
                        "Session Error",
                        &format!("Failed to refresh session list: {e}"),
                    );
                }
            }
        }
        SessionModalOutcome::Rename { id: _, path, title } => {
            if let Err(e) = crate::session_mgr::rename_session(&path, &title) {
                state.push_system_card("Session Error", &format!("Failed to rename session: {e}"));
            }
            let current_id = cli
                .lock()
                .map(|g| g.session_id().to_string())
                .unwrap_or_default();
            match build_session_modal_entries(&current_id) {
                Ok(entries) => {
                    if let Some(modal) = state
                        .active_modal
                        .as_mut()
                        .and_then(ActiveModal::as_session_mut)
                    {
                        modal.set_entries(entries);
                    }
                }
                Err(e) => {
                    state.push_system_card(
                        "Session Error",
                        &format!("Failed to refresh session list: {e}"),
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn handle_slash_command_tui(
    terminal: &mut DefaultTerminal,
    state: &mut ReplTuiState,
    cli: &Arc<Mutex<LiveCli>>,
    ui_tx: &Sender<ReplTuiEvent>,
    cmd: SlashCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        SlashCommand::Help => {
            state.push_system_card("Slash Help", &render_repl_help());
        }
        SlashCommand::Status => {
            let report = cli.lock().expect("cli lock").status_report();
            state.push_system_card("Status", &report);
        }
        SlashCommand::Cost => {
            let report = cli.lock().expect("cli lock").cost_report();
            state.push_system_card("Cost", &report);
        }
        SlashCommand::Model { model } => {
            if let Some(model_name) = model {
                let mut g = cli.lock().expect("cli lock");
                let result = g.model_command(Some(model_name))?;
                if result.persist_after {
                    g.persist_session()?;
                }
                state.push_system_card("Model", &result.message);
            } else if state.busy {
                state.push_system_card("Model", "Cannot switch models while the agent is running.");
            } else {
                let credentials = api::load_credentials().unwrap_or_default();
                let registry = api::provider::ProviderRegistry::from_credentials(&credentials);
                let current_model = cli.lock().expect("cli lock").model_name().to_string();
                let (catalog, catalog_source) = match &state.live_model_catalog {
                    ModelCatalogState::Ready(models) => {
                        (models.clone(), crate::tui::model_modal::CatalogSource::Live)
                    }
                    ModelCatalogState::Loading => (
                        api::provider::catalog::builtin_models(),
                        crate::tui::model_modal::CatalogSource::BuiltinWhileLoading,
                    ),
                    ModelCatalogState::Failed => (
                        api::provider::catalog::builtin_models(),
                        crate::tui::model_modal::CatalogSource::BuiltinFallback,
                    ),
                };
                state.active_modal = Some(ActiveModal::Model(
                    crate::tui::model_modal::ModelModal::new(
                        &registry,
                        &current_model,
                        catalog,
                        catalog_source,
                    ),
                ));
            }
        }
        SlashCommand::Compact => {
            let mut g = cli.lock().expect("cli lock");
            let result = g.compact_command()?;
            if result.persist_after {
                g.persist_session()?;
            }
            state.push_system_card("Compact", &result.message);
        }
        SlashCommand::Clear => {
            let mut g = cli.lock().expect("cli lock");
            let result = g.clear_session_command()?;
            state.messages.clear();
            state.live_tool_calls.clear();
            state.typewriter.chars.clear();
            state.typewriter.live.clear();
            state.busy = false;
            state.current_tool = None;
            state.follow_bottom = true;
            state.child_tab_panel = crate::child_tabs::ChildTabPanel::default();
            state.view_mode = ViewMode::Parent;
            state.push_system_card("Session", &result.message);
        }
        SlashCommand::Config { section } => {
            let report = LiveCli::config_report(section.as_deref())?;
            state.push_system_card("Config", &report);
        }
        SlashCommand::Version => {
            let report = LiveCli::version_report();
            state.push_system_card("Version", &report);
        }
        SlashCommand::Export { path } => {
            let report = cli
                .lock()
                .expect("cli lock")
                .export_session_report(path.as_deref())?;
            state.push_system_card("Export", &report);
        }
        SlashCommand::Sessions => {
            if state.busy {
                state.push_system_card("Sessions", "Cannot open the session picker while busy.");
                return Ok(());
            }
            let current_id = cli.lock().expect("cli lock").session_id().to_string();
            let entries = build_session_modal_entries(&current_id)?;
            state.active_modal = Some(ActiveModal::Session(
                crate::tui::session_modal::SessionModal::new(entries),
            ));
        }
        SlashCommand::Debug => {
            state.debug_mode = !state.debug_mode;
            let label = if state.debug_mode { "ON" } else { "OFF" };
            state.push_system(&format!(
                "Debug mode {label}  tool calls show {}",
                if state.debug_mode {
                    "expanded output"
                } else {
                    "compact summary"
                }
            ));
        }
        SlashCommand::Headed => {
            if cli.lock().expect("cli lock").is_extension_mode_active() {
                state.push_system_card(
                    "Browser Mode",
                    "Browser mode\n  Ignored          extension mode is active (browser is already visible)",
                );
            } else {
                #[cfg(target_os = "linux")]
                let has_display = std::env::var_os("DISPLAY").is_some()
                    || std::env::var_os("WAYLAND_DISPLAY").is_some();
                #[cfg(not(target_os = "linux"))]
                let has_display = true;

                if has_display {
                    std::env::set_var("HEADLESS", "false");
                    let _ = runtime::update_settings(|s| {
                        s.headless = Some(false);
                    });
                    cli.lock().expect("cli lock").reset_browser();
                    state.push_system_card(
                        "Browser Mode",
                        "Browser mode\n  Result           switched to headed (visible)",
                    );
                } else {
                    state.push_system_card(
                        "Browser Mode",
                        "Browser mode\n  Error            No display server found ($DISPLAY / $WAYLAND_DISPLAY not set).\n                   Run inside a desktop session or use `xvfb-run` to create a virtual display.",
                    );
                }
            }
        }
        SlashCommand::Headless => {
            if cli.lock().expect("cli lock").is_extension_mode_active() {
                state.push_system_card(
                    "Browser Mode",
                    "Browser mode\n  Ignored          extension mode is active (browser is already visible)",
                );
            } else {
                std::env::set_var("HEADLESS", "true");
                let _ = runtime::update_settings(|s| {
                    s.headless = Some(true);
                });
                cli.lock().expect("cli lock").reset_browser();
                state.push_system_card(
                    "Browser Mode",
                    "Browser mode\n  Result           switched to headless",
                );
            }
        }
        cmd @ (SlashCommand::Extension { .. } | SlashCommand::CloakBrowser) => match cmd {
            SlashCommand::Extension { stop } => {
                if stop {
                    let mut g = cli.lock().expect("cli lock");
                    let msg = g.stop_extension_server();
                    state.push_system_card("Extension", &msg);
                    return Ok(());
                }
                let mut g = cli.lock().expect("cli lock");
                if let Some(status) = g.extension_bridge_status() {
                    state.push_system_card("Extension", &status);
                } else {
                    match g.start_extension_server() {
                        Ok((token, port)) => {
                            state.push_system_card(
                                "Extension",
                                &format!(
                                    "Extension bridge\n  \
                                     Status           server started (port {port})\n  \
                                     Token            {token}"
                                ),
                            );
                            if let Some(watch) = g.extension_connection_watch() {
                                drop(g);
                                spawn_extension_connection_watch_from_receiver(watch, cli, ui_tx);
                            }
                        }
                        Err(e) => {
                            state.push_system_card("Extension", &e);
                        }
                    }
                }
            }
            SlashCommand::CloakBrowser => {
                let mut g = cli.lock().expect("cli lock");
                let msg = g.switch_to_cloakbrowser();
                state.push_system_card("Browser Mode", &msg);
            }
            _ => unreachable!(),
        },
        SlashCommand::Auth { provider } => {
            if state.busy {
                state.push_system("Please wait for the current task to finish.");
                return Ok(());
            }
            let parsed_provider = provider
                .as_deref()
                .and_then(|p| crate::auth::resolve_provider_arg(p).ok());
            let is_anthropic_legacy = matches!(
                parsed_provider,
                Some(ProviderChoice::Legacy(crate::app::Provider::Anthropic))
            );
            state.active_modal = Some(ActiveModal::Auth(AuthModal::new_with_choice(
                ui_tx.clone(),
                parsed_provider,
            )));
            if is_anthropic_legacy {
                spawn_anthropic_oauth_thread(ui_tx.clone(), &mut state.active_modal);
            }
        }
        other @ SlashCommand::Unknown(_) => {
            suspend_for_stdout(terminal, || {
                let mut g = cli.lock().expect("cli lock");
                let _ = g.handle_repl_command(other);
            })?;
            state.push_system("(slash command output printed to stdout)");
        }
    }
    Ok(())
}
