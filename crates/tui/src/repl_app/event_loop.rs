#![allow(clippy::redundant_closure_for_method_calls)]

use super::oauth_spawn::{
    spawn_anthropic_oauth_thread, spawn_extension_connection_watch, spawn_openai_oauth_thread,
};
use super::slash_commands::{handle_session_modal_outcome, handle_slash_command_tui};
use super::*;

#[allow(dead_code)]
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

    fn handle_mouse_event(&mut self, me: MouseEvent) -> bool {
        if matches!(
            me.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) && self.active_modal.is_some()
        {
            if let Some(modal) = self.active_modal.as_mut() {
                if modal.supports_vertical_wheel() {
                    modal.handle_vertical_wheel(matches!(me.kind, MouseEventKind::ScrollDown));
                }
            }
            return true;
        }

        if let Some(overlay_rect) = self.last_slash_overlay_rect {
            if rect_contains_mouse(overlay_rect, me.column, me.row) {
                match me.kind {
                    MouseEventKind::ScrollUp => {
                        self.slash_overlay_select_prev();
                        return true;
                    }
                    MouseEventKind::ScrollDown => {
                        self.slash_overlay_select_next();
                        return true;
                    }
                    _ => {}
                }
            }
        }

        let in_transcript = rect_contains_mouse(self.last_transcript_rect, me.column, me.row);
        let in_input = rect_contains_mouse(self.last_input_rect, me.column, me.row);

        if matches!(self.view_mode, ViewMode::Child(_)) {
            self.handle_child_view_mouse_event(me);
            return true;
        }

        if in_transcript {
            self.handle_parent_transcript_mouse_event(me);
        } else if in_input || self.ui_state == AppUiState::WelcomeMode {
            self.handle_input_mouse_event(me);
        }

        false
    }

    fn handle_child_view_mouse_event(&mut self, me: MouseEvent) {
        let ViewMode::Child(id) = self.view_mode.clone() else {
            return;
        };

        match me.kind {
            MouseEventKind::ScrollUp => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&id) {
                    tab.follow_bottom = false;
                    *tab.list_state.offset_mut() = tab.list_state.offset().saturating_sub(6);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&id) {
                    let max = tab
                        .last_wrapped_len
                        .saturating_sub(tab.last_view_height.max(1));
                    *tab.list_state.offset_mut() =
                        (tab.list_state.offset().saturating_add(6)).min(max);
                    tab.follow_bottom = tab.list_state.offset() >= max;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let tr = self.last_transcript_rect;
                if rect_contains_mouse(tr, me.column, me.row) {
                    let cur = self
                        .child_tab_panel
                        .find_tab_mut(&id)
                        .map_or(0, |tab| tab.list_state.offset());
                    let col = me.column.saturating_sub(tr.x);
                    let row = cur + usize::from(me.row.saturating_sub(tr.y));
                    self.selection.anchor = Some((col, row));
                    self.selection.end = Some((col, row));
                    self.selection.mouse_drag_occurred = false;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let tr = self.last_transcript_rect;
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&id) {
                    let cur = tab.list_state.offset();
                    let col = me
                        .column
                        .saturating_sub(tr.x)
                        .min(tr.width.saturating_sub(1));
                    let row = cur
                        + usize::from(me.row.saturating_sub(tr.y).min(tr.height.saturating_sub(1)));
                    self.selection.end = Some((col, row));
                    self.selection.mouse_drag_occurred = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.selection.mouse_drag_occurred {
                    let tr = self.last_transcript_rect;
                    if let Some(tab) = self.child_tab_panel.find_tab_mut(&id) {
                        let cur = tab.list_state.offset();
                        let col = me
                            .column
                            .saturating_sub(tr.x)
                            .min(tr.width.saturating_sub(1));
                        let row = cur
                            + usize::from(
                                me.row.saturating_sub(tr.y).min(tr.height.saturating_sub(1)),
                            );
                        self.selection.end = Some((col, row));
                    }
                } else {
                    self.selection.anchor = None;
                    self.selection.end = None;
                }
            }
            MouseEventKind::Down(MouseButton::Right) if self.selection.anchor.is_some() => {
                self.selection.pending_copy = Some(true);
                self.selection.suppress_paste_until =
                    Some(Instant::now() + Duration::from_millis(800));
            }
            _ => {}
        }
    }

    fn handle_parent_transcript_mouse_event(&mut self, me: MouseEvent) {
        let max_off = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        let cur = self.list_state.offset();
        let tr = self.last_transcript_rect;

        match me.kind {
            MouseEventKind::ScrollUp => {
                *self.list_state.offset_mut() = cur.saturating_sub(6);
                self.follow_bottom = false;
            }
            MouseEventKind::ScrollDown => {
                *self.list_state.offset_mut() = (cur.saturating_add(6)).min(max_off);
                self.follow_bottom = self.list_state.offset() >= max_off;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let col = me.column.saturating_sub(tr.x);
                let row = cur + usize::from(me.row.saturating_sub(tr.y));
                self.selection.anchor = Some((col, row));
                self.selection.end = Some((col, row));
                self.selection.mouse_drag_occurred = false;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let col = me
                    .column
                    .saturating_sub(tr.x)
                    .min(tr.width.saturating_sub(1));
                let row =
                    cur + usize::from(me.row.saturating_sub(tr.y).min(tr.height.saturating_sub(1)));
                self.selection.end = Some((col, row));
                self.selection.mouse_drag_occurred = true;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.selection.mouse_drag_occurred {
                    let col = me
                        .column
                        .saturating_sub(tr.x)
                        .min(tr.width.saturating_sub(1));
                    let row = cur
                        + usize::from(me.row.saturating_sub(tr.y).min(tr.height.saturating_sub(1)));
                    self.selection.end = Some((col, row));
                } else {
                    self.selection.anchor = None;
                    self.selection.end = None;
                }
            }
            MouseEventKind::Down(MouseButton::Right) if self.selection.anchor.is_some() => {
                self.selection.pending_copy = Some(true);
                self.selection.suppress_paste_until =
                    Some(Instant::now() + Duration::from_millis(800));
            }
            _ => {}
        }
    }

    fn handle_input_mouse_event(&mut self, me: MouseEvent) {
        let ir = self.last_input_rect;
        let widget_row = usize::from(me.row.saturating_sub(ir.y));
        let widget_col = usize::from(me.column.saturating_sub(ir.x));
        let char_idx = self.char_index_at_mouse(widget_row, widget_col);

        match me.kind {
            MouseEventKind::ScrollUp => {
                self.input_scroll_offset = self.input_scroll_offset.saturating_sub(1);
                self.input_scroll_manual = true;
            }
            MouseEventKind::ScrollDown => {
                self.input_scroll_offset = self.input_scroll_offset.saturating_add(1);
                self.input_scroll_manual = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = char_idx {
                    self.set_input_cursor_line_col_by_char(idx);
                    self.input_selection = Some((idx, idx));
                    self.input_click_anchor = Some(idx);
                    self.selection.anchor = None;
                    self.selection.end = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(idx) = char_idx {
                    if let Some(anchor) = self.input_click_anchor {
                        let a = anchor.min(idx);
                        let b = anchor.max(idx);
                        self.input_selection = Some((a, b));
                    }
                    self.selection.anchor = None;
                    self.selection.end = None;
                }
            }
            MouseEventKind::Down(MouseButton::Right) if self.input_selection.is_some() => {
                if let Some(text) = self.selected_input_text_expanded() {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(text);
                    }
                }
                self.input_selection = None;
                self.input_click_anchor = None;
            }
            _ => {}
        }
    }

    fn process_paste_event(&mut self, text: &str) -> bool {
        let suppress = self
            .selection
            .suppress_paste_until
            .is_some_and(|deadline| Instant::now() <= deadline);
        self.selection.suppress_paste_until = None;

        if suppress || self.active_modal.is_some() {
            return true;
        }

        self.flush_paste_burst();
        self.last_key_time = None;
        self.handle_paste_event(text);
        if text.contains('\n') {
            self.arm_paste_enter_suppression();
        }
        self.wake_input_caret();
        self.refresh_slash_overlay();
        true
    }

    fn handle_key_event(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        ui_tx: &Sender<ReplTuiEvent>,
        work_tx: &Sender<WorkerMsg>,
        cli: &Arc<Mutex<LiveCli>>,
        cancel_flag: &Arc<ControlState>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if self.paste_enter_is_suppressed(key.code) {
            self.selection.suppress_paste_until = Some(Instant::now() + Duration::from_millis(150));
            return Ok(true);
        }

        if !matches!(key.code, KeyCode::Char(_) | KeyCode::Enter) || !key.modifiers.is_empty() {
            self.flush_paste_burst();
        }
        if self
            .selection
            .suppress_paste_until
            .is_some_and(|deadline| Instant::now() > deadline)
        {
            self.selection.suppress_paste_until = None;
        }

        if !matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            self.selection.anchor = None;
            self.selection.end = None;
        }

        if self.handle_input_shortcuts(key) {
            return Ok(true);
        }
        if self.handle_child_view_key_event(key) {
            return Ok(true);
        }
        if self.handle_modal_key_event(key, cli, ui_tx) {
            return Ok(true);
        }

        self.handle_parent_key_event(key, terminal, ui_tx, work_tx, cli, cancel_flag)
    }

    fn handle_input_shortcuts(&mut self, key: KeyEvent) -> bool {
        if self.active_modal.is_none()
            && ((key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL))
                || (key.code == KeyCode::Insert && key.modifiers.contains(KeyModifiers::SHIFT)))
        {
            self.flush_paste_burst();
            self.last_key_time = None;
            if let Some(text) = read_clipboard_text() {
                self.handle_paste_event(&text);
                if text.contains('\n') {
                    self.arm_paste_enter_suppression();
                }
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            return true;
        }

        if self.active_modal.is_none()
            && key.code == KeyCode::Char('a')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.select_all_input();
            self.wake_input_caret();
            return true;
        }

        if self.active_modal.is_none()
            && key.code == KeyCode::Char('z')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            if self.undo_input_edit() {
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            return true;
        }

        if self.active_modal.is_none()
            && key.code == KeyCode::Char('y')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            if self.redo_input_edit() {
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            return true;
        }

        if self.active_modal.is_none()
            && key.code == KeyCode::Char('w')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.word_backspace();
            self.wake_input_caret();
            self.refresh_slash_overlay();
            return true;
        }

        if self.active_modal.is_none()
            && self.input_selection.is_some()
            && ((key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
                || (key.code == KeyCode::Insert && key.modifiers == KeyModifiers::CONTROL))
        {
            if let Some(text) = self.selected_input_text_expanded() {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            }
            self.input_selection = None;
            self.selection.anchor = None;
            self.selection.end = None;
            return true;
        }

        if self.active_modal.is_none()
            && self.input_selection.is_some()
            && key.code == KeyCode::Char('x')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            if let Some(text) = self.cut_input_selection_text() {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            }
            self.wake_input_caret();
            self.refresh_slash_overlay();
            return true;
        }

        false
    }

    fn handle_child_view_key_event(&mut self, key: KeyEvent) -> bool {
        let ViewMode::Child(child_id) = self.view_mode.clone() else {
            return false;
        };

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Up => {
                self.selection.anchor = None;
                self.selection.end = None;
                self.view_mode = ViewMode::Parent;
            }
            KeyCode::Left => {
                self.child_tab_panel.prev_tab();
                if let Some(tab) = self.child_tab_panel.active_tab_state_mut() {
                    self.view_mode = ViewMode::Child(tab.child_id.clone());
                } else {
                    self.view_mode = ViewMode::Parent;
                }
            }
            KeyCode::Right => {
                self.child_tab_panel.next_tab();
                if let Some(tab) = self.child_tab_panel.active_tab_state_mut() {
                    self.view_mode = ViewMode::Child(tab.child_id.clone());
                } else {
                    self.view_mode = ViewMode::Parent;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&child_id) {
                    let max = tab
                        .last_wrapped_len
                        .saturating_sub(tab.last_view_height.max(1));
                    *tab.list_state.offset_mut() =
                        (tab.list_state.offset().saturating_add(1)).min(max);
                    tab.follow_bottom = false;
                }
            }
            KeyCode::Char('k') => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&child_id) {
                    tab.follow_bottom = false;
                    *tab.list_state.offset_mut() = tab.list_state.offset().saturating_sub(1);
                }
            }
            KeyCode::PageDown => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&child_id) {
                    let max = tab
                        .last_wrapped_len
                        .saturating_sub(tab.last_view_height.max(1));
                    *tab.list_state.offset_mut() =
                        (tab.list_state.offset().saturating_add(10)).min(max);
                    tab.follow_bottom = false;
                }
            }
            KeyCode::PageUp => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&child_id) {
                    tab.follow_bottom = false;
                    *tab.list_state.offset_mut() = tab.list_state.offset().saturating_sub(10);
                }
            }
            KeyCode::Char('G') => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&child_id) {
                    tab.follow_bottom = true;
                }
            }
            KeyCode::Char('g') => {
                if let Some(tab) = self.child_tab_panel.find_tab_mut(&child_id) {
                    tab.follow_bottom = false;
                    *tab.list_state.offset_mut() = 0;
                }
            }
            KeyCode::Enter => {
                let should_resume =
                    self.child_tab_panel
                        .find_tab_mut(&child_id)
                        .is_some_and(|tab| {
                            matches!(tab.status, child_tabs::ChildTabStatus::Paused { .. })
                        });
                if should_resume {
                    if let Some(registry) = &self.child_control_registry {
                        if let Some(child_state) = registry.get(&child_id) {
                            child_state.resume();
                            self.push_system(&format!("Resuming child {child_id}..."));
                        }
                    }
                }
            }
            _ => return true,
        }

        true
    }

    fn handle_modal_key_event(
        &mut self,
        key: KeyEvent,
        cli: &Arc<Mutex<LiveCli>>,
        ui_tx: &Sender<ReplTuiEvent>,
    ) -> bool {
        let mut modal_action = None;
        let mut modal_succeeded = false;
        let mut oauth_provider = None;
        let mut model_outcome = None;
        let mut session_outcome = None;

        if let Some(ref mut modal) = self.active_modal {
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
            handle_session_modal_outcome(self, cli, outcome);
            return true;
        }

        let Some(action) = modal_action else {
            return false;
        };

        if let Some(prov) = oauth_provider {
            match prov {
                crate::tui::auth_modal::ProviderKind::Anthropic => {
                    spawn_anthropic_oauth_thread(ui_tx.clone(), &mut self.active_modal);
                }
                crate::tui::auth_modal::ProviderKind::OpenAi => {
                    spawn_openai_oauth_thread(ui_tx.clone(), &mut self.active_modal);
                }
                crate::tui::auth_modal::ProviderKind::Other
                | crate::tui::auth_modal::ProviderKind::Preset(_) => {}
            }
        }

        match action {
            ModalAction::Consumed => true,
            ModalAction::Unhandled => false,
            ModalAction::Dismiss => {
                self.active_modal = None;
                if modal_succeeded {
                    self.handle_successful_auth_modal_dismiss(cli);
                } else if let Some(outcome) = model_outcome {
                    self.handle_model_modal_outcome(cli, ui_tx, outcome);
                }
                true
            }
        }
    }

    fn handle_successful_auth_modal_dismiss(&mut self, cli: &Arc<Mutex<LiveCli>>) {
        match cli.lock() {
            Ok(mut guard) => {
                if let Some(preferred_model) = crate::app::initial_model_from_credentials() {
                    if preferred_model != guard.model_name() {
                        let _ = guard.model_command(Some(preferred_model));
                    }
                }
                if let Err(error) = guard.refresh_runtime_auth() {
                    self.push_system(&format!(
                        "Auth setup saved, but runtime refresh failed: {error}"
                    ));
                } else {
                    self.ui_state = AppUiState::WelcomeMode;
                    self.messages.clear();
                    self.live_tool_calls.clear();
                }
            }
            Err(_) => {
                self.push_system("Authentication configured, but runtime lock failed.");
            }
        }

        if let Some(pending_model) = self.pending_model_after_auth.take() {
            match cli.lock() {
                Ok(mut guard) => match guard.model_command(Some(pending_model.clone())) {
                    Ok(result) => {
                        if result.persist_after {
                            let _ = guard.persist_session();
                        }
                        self.push_system_card("Model", &result.message);
                    }
                    Err(e) => {
                        self.push_system_card(
                            "Model Error",
                            &format!("Failed to switch model: {e}"),
                        );
                    }
                },
                Err(_) => {
                    self.push_system_card(
                        "Model Error",
                        "Failed to acquire CLI lock for model switch",
                    );
                }
            }
        }
    }

    fn handle_model_modal_outcome(
        &mut self,
        cli: &Arc<Mutex<LiveCli>>,
        ui_tx: &Sender<ReplTuiEvent>,
        outcome: crate::tui::model_modal::ModelModalOutcome,
    ) {
        match outcome {
            crate::tui::model_modal::ModelModalOutcome::SwitchModel { model_id } => {
                match cli.lock() {
                    Ok(mut guard) => match guard.model_command(Some(model_id.clone())) {
                        Ok(result) => {
                            if result.persist_after {
                                let _ = guard.persist_session();
                            }
                            drop(guard);
                            self.push_system_card("Model", &result.message);
                        }
                        Err(e) => {
                            drop(guard);
                            self.push_system_card("Model Error", &format!("{e}"));
                        }
                    },
                    Err(_) => {
                        self.push_system_card("Model Error", "Failed to acquire CLI lock.");
                    }
                }
            }
            crate::tui::model_modal::ModelModalOutcome::AuthRequired {
                provider_id,
                model_id,
            } => {
                self.pending_model_after_auth = Some(model_id.clone());
                let parsed_provider = crate::app::parse_provider_arg(&provider_id).ok();
                self.active_modal = Some(ActiveModal::Auth(AuthModal::new(
                    ui_tx.clone(),
                    parsed_provider,
                )));
            }
            crate::tui::model_modal::ModelModalOutcome::None => {}
        }
    }

    fn handle_parent_key_event(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        ui_tx: &Sender<ReplTuiEvent>,
        work_tx: &Sender<WorkerMsg>,
        cli: &Arc<Mutex<LiveCli>>,
        cancel_flag: &Arc<ControlState>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if self.active_modal.is_none()
            && key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            if self.busy {
                cancel_flag.request_cancel();
                if let Some(registry) = &self.child_control_registry {
                    registry.cancel_all();
                }
                self.cancelling = true;
                self.push_system("Interrupting...");
                return Ok(true);
            }
            self.exit = true;
            self.persist_on_exit = true;
            return Ok(true);
        }

        if key.code == KeyCode::Char('p')
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && self.busy
            && !self.paused
        {
            cancel_flag.request_pause();
            self.push_system("Pausing after current operation...");
            return Ok(true);
        }

        if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.busy {
                return Ok(true);
            }
            if self.input.text.is_empty() {
                self.exit = true;
                self.persist_on_exit = true;
            }
            return Ok(true);
        }

        if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let mut g = cli.lock().expect("cli lock");
            let new_effort = g.cycle_reasoning_effort();
            self.cached_header = build_header_snapshot(&g);
            if let Some(effort) = new_effort {
                self.push_system(&format!("Reasoning effort: {effort}"));
            } else {
                self.push_system("Reasoning effort: off");
            }
            return Ok(true);
        }

        if key.code == KeyCode::Up && self.slash_overlay.is_some() {
            self.slash_overlay_select_prev();
            return Ok(true);
        }

        if key.code == KeyCode::Down && self.slash_overlay.is_some() {
            self.slash_overlay_select_next();
            return Ok(true);
        }

        if key.code == KeyCode::Char('x')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(self.view_mode, ViewMode::Parent)
        {
            if let Some(child_id) = self
                .child_tab_panel
                .tabs
                .get(self.child_tab_panel.active_tab)
                .map(|tab| tab.child_id.clone())
            {
                self.selection.anchor = None;
                self.selection.end = None;
                self.view_mode = ViewMode::Child(child_id);
                return Ok(true);
            }
        }

        self.handle_parent_key_code(key, terminal, ui_tx, work_tx, cli, cancel_flag)
    }

    fn handle_parent_key_code(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        ui_tx: &Sender<ReplTuiEvent>,
        work_tx: &Sender<WorkerMsg>,
        cli: &Arc<Mutex<LiveCli>>,
        cancel_flag: &Arc<ControlState>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        match key.code {
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.last_key_time = Some(Instant::now());
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            KeyCode::Enter => {
                self.handle_enter_key(terminal, ui_tx, work_tx, cli, cancel_flag)?;
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_input_char('\n');
                self.last_key_time = Some(Instant::now());
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            KeyCode::Char(c) => {
                let now = Instant::now();
                if self.in_paste_burst(now) || !self.paste_burst_chars.is_empty() {
                    self.paste_burst_chars.push(c);
                } else {
                    self.insert_input_char(c);
                }
                self.last_key_time = Some(now);
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            KeyCode::Backspace => {
                self.backspace_input_char();
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            KeyCode::Delete => {
                self.delete_input_char();
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
            KeyCode::Left => {
                self.move_input_cursor_left();
                self.wake_input_caret();
            }
            KeyCode::Right => {
                self.move_input_cursor_right();
                self.wake_input_caret();
            }
            KeyCode::Home => {
                self.move_input_cursor_home();
                self.wake_input_caret();
            }
            KeyCode::End => {
                self.move_input_cursor_end();
                self.wake_input_caret();
            }
            KeyCode::Up => {
                self.move_input_cursor_up();
                self.wake_input_caret();
            }
            KeyCode::Down => {
                self.move_input_cursor_down();
                self.wake_input_caret();
            }
            KeyCode::Tab => self.handle_tab_key(),
            KeyCode::Esc => self.handle_escape_key(cancel_flag),
            KeyCode::PageUp => {
                let cur = self.list_state.offset();
                *self.list_state.offset_mut() = cur.saturating_sub(10);
                self.follow_bottom = false;
            }
            KeyCode::PageDown => {
                let max_off = self
                    .last_wrapped_len
                    .saturating_sub(self.last_view_height.max(1));
                let cur = self.list_state.offset();
                *self.list_state.offset_mut() = (cur.saturating_add(10)).min(max_off);
                self.follow_bottom = self.list_state.offset() >= max_off;
            }
            _ => {}
        }

        Ok(true)
    }

    fn handle_enter_key(
        &mut self,
        terminal: &mut DefaultTerminal,
        ui_tx: &Sender<ReplTuiEvent>,
        work_tx: &Sender<WorkerMsg>,
        cli: &Arc<Mutex<LiveCli>>,
        cancel_flag: &Arc<ControlState>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        if self.in_paste_burst(now) {
            self.paste_burst_chars.push('\n');
            self.last_key_time = Some(now);
            self.wake_input_caret();
            return Ok(());
        }
        self.flush_paste_burst();
        self.last_key_time = Some(now);

        if self.child_tab_panel.active_tab_is_paused() {
            if let Some(child_id) = self.child_tab_panel.active_child_id() {
                if let Some(registry) = &self.child_control_registry {
                    if let Some(child_state) = registry.get(child_id) {
                        child_state.resume();
                        self.push_system(&format!("Resuming child {child_id}..."));
                    }
                }
            }
            return Ok(());
        }
        if self.paused {
            cancel_flag.resume();
            self.paused = false;
            self.pause_reason = String::new();
            self.push_system("Resuming...");
            return Ok(());
        }
        let trimmed_peek = self.input.text.trim().to_ascii_lowercase();
        if trimmed_peek == "/exit" || trimmed_peek == "/quit" {
            if self.busy {
                cancel_flag.request_cancel();
            }
            self.exit = true;
            self.persist_on_exit = true;
            self.reset_input();
            return Ok(());
        }
        if self.busy {
            self.push_system("Please wait for the current task to finish before submitting.");
            return Ok(());
        }
        if self.slash_overlay.is_some() {
            let trimmed = self.input.text.trim().to_string();
            if let Some(selected) = self.selected_slash_command() {
                if selected != trimmed {
                    self.record_input_undo_snapshot();
                    self.input.text = selected;
                    self.input.text.push(' ');
                    self.input.cursor = self.input.text.chars().count();
                    self.resync_byte_cursor();
                    self.input.preferred_col = None;
                    self.input_scroll_offset = usize::MAX;
                    self.wake_input_caret();
                    self.refresh_slash_overlay();
                    return Ok(());
                }
            }
        }

        let raw_line = std::mem::take(&mut self.input.text);
        let line = expand_masks(&raw_line, &self.input.pastes);
        self.reset_input();
        self.input_scroll_offset = 0;
        self.clear_input_history();
        let trimmed = line.trim().to_string();
        self.refresh_slash_overlay();
        if trimmed.is_empty() {
            self.wake_input_caret();
            return Ok(());
        }
        if let Some(cmd) = SlashCommand::parse(&trimmed) {
            handle_slash_command_tui(terminal, self, cli, ui_tx, cmd)?;
            self.wake_input_caret();
            return Ok(());
        }
        self.push_user_line(&trimmed);
        work_tx.send(WorkerMsg::RunTurn(trimmed))?;
        self.wake_input_caret();
        Ok(())
    }

    fn handle_tab_key(&mut self) {
        if self.busy || !self.input.text.trim_start().starts_with('/') {
            return;
        }
        if let Some(selected) = self.selected_slash_command() {
            self.record_input_undo_snapshot();
            self.input.text = selected;
            self.input.text.push(' ');
            self.input.cursor = self.input.text.chars().count();
            self.input.preferred_col = None;
            self.input_scroll_offset = usize::MAX;
            self.wake_input_caret();
            self.refresh_slash_overlay();
        } else {
            let prefix = self.input.text.to_ascii_lowercase();
            let candidates = slash_command_completion_candidates();
            let matches: Vec<_> = candidates
                .into_iter()
                .filter(|candidate| candidate.starts_with(&prefix))
                .collect();
            if matches.len() == 1 {
                self.record_input_undo_snapshot();
                self.input.text.clone_from(&matches[0]);
                self.input.text.push(' ');
                self.input.cursor = self.input.text.chars().count();
                self.input.preferred_col = None;
                self.input_scroll_offset = usize::MAX;
                self.wake_input_caret();
                self.refresh_slash_overlay();
            }
        }
    }

    fn handle_escape_key(&mut self, cancel_flag: &Arc<ControlState>) {
        if self.busy {
            let now = Instant::now();
            if self
                .last_esc_at
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(500))
            {
                cancel_flag.request_cancel();
                if let Some(registry) = &self.child_control_registry {
                    registry.cancel_all();
                }
                self.cancelling = true;
                self.push_system("Interrupting...");
                self.last_esc_at = None;
            } else {
                self.last_esc_at = Some(now);
            }
        } else {
            self.slash_overlay = None;
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
                    let mut g = cli_worker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let _ = g.run_turn_tui(&line);
                }
                WorkerMsg::Shutdown => break,
            }
        }
    });

    let cancel_flag = cli
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .cancel_flag();

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

    {
        let mut g = cli
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.child_event_rx = g.take_child_event_rx();
        state.child_control_registry = g.take_child_control_registry();
    }

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
                let mut g = cli
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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

        let poll_ms = if state.typewriter.chars.is_empty() {
            50
        } else {
            16
        };
        if !event::poll(Duration::from_millis(poll_ms))? {
            continue;
        }

        match event::read()? {
            Event::Mouse(me) => {
                state.handle_mouse_event(me);
            }
            Event::Paste(text) => {
                state.process_paste_event(&text);
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                state.handle_key_event(key, terminal, ui_tx, work_tx, cli, cancel_flag)?;
            }
            _ => {}
        }
    }

    Ok(())
}
