use super::oauth_spawn::{
    spawn_anthropic_oauth_thread, spawn_extension_connection_watch, spawn_openai_oauth_thread,
};
use super::slash_commands::{handle_session_modal_outcome, handle_slash_command_tui};
use super::*;

impl ReplTuiState {
    pub(crate) fn clamp_scroll_offset(&mut self) {
        let max_offset = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        if self.list_state.offset() > max_offset {
            *self.list_state.offset_mut() = max_offset;
        }
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        let max_offset = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        *self.list_state.offset_mut() = max_offset;
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn drain_events(&mut self, rx: &Receiver<ReplTuiEvent>) {
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Ok(ev) = rx.try_recv() else { break };
            match ev {
                ReplTuiEvent::StreamText(s) => {
                    // Enqueue raw chars for typewriter reveal.
                    for c in s.chars() {
                        self.typewriter.chars.push_back(c);
                    }
                    // If the producer is faster than the reveal can keep up,
                    // bypass the slow per-char reveal so the queue doesn't
                    // grow unbounded.
                    if self.typewriter.chars.len() > MAX_TYPEWRITER_BACKLOG {
                        self.flush_typewriter();
                    }
                }
                ReplTuiEvent::TurnStarting => {
                    self.busy = true;
                    self.live_tool_calls.clear();
                    self.current_tool = None;
                    self.status_line = "Thinking...".to_string();
                }
                ReplTuiEvent::ToolCallStart { name, input } => {
                    self.handle_tool_call_start(name, &input);
                }
                ReplTuiEvent::ToolCallComplete {
                    name,
                    output,
                    is_error,
                } => {
                    self.handle_tool_call_complete(&name, output, is_error);
                }
                ReplTuiEvent::TurnFinished(result) => {
                    self.busy = false;
                    self.cancelling = false;
                    self.current_tool = None;
                    self.live_tool_calls.clear();

                    self.status_line = match &result {
                        Ok(()) => "Ready".to_string(),
                        Err(e) => format!("Error: {e}"),
                    };

                    self.flush_typewriter();
                    if let Err(e) = result {
                        self.push_system(&format!("Error: {e}"));
                    }
                }
                ReplTuiEvent::SystemMessage(s) => {
                    self.push_system(&s);
                }
                ReplTuiEvent::MessageCompleted(message) => {
                    self.typewriter.chars.clear();
                    self.typewriter.live.clear();
                    self.messages.push(message);
                    self.follow_bottom = true;
                }
                ReplTuiEvent::MessagesLoaded(messages) => {
                    self.messages = messages;
                    self.live_tool_calls.clear();
                    self.typewriter.chars.clear();
                    self.typewriter.live.clear();
                    self.current_tool = None;
                    self.busy = false;
                    self.cancelling = false;
                    self.status_line = "Ready".to_string();
                    self.follow_bottom = true;
                }
                ReplTuiEvent::AuthOAuthComplete { provider, result } => {
                    if let Some(modal) = self
                        .active_modal
                        .as_mut()
                        .and_then(ActiveModal::as_auth_mut)
                    {
                        modal.step = match result {
                            Ok(()) => {
                                let provider_kind = match crate::app::parse_provider_arg(&provider)
                                    .unwrap_or(crate::app::Provider::Anthropic)
                                {
                                    crate::app::Provider::Anthropic => {
                                        crate::tui::auth_modal::ProviderKind::Anthropic
                                    }
                                    crate::app::Provider::OpenAi => {
                                        crate::tui::auth_modal::ProviderKind::OpenAi
                                    }
                                    crate::app::Provider::Other => {
                                        crate::tui::auth_modal::ProviderKind::Other
                                    }
                                };

                                let mut store = crate::auth::load_credentials_or_warn();
                                let provider_str = match provider_kind {
                                    crate::tui::auth_modal::ProviderKind::Anthropic => "anthropic",
                                    crate::tui::auth_modal::ProviderKind::OpenAi => "openai",
                                    crate::tui::auth_modal::ProviderKind::Other => "other",
                                    crate::tui::auth_modal::ProviderKind::Preset(p) => p.id,
                                };
                                let mut config = store
                                    .providers
                                    .get(provider_str)
                                    .cloned()
                                    .unwrap_or_default();
                                config.auth_method = "oauth".to_string();
                                api::credentials::set_provider_config(
                                    &mut store,
                                    provider_str,
                                    config,
                                );
                                let _ = api::credentials::save_credentials(&store);

                                AuthModalStep::ModelFetchLoading {
                                    provider: provider_kind,
                                }
                            }
                            Err(e) => AuthModalStep::Error { message: e },
                        };
                    }
                }
                ReplTuiEvent::AuthOAuthProgress { message } => {
                    if let Some(modal) = self
                        .active_modal
                        .as_mut()
                        .and_then(ActiveModal::as_auth_mut)
                    {
                        if let AuthModalStep::OAuthWaiting { status, .. } = &mut modal.step {
                            *status = message;
                        }
                    }
                }
                ReplTuiEvent::AuthModelsLoaded(result) => {
                    if let Some(modal) = self
                        .active_modal
                        .as_mut()
                        .and_then(ActiveModal::as_auth_mut)
                    {
                        modal.finish_model_loading(result);
                    }
                }
                ReplTuiEvent::ModelCatalogReady(models) => {
                    self.live_model_catalog = if models.is_empty() {
                        ModelCatalogState::Failed
                    } else {
                        ModelCatalogState::Ready(models)
                    };
                }
                ReplTuiEvent::PauseStarted(reason) => {
                    self.paused = true;
                    self.pause_reason = reason;
                    self.status_line = format!(
                        "PAUSED: {} -- Solve in browser, press Enter to resume",
                        self.pause_reason
                    );
                }
                ReplTuiEvent::PauseEnded => {
                    self.paused = false;
                    self.pause_reason = String::new();
                    self.status_line = "Thinking...".to_string();
                }
                ReplTuiEvent::ChildEvent(_) => {}
                ReplTuiEvent::ExtensionBridgeResult { success, message } => {
                    let title = if success {
                        "Extension"
                    } else {
                        "Extension Error"
                    };
                    self.push_system_card(title, &message);
                }
            }
        }

        // Drain child agent events
        if let Some(ref child_rx) = self.child_event_rx {
            while let Ok(child_ev) = child_rx.try_recv() {
                self.child_tab_panel.apply_event(
                    &child_ev.child_id,
                    &child_ev.sub_goal,
                    &child_ev.event,
                );
                if matches!(child_ev.event, ChildEventKind::PauseRequested { .. })
                    && matches!(self.view_mode, ViewMode::Parent)
                {
                    self.view_mode = ViewMode::Child(child_ev.child_id.clone());
                }
            }
        }
    }

