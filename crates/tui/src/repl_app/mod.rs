use std::cmp::min;
use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::{slash_command_completion_candidates, AllowedToolSet, LiveCli};
use crate::auth::ProviderChoice;
use crate::display_width::{char_count_for_display_col, char_display_width, text_display_width};
use crate::format::render_repl_help;
use crate::tool_format::tool_input_summary;
use crate::tui::active_modal::ActiveModal;
use crate::tui::auth_modal::{AuthModal, AuthModalStep};
use crate::tui::child_tabs;
use crate::tui::modal::{Modal, ModalAction};
use crate::tui::repl_render::{
    build_header_snapshot, draw_chat, draw_welcome, parse_report_rows, rect_contains_mouse,
    suspend_for_stdout,
};
use crate::tui::session_modal::SessionModalEntry;
use crate::tui::ReplTuiEvent;
use acrawl_core::message::{ContentBlock, ConversationMessage, MessageRole};
use agent::{ChildControlRegistry, ChildEvent};
use browser::{BrowserState, SharedBridge};
use commands::{slash_command_specs, SlashCommand};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;
use ratatui::DefaultTerminal;
use runtime::ControlState;

#[allow(clippy::wildcard_imports)]
mod event_loop;
#[allow(clippy::wildcard_imports)]
mod input_editor;
#[allow(clippy::wildcard_imports)]
mod layout;
#[allow(clippy::wildcard_imports)]
mod oauth_spawn;
#[allow(clippy::wildcard_imports)]
mod slash_commands;
#[cfg(test)]
mod tests;
#[allow(clippy::wildcard_imports)]
mod types;

pub(crate) use self::event_loop::run_repl_ratatui;
pub(super) use self::types::*;
pub(crate) use self::types::{HeaderSnapshot, ToolCallStatus};

const MAX_INPUT_LINES: usize = 5;
/// Cap on events processed in a single `drain_events` call so a producer that
/// emits faster than the typewriter reveals can't starve the render loop.
const MAX_EVENTS_PER_FRAME: usize = 256;
/// Cap on the typewriter backlog. If the producer overruns this, flush the
/// queue straight to the transcript (skipping the slow per-char reveal) so the
/// `VecDeque` can't grow unbounded.
const MAX_TYPEWRITER_BACKLOG: usize = 64 * 1024;
pub(super) const WELCOME_BOX_SIDE_GUTTER: u16 = 16;
pub(super) const WELCOME_BOX_MAX_WIDTH: u16 = 82;
pub(super) const WELCOME_BOX_MIN_WIDTH: u16 = 30;
const INPUT_CARET_MARKER: char = '\u{E000}';
pub(super) const SLASH_OVERLAY_VISIBLE_ITEMS: usize = 7;
pub(super) const SLASH_OVERLAY_HINT_TEXT: &str =
    "Up/Down move  Enter accept  Tab complete  Esc close";

fn normalize_pasted_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn read_clipboard_text() -> Option<String> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let text = clipboard.get_text().ok()?;
    Some(normalize_pasted_text(&text))
}

/// Mask any paste at or above this byte length.
const PASTE_MASK_THRESHOLD_BYTES: usize = 150;

/// Count logical lines in `text` by physical newline count plus one.  O(n) byte
/// scan, no UTF-8 decoding - `\n` is ASCII so this is safe on any UTF-8 string.
fn count_lines(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// Whether a paste should be replaced by a placeholder mask.
/// Inclusive at the threshold: returns true when `text.len() >= PASTE_MASK_THRESHOLD_BYTES`.
fn should_mask_paste(text: &str) -> bool {
    text.len() >= PASTE_MASK_THRESHOLD_BYTES
}

/// Format the visible placeholder for a masked paste.
fn format_paste_placeholder(id: u32, line_count: usize) -> String {
    format!("[#{id} Pasted ~{line_count} lines]")
}

/// Replace each paste mask's placeholder with its original content.  Used at
/// submit time so the model receives the full pasted text, and at clipboard
/// yank time so copying the input bar produces the original content.
///
/// Known limitation: if the user manually types text that exactly matches a
/// live placeholder (e.g. `[#1 Pasted ~5 lines]` character-for-character), that
/// typed text will also be replaced with the paste content.  This is rare in
/// practice - the per-prompt `#N` index and the specific bracket+tilde format
/// make accidental collisions unlikely.
fn expand_masks(text: &str, pastes: &[PasteEntry]) -> String {
    let mut out = text.to_string();
    for p in pastes {
        out = out.replace(&p.placeholder, &p.content);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AppUiState {
    WelcomeMode,
    ChatMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ViewMode {
    Parent,
    Child(String),
}
#[allow(clippy::struct_excessive_bools)]
pub(super) struct ReplTuiState {
    ui_state: AppUiState,
    pub(super) messages: Vec<ConversationMessage>,
    pub(super) live_tool_calls: Vec<(String, String, ToolCallStatus)>,
    pub(super) list_state: ListState,
    pub(super) follow_bottom: bool,
    pub(super) last_transcript_rect: Rect,
    pub(super) last_wrapped_len: usize,
    pub(super) last_view_height: usize,
    pub(super) last_input_rect: Rect,
    pub(super) input_scroll_offset: usize,
    /// Set to `true` when the user manually scrolls the input field with the
    /// mouse wheel; suppresses the caret-visibility snap in
    /// `calculate_input_dimensions` so the scroll position is preserved until
    /// the cursor moves (typing, arrow keys, paste, etc.).
    input_scroll_manual: bool,
    /// Active text selection within the input field: `(start_char, end_char)`
    /// where start <= end.  `None` when no selection is active.
    pub(super) input_selection: Option<(usize, usize)>,
    /// Immutable anchor set on mouse Down(Left); used by Drag to extend the
    /// selection without ever being modified by other mouse events or key
    /// handlers.  `None` between drags.
    input_click_anchor: Option<usize>,
    /// Cached width (columns) of the input widget from the last render pass.
    /// Used by cursor up/down to compute soft-wrap boundaries.
    pub(super) input_area_width: u16,
    input: InputEditorState,
    status_line: String,
    pub(super) busy: bool,
    pub(super) cancelling: bool,
    pending_model_after_auth: Option<String>,
    active_modal: Option<ActiveModal>,
    /// Picker catalog state for the `/model` modal.
    live_model_catalog: ModelCatalogState,
    exit: bool,
    pub(super) current_tool: Option<String>,
    persist_on_exit: bool,
    cursor_on: bool,
    cursor_blink_deadline: Instant,
    pub(super) slash_overlay: Option<SlashOverlay>,
    pub(super) last_slash_overlay_rect: Option<Rect>,
    pub(super) cached_header: HeaderSnapshot,
    spinner_tick: u8,
    spinner_deadline: Instant,
    pub(super) typewriter: TypewriterState,
    pub(super) selection: SelectionState,
    /// Accumulator for chars/newlines arriving faster than a human can type
    /// (<= 30 ms apart). Flushed via `handle_paste_event` so masking and the
    /// paste-newline suppression window apply uniformly even on terminals that
    /// deliver pastes as raw keystrokes instead of `Event::Paste`.
    paste_burst_chars: Vec<char>,
    /// Timestamp of the most-recent `KeyCode::Char` or `KeyCode::Enter` event.
    /// Used to decide whether the next keystroke is part of the same burst.
    last_key_time: Option<Instant>,
    input_undo_stack: Vec<InputUndoSnapshot>,
    input_redo_stack: Vec<InputUndoSnapshot>,
    last_esc_at: Option<Instant>,
    pub(super) debug_mode: bool,
    pub(super) update_info: Option<runtime::update_check::UpdateInfo>,
    pub(super) update_rx:
        Option<tokio::sync::oneshot::Receiver<Option<runtime::update_check::UpdateInfo>>>,
    pub(super) child_tab_panel: child_tabs::ChildTabPanel,
    child_event_rx: Option<std::sync::mpsc::Receiver<ChildEvent>>,
    pub(super) child_control_registry: Option<ChildControlRegistry>,
    pub(super) view_mode: ViewMode,
}

impl ReplTuiState {
    fn new() -> Self {
        Self {
            ui_state: AppUiState::WelcomeMode,
            messages: Vec::new(),
            live_tool_calls: Vec::new(),
            list_state: ListState::default(),
            follow_bottom: true,
            last_transcript_rect: Rect::default(),
            last_wrapped_len: 0,
            last_view_height: 0,
            last_input_rect: Rect::default(),
            input_scroll_offset: 0,
            input_scroll_manual: false,
            input_selection: None,
            input_click_anchor: None,
            input_area_width: 0,
            input: InputEditorState {
                text: String::new(),
                cursor: 0,
                byte_cursor: 0,
                preferred_col: None,
                pastes: Vec::new(),
                next_paste_id: 1,
            },
            status_line: String::new(),
            busy: false,
            cancelling: false,
            pending_model_after_auth: None,
            active_modal: None,
            live_model_catalog: ModelCatalogState::Loading,
            exit: false,
            persist_on_exit: false,
            current_tool: None,
            cursor_on: true,
            cursor_blink_deadline: Instant::now() + Duration::from_millis(530),
            slash_overlay: None,
            last_slash_overlay_rect: None,
            cached_header: HeaderSnapshot::default(),
            spinner_tick: 0,
            spinner_deadline: Instant::now() + Duration::from_millis(120),
            typewriter: TypewriterState {
                chars: VecDeque::new(),
                live: String::new(),
            },
            paste_burst_chars: Vec::new(),
            last_key_time: None,
            selection: SelectionState::default(),
            input_undo_stack: Vec::new(),
            input_redo_stack: Vec::new(),
            last_esc_at: None,
            debug_mode: false,
            update_info: None,
            update_rx: None,
            child_tab_panel: child_tabs::ChildTabPanel::default(),
            child_event_rx: None,
            child_control_registry: None,
            view_mode: ViewMode::Parent,
        }
    }
}