    pub(super) fn reconcile_child_view_mode(&mut self) {
        if self.child_tab_panel.tabs.is_empty() {
            if matches!(self.view_mode, ViewMode::Child(_)) {
                self.view_mode = ViewMode::Parent;
            }
            return;
        }

        if let ViewMode::Child(child_id) = &self.view_mode {
            let child_exists = self
                .child_tab_panel
                .tabs
                .iter()
                .any(|tab| tab.child_id == *child_id);
            if !child_exists {
                self.view_mode = ViewMode::Parent;
            }
        }
    }
}

/// Interactive REPL using Ratatui. Requires a TTY on stdout - the caller must gate accordingly.
pub(crate) fn run_repl_ratatui(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (ui_tx, ui_rx) = mpsc::channel::<ReplTuiEvent>();
    let (work_tx, work_rx) = mpsc::channel::<WorkerMsg>();

    let cli = Arc::new(Mutex::new(LiveCli::new_with_ui_tx(
        model,
        true,
        allowed_tools,
        ui_tx.clone(),
    )?));

    spawn_extension_connection_watch(&cli, &ui_tx);

    let cli_worker = Arc::clone(&cli);
    thread::spawn(move || {
        while let Ok(msg) = work_rx.recv() {
            match msg {
                WorkerMsg::RunTurn(line) => {
                    let mut g = cli_worker.lock().expect("cli lock");
                    let _ = g.run_turn_tui(&line);
                }
                WorkerMsg::Shutdown => break,
            }
        }
    });

    let cancel_flag = cli.lock().expect("cli lock").cancel_flag();

    acrawl_core::set_tui_active(true);
    let mut terminal = ratatui::init();
    let work_shutdown = work_tx.clone();
    let result = run_loop(&mut terminal, &ui_rx, &ui_tx, &work_tx, &cli, &cancel_flag);
    let _ = work_shutdown.send(WorkerMsg::Shutdown);
    ratatui::restore();
    acrawl_core::set_tui_active(false);
    result
}

#[allow(clippy::too_many_lines)]
fn run_loop(
    terminal: &mut DefaultTerminal,
    ui_rx: &Receiver<ReplTuiEvent>,
    ui_tx: &Sender<ReplTuiEvent>,
    work_tx: &Sender<WorkerMsg>,
    cli: &Arc<Mutex<LiveCli>>,
    cancel_flag: &Arc<ControlState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = execute!(io::stdout(), event::EnableMouseCapture);
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let _ = execute!(io::stdout(), SetCursorStyle::SteadyBar);
    let _mouse_guard = MouseCaptureGuard;

    let mut state = ReplTuiState::new();

    // Claim child event receiver and registry from LiveCli
    {
        let mut g = cli.lock().expect("cli lock");
        state.child_event_rx = g.take_child_event_rx();
        state.child_control_registry = g.take_child_control_registry();
    }

    // Extension bridge is started explicitly via /extension.

    let (update_tx, update_rx) =
        tokio::sync::oneshot::channel::<Option<runtime::update_check::UpdateInfo>>();
    state.update_rx = Some(update_rx);
    crate::TOKIO_RUNTIME.get().unwrap().spawn(async move {
        let info = runtime::update_check::check_for_update().await;
        let _ = update_tx.send(info);
    });

    {
        let ui_tx_catalog = ui_tx.clone();
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime for catalog fetch");
            let models = rt.block_on(api::provider::catalog::fetch_all_models_dev_for_picker());
            let _ = ui_tx_catalog.send(ReplTuiEvent::ModelCatalogReady(models));
        });
    }

    loop {
        // Flush paste-burst buffer when the burst has been idle longer than
        // the threshold ￀covers pastes that end without a subsequent key.
        if !state.paste_burst_chars.is_empty()
            && state.last_key_time.is_some_and(|t| {
                t.elapsed() > Duration::from_millis(ReplTuiState::PASTE_BURST_THRESHOLD_MS)
            })
        {
            state.flush_paste_burst();
        }

        state.drain_events(ui_rx);
        state.reconcile_child_view_mode();

        if let Some(rx) = state.update_rx.as_mut() {
            if let Ok(info) = rx.try_recv() {
                state.update_info = info;
                state.update_rx = None;
            }
        }

        // Detect pause state directly from ControlState ￀the observer event may not
        // fire for LLM-triggered pauses (handle_pause blocks inline before the runtime
        // can notify the observer).
        if !state.paused && state.busy && cancel_flag.is_paused() {
            state.paused = true;
            state.pause_reason = state
                .last_wait_for_human_reason
                .clone()
                .unwrap_or_else(|| "Human intervention requested".to_string());
            state.status_line = format!(
                "PAUSED: {} -- Solve in browser, press Enter to resume",
                state.pause_reason
            );
        }

        if state.exit {
            if state.persist_on_exit {
                let mut g = cli.lock().expect("cli lock");
                g.persist_session()?;
            }
            break;
        }

        if let Ok(g) = cli.try_lock() {
            state.cached_header = build_header_snapshot(&g);
        }
        state.tick_input_caret();
        state.refresh_slash_overlay();
        let header = state.cached_header.clone();

        terminal.draw(|frame| {
            let show_input_cursor = state.active_modal.is_none();
            if state.view_mode == ViewMode::Parent {
                match state.ui_state {
                    AppUiState::WelcomeMode => {
                        draw_welcome(frame, frame.area(), &mut state, show_input_cursor);
                    }
                    AppUiState::ChatMode => {
                        draw_chat(frame, &mut state, &header, show_input_cursor);
                    }
                }
            } else {
                crate::tui::repl_render::draw_child_view(frame, &mut state);
            }
            if let Some(ref modal) = state.active_modal {
                modal.draw(frame, frame.area());
            }
        })?;

        if let Some(ref mut modal) = state.active_modal {
            modal.process_loading();
        }

        if !state.typewriter.chars.is_empty() {
            let q_len = state.typewriter.chars.len();
            let count = if q_len > 100 {
                8
            } else if q_len > 30 {
                4
            } else {
                2
            };
            state.tick_typewriter(count);
        }

        // Shorter poll when typewriter has pending chars so reveal feels smooth
        let poll_ms = if state.typewriter.chars.is_empty() {
            50
        } else {
            16
        };
        if !event::poll(Duration::from_millis(poll_ms))? {
            continue;
        }

        let ev = event::read()?;
        match ev {
            Event::Mouse(me) => {
                if matches!(
                    me.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) && state.active_modal.is_some()
                {
                    if let Some(modal) = state.active_modal.as_mut() {
                        if modal.supports_vertical_wheel() {
                            modal.handle_vertical_wheel(matches!(
                                me.kind,
                                MouseEventKind::ScrollDown
                            ));
                        }
                    }
                    continue;
                }

                if let Some(overlay_rect) = state.last_slash_overlay_rect {
                    if rect_contains_mouse(overlay_rect, me.column, me.row) {
                        match me.kind {
                            MouseEventKind::ScrollUp => {
                                state.slash_overlay_select_prev();
                                continue;
                            }
                            MouseEventKind::ScrollDown => {
                                state.slash_overlay_select_next();
                                continue;
                            }
                            _ => {}
                        }
                    }
                }

                let in_transcript =
                    rect_contains_mouse(state.last_transcript_rect, me.column, me.row);
                let in_input = rect_contains_mouse(state.last_input_rect, me.column, me.row);

                if let ViewMode::Child(ref id) = state.view_mode {
                    match me.kind {
                        MouseEventKind::ScrollUp => {
                            if let Some(tab) = state.child_tab_panel.find_tab_mut(id) {
                                tab.follow_bottom = false;
                                *tab.list_state.offset_mut() =
                                    tab.list_state.offset().saturating_sub(6);
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            if let Some(tab) = state.child_tab_panel.find_tab_mut(id) {
                                let max = tab
                                    .last_wrapped_len
                                    .saturating_sub(tab.last_view_height.max(1));
                                *tab.list_state.offset_mut() =
                                    (tab.list_state.offset().saturating_add(6)).min(max);
                                tab.follow_bottom = tab.list_state.offset() >= max;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            let tr = state.last_transcript_rect;
                            if rect_contains_mouse(tr, me.column, me.row) {
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(id) {
                                    let cur = tab.list_state.offset();
                                    let col = me.column.saturating_sub(tr.x);
                                    let row = cur + usize::from(me.row.saturating_sub(tr.y));
                                    state.selection.anchor = Some((col, row));
                                    state.selection.end = Some((col, row));
                                    state.selection.mouse_drag_occurred = false;
                                }
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            let tr = state.last_transcript_rect;
                            if let Some(tab) = state.child_tab_panel.find_tab_mut(id) {
                                let cur = tab.list_state.offset();
                                let col = me
                                    .column
                                    .saturating_sub(tr.x)
                                    .min(tr.width.saturating_sub(1));
                                let row = cur
                                    + usize::from(
                                        me.row
                                            .saturating_sub(tr.y)
                                            .min(tr.height.saturating_sub(1)),
                                    );
                                state.selection.end = Some((col, row));
                                state.selection.mouse_drag_occurred = true;
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if state.selection.mouse_drag_occurred {
                                let tr = state.last_transcript_rect;
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(id) {
                                    let cur = tab.list_state.offset();
                                    let col = me
                                        .column
                                        .saturating_sub(tr.x)
                                        .min(tr.width.saturating_sub(1));
                                    let row = cur
                                        + usize::from(
                                            me.row
                                                .saturating_sub(tr.y)
                                                .min(tr.height.saturating_sub(1)),
                                        );
                                    state.selection.end = Some((col, row));
                                }
                            } else {
                                state.selection.anchor = None;
                                state.selection.end = None;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Right)
                            if state.selection.anchor.is_some() =>
                        {
                            state.selection.pending_copy = Some(true);
                            state.selection.suppress_paste_until =
                                Some(Instant::now() + Duration::from_millis(800));
                        }
                        _ => {}
                    }
                    continue;
                }

                if in_transcript {
                    let max_off = state
                        .last_wrapped_len
                        .saturating_sub(state.last_view_height.max(1));
                    let cur = state.list_state.offset();
                    let tr = state.last_transcript_rect;
                    match me.kind {
                        MouseEventKind::ScrollUp => {
                            *state.list_state.offset_mut() = cur.saturating_sub(6);
                            state.follow_bottom = false;
                        }
                        MouseEventKind::ScrollDown => {
                            *state.list_state.offset_mut() = (cur.saturating_add(6)).min(max_off);
                            state.follow_bottom = state.list_state.offset() >= max_off;
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            let col = me.column.saturating_sub(tr.x);
                            let row = cur + usize::from(me.row.saturating_sub(tr.y));
                            state.selection.anchor = Some((col, row));
                            state.selection.end = Some((col, row));
                            state.selection.mouse_drag_occurred = false;
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            let col = me
                                .column
                                .saturating_sub(tr.x)
                                .min(tr.width.saturating_sub(1));
                            let row = cur
                                + usize::from(
                                    me.row.saturating_sub(tr.y).min(tr.height.saturating_sub(1)),
                                );
                            state.selection.end = Some((col, row));
                            state.selection.mouse_drag_occurred = true;
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if state.selection.mouse_drag_occurred {
                                let col = me
                                    .column
                                    .saturating_sub(tr.x)
                                    .min(tr.width.saturating_sub(1));
                                let row = cur
                                    + usize::from(
                                        me.row
                                            .saturating_sub(tr.y)
                                            .min(tr.height.saturating_sub(1)),
                                    );
                                state.selection.end = Some((col, row));
                            } else {
                                state.selection.anchor = None;
                                state.selection.end = None;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Right)
                            if state.selection.anchor.is_some() =>
                        {
                            state.selection.pending_copy = Some(true);
                            state.selection.suppress_paste_until =
                                Some(Instant::now() + Duration::from_millis(800));
                        }
                        _ => {}
                    }
                } else if in_input || state.ui_state == AppUiState::WelcomeMode {
                    // Map mouse position to a character index in the input text.
                    let ir = state.last_input_rect;
                    let widget_row = usize::from(me.row.saturating_sub(ir.y));
                    let widget_col = usize::from(me.column.saturating_sub(ir.x));
                    let char_idx = state.char_index_at_mouse(widget_row, widget_col);

                    match me.kind {
                        MouseEventKind::ScrollUp => {
                            state.input_scroll_offset = state.input_scroll_offset.saturating_sub(1);
                            state.input_scroll_manual = true;
                        }
                        MouseEventKind::ScrollDown => {
                            state.input_scroll_offset = state.input_scroll_offset.saturating_add(1);
                            state.input_scroll_manual = true;
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            if let Some(idx) = char_idx {
                                state.set_input_cursor_line_col_by_char(idx);
                                state.input_selection = Some((idx, idx));
                                state.input_click_anchor = Some(idx);
                                state.selection.anchor = None;
                                state.selection.end = None;
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some(idx) = char_idx {
                                if let Some(anchor) = state.input_click_anchor {
                                    let a = anchor.min(idx);
                                    let b = anchor.max(idx);
                                    state.input_selection = Some((a, b));
                                }
                                state.selection.anchor = None;
                                state.selection.end = None;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Right)
                            if state.input_selection.is_some() =>
                        {
                            // Copy selected text to clipboard.
                            if let Some(text) = state.selected_input_text_expanded() {
                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                    let _ = cb.set_text(text);
                                }
                            }
                            state.input_selection = None;
                            state.input_click_anchor = None;
                        }
                        _ => {
                            // Mouse events that should clear the selection
                            // (e.g. Up, Moved) should NOT clear it here ￀
                            // doing so would wipe the anchor between Down
                            // and the first Drag.  Selection is cleared
                            // explicitly by cursor movement, new clicks,
                            // or copy actions.
                        }
                    }
                }
            }
            Event::Paste(text) => {
                let suppress = state
                    .selection
                    .suppress_paste_until
                    .is_some_and(|deadline| Instant::now() <= deadline);
                state.selection.suppress_paste_until = None;

                if suppress || state.active_modal.is_some() {
                    continue;
                }

                // Bracketed paste supersedes any in-flight burst accumulation.
                state.flush_paste_burst();
                state.last_key_time = None;
                state.handle_paste_event(&text);
                if text.contains('\n') {
                    state.arm_paste_enter_suppression();
                }
                state.wake_input_caret();
                state.refresh_slash_overlay();
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let suppress_paste_key = state.paste_enter_is_suppressed(key.code);
                if suppress_paste_key {
                    state.selection.suppress_paste_until =
                        Some(Instant::now() + Duration::from_millis(150));
                    continue;
                }
                // Any key that isn't bare Char/Enter ends a paste burst ￀flush
                // the buffer here so command handlers (Ctrl-A/Z/Y/W/C/X, etc.)
                // see a consistent input state.
                if !matches!(key.code, KeyCode::Char(_) | KeyCode::Enter)
                    || !key.modifiers.is_empty()
                {
                    state.flush_paste_burst();
                }
                if state
                    .selection
                    .suppress_paste_until
                    .is_some_and(|deadline| Instant::now() > deadline)
                {
                    state.selection.suppress_paste_until = None;
                }

                if !matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                    state.selection.anchor = None;
                    state.selection.end = None;
                }

                if state.active_modal.is_none()
                    && ((key.code == KeyCode::Char('v')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                        || (key.code == KeyCode::Insert
                            && key.modifiers.contains(KeyModifiers::SHIFT)))
                {
                    // Manual paste supersedes any in-flight burst accumulation.
                    state.flush_paste_burst();
                    state.last_key_time = None;
                    if let Some(text) = read_clipboard_text() {
                        state.handle_paste_event(&text);
                        if text.contains('\n') {
                            state.arm_paste_enter_suppression();
                        }
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    continue;
                }

                if state.active_modal.is_none()
                    && key.code == KeyCode::Char('a')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    state.select_all_input();
                    state.wake_input_caret();
                    continue;
                }

                if state.active_modal.is_none()
                    && key.code == KeyCode::Char('z')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    if state.undo_input_edit() {
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    continue;
                }

                if state.active_modal.is_none()
                    && key.code == KeyCode::Char('y')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    if state.redo_input_edit() {
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    continue;
                }

                if state.active_modal.is_none()
                    && key.code == KeyCode::Char('w')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    state.word_backspace();
                    state.wake_input_caret();
                    state.refresh_slash_overlay();
                    continue;
                }

                // Copy input selection (Ctrl+C / Ctrl+Insert).
                if state.active_modal.is_none()
                    && state.input_selection.is_some()
                    && ((key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                        || (key.code == KeyCode::Insert && key.modifiers == KeyModifiers::CONTROL))
                {
                    if let Some(text) = state.selected_input_text_expanded() {
                        if let Ok(mut cb) = arboard::Clipboard::new() {
                            let _ = cb.set_text(text);
                        }
                    }
                    state.input_selection = None;
                    state.selection.anchor = None;
                    state.selection.end = None;
                    continue;
                }

                if state.active_modal.is_none()
                    && state.input_selection.is_some()
                    && key.code == KeyCode::Char('x')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    if let Some(text) = state.cut_input_selection_text() {
                        if let Ok(mut cb) = arboard::Clipboard::new() {
                            let _ = cb.set_text(text);
                        }
                    }
                    state.wake_input_caret();
                    state.refresh_slash_overlay();
                    continue;
                }

                if let ViewMode::Child(child_id) = state.view_mode.clone() {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                    } else {
                        match key.code {
                            KeyCode::Esc | KeyCode::Up => {
                                state.selection.anchor = None;
                                state.selection.end = None;
                                state.view_mode = ViewMode::Parent;
                                continue;
                            }
                            KeyCode::Left => {
                                state.child_tab_panel.prev_tab();
                                if let Some(tab) = state.child_tab_panel.active_tab_state_mut() {
                                    state.view_mode = ViewMode::Child(tab.child_id.clone());
                                } else {
                                    state.view_mode = ViewMode::Parent;
                                }
                                continue;
                            }
                            KeyCode::Right => {
                                state.child_tab_panel.next_tab();
                                if let Some(tab) = state.child_tab_panel.active_tab_state_mut() {
                                    state.view_mode = ViewMode::Child(tab.child_id.clone());
                                } else {
                                    state.view_mode = ViewMode::Parent;
                                }
                                continue;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(&child_id) {
                                    let max = tab
                                        .last_wrapped_len
                                        .saturating_sub(tab.last_view_height.max(1));
                                    *tab.list_state.offset_mut() =
                                        (tab.list_state.offset().saturating_add(1)).min(max);
                                    tab.follow_bottom = false;
                                }
                                continue;
                            }
                            KeyCode::Char('k') => {
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(&child_id) {
                                    tab.follow_bottom = false;
                                    *tab.list_state.offset_mut() =
                                        tab.list_state.offset().saturating_sub(1);
                                }
                                continue;
                            }
                            KeyCode::PageDown => {
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(&child_id) {
                                    let max = tab
                                        .last_wrapped_len
                                        .saturating_sub(tab.last_view_height.max(1));
                                    *tab.list_state.offset_mut() =
                                        (tab.list_state.offset().saturating_add(10)).min(max);
                                    tab.follow_bottom = false;
                                }
                                continue;
                            }
                            KeyCode::PageUp => {
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(&child_id) {
                                    tab.follow_bottom = false;
                                    *tab.list_state.offset_mut() =
                                        tab.list_state.offset().saturating_sub(10);
                                }
                                continue;
                            }
                            KeyCode::Char('G') => {
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(&child_id) {
                                    tab.follow_bottom = true;
                                }
                                continue;
                            }
                            KeyCode::Char('g') => {
                                if let Some(tab) = state.child_tab_panel.find_tab_mut(&child_id) {
                                    tab.follow_bottom = false;
                                    *tab.list_state.offset_mut() = 0;
                                }
                                continue;
                            }
                            KeyCode::Enter => {
                                let should_resume = state
                                    .child_tab_panel
                                    .find_tab_mut(&child_id)
                                    .is_some_and(|tab| {
                                        matches!(
                                            tab.status,
                                            child_tabs::ChildTabStatus::Paused { .. }
                                        )
                                    });
                                if should_resume {
                                    if let Some(registry) = &state.child_control_registry {
                                        if let Some(child_state) = registry.get(&child_id) {
                                            child_state.resume();
                                            state.push_system(&format!(
                                                "Resuming child {child_id}..."
                                            ));
                                        }
                                    }
                                }
                                continue;
                            }
                            _ => continue,
                        }
                    }
                }

                let mut modal_action = None;
                let mut modal_succeeded = false;
                let mut oauth_provider = None;
                let mut model_outcome = None;
                let mut session_outcome = None;

                if let Some(ref mut modal) = state.active_modal {
                    modal_action = Some(modal.handle_key(key));
                    modal_succeeded = modal
                        .as_auth()
                        .is_some_and(|m| matches!(m.step, AuthModalStep::Success { .. }));
                    oauth_provider = modal.as_auth().and_then(|m| match &m.step {
                        AuthModalStep::OAuthWaiting {
                            cancel_tx: None,
                            provider,
                            ..
                        } => Some(*provider),
                        _ => None,
                    });
                    if let Some(m) = modal.as_model_mut() {
                        // `take_outcome` swaps the modal's outcome back to
                        // `None`, so a single Enter press can't be observed
                        // (and applied) twice on the next key event.
                        let taken = m.take_outcome();
                        if !matches!(taken, crate::tui::model_modal::ModelModalOutcome::None) {
                            model_outcome = Some(taken);
                        }
                    }
                    if let Some(m) = modal.as_session_mut() {
                        let taken = m.take_outcome();
                        if !matches!(taken, crate::tui::session_modal::SessionModalOutcome::None) {
                            session_outcome = Some(taken);
                        }
                    }
                }

                if let Some(outcome) = session_outcome {
                    handle_session_modal_outcome(&mut state, cli, outcome);
                    continue;
                }

                if let Some(action) = modal_action {
                    if let Some(prov) = oauth_provider {
                        match prov {
                            crate::tui::auth_modal::ProviderKind::Anthropic => {
                                spawn_anthropic_oauth_thread(
                                    ui_tx.clone(),
                                    &mut state.active_modal,
                                );
                            }
                            crate::tui::auth_modal::ProviderKind::OpenAi => {
                                spawn_openai_oauth_thread(ui_tx.clone(), &mut state.active_modal);
                            }
                            crate::tui::auth_modal::ProviderKind::Other
                            | crate::tui::auth_modal::ProviderKind::Preset(_) => {}
                        }
                    }

                    match action {
                        ModalAction::Consumed => continue,
                        ModalAction::Unhandled => {}
                        ModalAction::Dismiss => {
                            state.active_modal = None;
                            if modal_succeeded {
                                match cli.lock() {
                                    Ok(mut guard) => {
                                        if let Some(preferred_model) =
                                            crate::app::initial_model_from_credentials()
                                        {
                                            if preferred_model != guard.model_name() {
                                                let _ = guard.model_command(Some(preferred_model));
                                            }
                                        }
                                        if let Err(error) = guard.refresh_runtime_auth() {
                                            state.push_system(&format!(
                                                "Auth setup saved, but runtime refresh failed: {error}"
                                            ));
                                        } else {
                                            state.ui_state = AppUiState::WelcomeMode;
                                            state.messages.clear();
                                            state.live_tool_calls.clear();
                                        }
                                    }
                                    Err(_) => {
                                        state.push_system(
                                            "Authentication configured, but runtime lock failed.",
                                        );
                                    }
                                }
                                // Handle pending model switch after auth success
                                if let Some(pending_model) = state.pending_model_after_auth.take() {
                                    match cli.lock() {
                                        Ok(mut guard) => {
                                            match guard.model_command(Some(pending_model.clone())) {
                                                Ok(result) => {
                                                    if result.persist_after {
                                                        let _ = guard.persist_session();
                                                    }
                                                    state
                                                        .push_system_card("Model", &result.message);
                                                }
                                                Err(e) => {
                                                    state.push_system_card(
                                                        "Model Error",
                                                        &format!("Failed to switch model: {e}"),
                                                    );
                                                }
                                            }
                                        }
                                        Err(_) => {
                                            state.push_system_card(
                                                "Model Error",
                                                "Failed to acquire CLI lock for model switch",
                                            );
                                        }
                                    }
                                }
                            } else if let Some(outcome) = model_outcome {
                                match outcome {
                                    crate::tui::model_modal::ModelModalOutcome::SwitchModel {
                                        model_id,
                                    } => match cli.lock() {
                                        Ok(mut guard) => match guard
                                            .model_command(Some(model_id.clone()))
                                        {
                                            Ok(result) => {
                                                if result.persist_after {
                                                    let _ = guard.persist_session();
                                                }
                                                drop(guard);
                                                state.push_system_card("Model", &result.message);
                                            }
                                            Err(e) => {
                                                drop(guard);
                                                state.push_system_card(
                                                    "Model Error",
                                                    &format!("{e}"),
                                                );
                                            }
                                        },
                                        Err(_) => {
                                            state.push_system_card(
                                                "Model Error",
                                                "Failed to acquire CLI lock.",
                                            );
                                        }
                                    },
                                    crate::tui::model_modal::ModelModalOutcome::AuthRequired {
                                        provider_id,
                                        model_id,
                                    } => {
                                        state.pending_model_after_auth = Some(model_id.clone());
                                        let parsed_provider =
                                            crate::app::parse_provider_arg(&provider_id).ok();
                                        state.active_modal = Some(ActiveModal::Auth(
                                            AuthModal::new(ui_tx.clone(), parsed_provider),
                                        ));
                                    }
                                    crate::tui::model_modal::ModelModalOutcome::None => {}
                                }
                            }
                            continue;
                        }
                    }
                }

                if state.active_modal.is_none()
                    && key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    // Only honour Ctrl+C as a global cancel/exit when no modal
                    // is open. Cancelling here while e.g. an AuthModal is mid
                    // OAuth flow would orphan its callback thread; the modal
                    // owns its own cancel path (Esc, or its cancel_tx).
                    if state.busy {
                        cancel_flag.request_cancel();
                        if let Some(registry) = &state.child_control_registry {
                            registry.cancel_all();
                        }
                        state.cancelling = true;
                        state.push_system("Interrupting...");
                        continue;
                    }
                    state.exit = true;
                    state.persist_on_exit = true;
                    continue;
                }

                if key.code == KeyCode::Char('p')
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && state.busy
                    && !state.paused
                {
                    cancel_flag.request_pause();
                    state.push_system("Pausing after current operation...");
                    continue;
                }

                if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if state.busy {
                        continue;
                    }
                    if state.input.text.is_empty() {
                        state.exit = true;
                        state.persist_on_exit = true;
                    }
                    continue;
                }

                if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    let mut g = cli.lock().expect("cli lock");
                    let new_effort = g.cycle_reasoning_effort();
                    state.cached_header = build_header_snapshot(&g);
                    if let Some(effort) = new_effort {
                        state.push_system(&format!("Reasoning effort: {effort}"));
                    } else {
                        state.push_system("Reasoning effort: off");
                    }
                    continue;
                }

                if key.code == KeyCode::Up && state.slash_overlay.is_some() {
                    state.slash_overlay_select_prev();
                    continue;
                }

                if key.code == KeyCode::Down && state.slash_overlay.is_some() {
                    state.slash_overlay_select_next();
                    continue;
                }

                if key.code == KeyCode::Char('x')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(state.view_mode, ViewMode::Parent)
                {
                    if let Some(child_id) = state
                        .child_tab_panel
                        .tabs
                        .get(state.child_tab_panel.active_tab)
                        .map(|tab| tab.child_id.clone())
                    {
                        state.selection.anchor = None;
                        state.selection.end = None;
                        state.view_mode = ViewMode::Child(child_id);
                        continue;
                    }
                }

                match key.code {
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        state.insert_input_char('\n');
                        state.last_key_time = Some(Instant::now());
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Enter => {
                        // Paste-burst Enter: if the previous keystroke arrived
                        // within the burst threshold, treat this Enter as a
                        // pasted `\n` (accumulate, don't submit).  Handles
                        // terminals that deliver pastes as raw keystrokes
                        // instead of `Event::Paste`.
                        let now = Instant::now();
                        if state.in_paste_burst(now) {
                            state.paste_burst_chars.push('\n');
                            state.last_key_time = Some(now);
                            state.wake_input_caret();
                            continue;
                        }
                        state.flush_paste_burst();
                        state.last_key_time = Some(now);
                        // Child tab resume (check before parent pause)
                        if state.child_tab_panel.active_tab_is_paused() {
                            if let Some(child_id) = state.child_tab_panel.active_child_id() {
                                if let Some(registry) = &state.child_control_registry {
                                    if let Some(child_state) = registry.get(child_id) {
                                        child_state.resume();
                                        state.push_system(&format!("Resuming child {child_id}..."));
                                    }
                                }
                            }
                            continue;
                        }
                        if state.paused {
                            cancel_flag.resume();
                            state.paused = false;
                            state.pause_reason = String::new();
                            state.push_system("Resuming...");
                            continue;
                        }
                        let trimmed_peek = state.input.text.trim().to_ascii_lowercase();
                        if trimmed_peek == "/exit" || trimmed_peek == "/quit" {
                            if state.busy {
                                cancel_flag.request_cancel();
                            }
                            state.exit = true;
                            state.persist_on_exit = true;
                            state.reset_input();
                            continue;
                        }
                        if state.busy {
                            state.push_system(
                                "Please wait for the current task to finish before submitting.",
                            );
                            continue;
                        }
                        if state.slash_overlay.is_some() {
                            let trimmed = state.input.text.trim().to_string();
                            if let Some(selected) = state.selected_slash_command() {
                                if selected != trimmed {
                                    state.record_input_undo_snapshot();
                                    state.input.text = selected;
                                    state.input.text.push(' ');
                                    state.input.cursor = state.input.text.chars().count();
                                    state.resync_byte_cursor();
                                    state.input.preferred_col = None;
                                    state.input_scroll_offset = usize::MAX;
                                    state.wake_input_caret();
                                    state.refresh_slash_overlay();
                                    continue;
                                }
                            }
                        }

                        let raw_line = std::mem::take(&mut state.input.text);
                        let line = expand_masks(&raw_line, &state.input.pastes);
                        state.reset_input();
                        state.input_scroll_offset = 0;
                        state.clear_input_history();
                        let trimmed = line.trim().to_string();
                        state.refresh_slash_overlay();
                        if trimmed.is_empty() {
                            state.wake_input_caret();
                            continue;
                        }
                        if let Some(cmd) = SlashCommand::parse(&trimmed) {
                            handle_slash_command_tui(terminal, &mut state, cli, ui_tx, cmd)?;
                            state.wake_input_caret();
                            continue;
                        }
                        state.push_user_line(&trimmed);
                        work_tx.send(WorkerMsg::RunTurn(trimmed))?;
                        state.wake_input_caret();
                    }
                    KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        state.insert_input_char('\n');
                        state.last_key_time = Some(Instant::now());
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Char(c) => {
                        let now = Instant::now();
                        if state.in_paste_burst(now) || !state.paste_burst_chars.is_empty() {
                            state.paste_burst_chars.push(c);
                        } else {
                            state.insert_input_char(c);
                        }
                        state.last_key_time = Some(now);
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Backspace => {
                        state.backspace_input_char();
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Delete => {
                        state.delete_input_char();
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Left => {
                        state.move_input_cursor_left();
                        state.wake_input_caret();
                    }
                    KeyCode::Right => {
                        state.move_input_cursor_right();
                        state.wake_input_caret();
                    }
                    KeyCode::Home => {
                        state.move_input_cursor_home();
                        state.wake_input_caret();
                    }
                    KeyCode::End => {
                        state.move_input_cursor_end();
                        state.wake_input_caret();
                    }
                    KeyCode::Up => {
                        state.move_input_cursor_up();
                        state.wake_input_caret();
                    }
                    KeyCode::Down => {
                        state.move_input_cursor_down();
                        state.wake_input_caret();
                    }
                    KeyCode::Tab => {
                        if state.busy || !state.input.text.trim_start().starts_with('/') {
                            continue;
                        }
                        if let Some(selected) = state.selected_slash_command() {
                            state.record_input_undo_snapshot();
                            state.input.text = selected;
                            state.input.text.push(' ');
                            state.input.cursor = state.input.text.chars().count();
                            state.input.preferred_col = None;
                            state.input_scroll_offset = usize::MAX;
                            state.wake_input_caret();
                            state.refresh_slash_overlay();
                        } else {
                            let prefix = state.input.text.to_ascii_lowercase();
                            let candidates = slash_command_completion_candidates();
                            let matches: Vec<_> = candidates
                                .into_iter()
                                .filter(|candidate| candidate.starts_with(&prefix))
                                .collect();
                            if matches.len() == 1 {
                                state.record_input_undo_snapshot();
                                state.input.text.clone_from(&matches[0]);
                                state.input.text.push(' ');
                                state.input.cursor = state.input.text.chars().count();
                                state.input.preferred_col = None;
                                state.input_scroll_offset = usize::MAX;
                                state.wake_input_caret();
                                state.refresh_slash_overlay();
                            }
                        }
                    }
                    KeyCode::Esc => {
                        if state.busy {
                            let now = Instant::now();
                            if state
                                .last_esc_at
                                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(500))
                            {
                                cancel_flag.request_cancel();
                                if let Some(registry) = &state.child_control_registry {
                                    registry.cancel_all();
                                }
                                state.cancelling = true;
                                state.push_system("Interrupting...");
                                state.last_esc_at = None;
                            } else {
                                state.last_esc_at = Some(now);
                            }
                        } else {
                            state.slash_overlay = None;
                        }
                    }
                    KeyCode::PageUp => {
                        let cur = state.list_state.offset();
                        *state.list_state.offset_mut() = cur.saturating_sub(10);
                        state.follow_bottom = false;
                    }
                    KeyCode::PageDown => {
                        let max_off = state
                            .last_wrapped_len
                            .saturating_sub(state.last_view_height.max(1));
                        let cur = state.list_state.offset();
                        *state.list_state.offset_mut() = (cur.saturating_add(10)).min(max_off);
                        state.follow_bottom = state.list_state.offset() >= max_off;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    Ok(())
}
