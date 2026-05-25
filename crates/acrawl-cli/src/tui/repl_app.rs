//! Ratatui REPL with a welcome screen, sticky-bottom chat transcript, slash overlay, and floating input.

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
use crate::markdown::{drain_safe_boundary, render_lines};
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
use commands::{slash_command_specs, SlashCommand};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;
use ratatui::DefaultTerminal;
use runtime::ControlState;

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
const PASTE_MASK_THRESHOLD_BYTES: usize = 2048;
const PASTE_MASK_THRESHOLD_LINES: usize = 30;
const PASTE_SENTINEL_BASE: u32 = 0xF0001;
pub(super) const SLASH_OVERLAY_VISIBLE_ITEMS: usize = 7;
pub(super) const SLASH_OVERLAY_HINT_TEXT: &str =
    "Up/Down move  Enter accept  Tab complete  Esc close";

fn normalize_pasted_text(text: &str) -> std::borrow::Cow<'_, str> {
    if text.as_bytes().contains(&b'\r') {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

fn is_paste_sentinel(c: char) -> bool {
    let cp = c as u32;
    (PASTE_SENTINEL_BASE..PASTE_SENTINEL_BASE + 65534).contains(&cp)
}

fn sentinel_to_id(c: char) -> u32 {
    (c as u32) - PASTE_SENTINEL_BASE + 1
}

fn id_to_sentinel(id: u32) -> char {
    char::from_u32(PASTE_SENTINEL_BASE + id - 1).unwrap()
}

fn should_mask_paste(text: &str) -> bool {
    text.len() >= PASTE_MASK_THRESHOLD_BYTES
        || text.bytes().filter(|&b| b == b'\n').count() >= PASTE_MASK_THRESHOLD_LINES
}

fn format_paste_pill(id: u32, content: &str) -> String {
    let line_count = content.bytes().filter(|&b| b == b'\n').count() + 1;
    format!("[📋 #{id} ~{line_count} lines]")
}

fn read_clipboard_text() -> Option<String> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let text = clipboard.get_text().ok()?;
    Some(normalize_pasted_text(&text).into_owned())
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

#[derive(Clone, Debug)]
pub(super) enum ToolCallStatus {
    Running,
    Interrupted,
    Success { output: String },
    Error(String),
}

#[derive(Clone)]
pub(super) enum TranscriptEntry {
    System(String),
    Status(String),
    User(String),
    Parent(String),
    Stream(Line<'static>),
    SystemCard {
        title: String,
        rows: Vec<(String, String)>,
    },
    ToolCall {
        name: String,
        input_summary: String,
        status: ToolCallStatus,
    },
}

#[derive(Debug, Clone)]
pub(super) struct SlashOverlayItem {
    pub(super) command: String,
    pub(super) summary: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct SlashOverlay {
    pub(super) items: Vec<SlashOverlayItem>,
    pub(super) selected: usize,
    pub(super) scroll_offset: usize,
}

#[derive(Debug, Clone)]
pub(super) struct HeaderSnapshot {
    pub(super) model: String,
    pub(super) session_id: String,
    pub(super) cost_text: String,
    pub(super) context_text: String,
    pub(super) reasoning_effort: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum ModelCatalogState {
    Loading,
    Ready(Vec<api::provider::ModelInfo>),
    Failed,
}

impl Default for HeaderSnapshot {
    fn default() -> Self {
        Self {
            model: "--".to_string(),
            session_id: "--".to_string(),
            cost_text: "--".to_string(),
            context_text: "--".to_string(),
            reasoning_effort: None,
        }
    }
}

pub(super) struct MouseCaptureGuard;

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), event::DisableMouseCapture);
        let _ = execute!(io::stdout(), DisableBracketedPaste);
    }
}

enum WorkerMsg {
    RunTurn(String),
    Shutdown,
}

struct InputEditorState {
    text: String,
    cursor: usize,
    /// Byte-level position matching `cursor` — avoids O(n) `char_indices().nth()`
    /// scans in hot paths (paste, render).  Invalidated and lazily re-synced
    /// when the cursor is set directly (clamp / `set_line_col`).
    byte_cursor: usize,
    preferred_col: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InputUndoSnapshot {
    text: String,
    cursor: usize,
    preferred_col: Option<usize>,
    selection: Option<(usize, usize)>,
    paste_entries: Vec<PasteEntry>,
    next_paste_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PasteEntry {
    id: u32,
    content: String,
}

#[derive(Default)]
pub(super) struct SelectionState {
    pub(super) anchor: Option<(u16, usize)>,
    pub(super) end: Option<(u16, usize)>,
    pub(super) pending_copy: Option<bool>,
    pub(super) mouse_drag_occurred: bool,
    pub(super) suppress_paste_until: Option<Instant>,
}

pub(super) struct TypewriterState {
    chars: VecDeque<char>,
    pub(super) live: String,
}

#[allow(clippy::struct_excessive_bools)]
pub(super) struct ReplTuiState {
    ui_state: AppUiState,
    pub(super) entries: Vec<TranscriptEntry>,
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
    /// where start ≤ end.  `None` when no selection is active.
    pub(super) input_selection: Option<(usize, usize)>,
    /// Immutable anchor set on mouse Down(Left); used by Drag to extend the
    /// selection without ever being modified by other mouse events or key
    /// handlers.  `None` between drags.
    input_click_anchor: Option<usize>,
    /// Cached width (columns) of the input widget from the last render pass.
    /// Used by cursor up/down to compute soft-wrap boundaries.
    pub(super) input_area_width: u16,
    /// Cached result of `visual_line_info` for the current self.input.text.
    /// None means stale; recomputed on next access.
    vis_cache: Option<Vec<(usize, usize, bool)>>,
    /// The `safe_width` at which `vis_cache` was last computed.
    vis_cache_width: usize,
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
    status_entry_index: Option<usize>,
    persist_on_exit: bool,
    cursor_on: bool,
    cursor_blink_deadline: Instant,
    pub(super) slash_overlay: Option<SlashOverlay>,
    pub(super) last_slash_overlay_rect: Option<Rect>,
    pub(super) cached_header: HeaderSnapshot,
    pub(super) paused: bool,
    pub(super) pause_reason: String,
    last_wait_for_human_reason: Option<String>,
    spinner_tick: u8,
    spinner_deadline: Instant,
    pub(super) typewriter: TypewriterState,
    pub(super) selection: SelectionState,
    /// Accumulator for chars/newlines arriving faster than a human can type
    /// (≤ 30 ms apart). Flushed via `handle_paste_event` so masking and the
    /// paste-newline suppression window apply uniformly even on terminals that
    /// deliver pastes as raw keystrokes instead of `Event::Paste`.
    paste_burst_chars: Vec<char>,
    /// Timestamp of the most-recent `KeyCode::Char` or `KeyCode::Enter` event.
    /// Used to decide whether the next keystroke is part of the same burst.
    last_key_time: Option<Instant>,
    /// Windows Ctrl+V can be delivered as a consumed modified key followed by a
    /// delayed stream of plain `Char`/`Enter` events. Timing alone is not
    /// reliable for the first streamed key, so this flag forces that first key
    /// into the burst accumulator.
    force_next_paste_burst_key: bool,
    input_undo_stack: Vec<InputUndoSnapshot>,
    input_redo_stack: Vec<InputUndoSnapshot>,
    paste_entries: Vec<PasteEntry>,
    next_paste_id: u32,
    last_esc_at: Option<Instant>,
    pub(super) debug_mode: bool,
    pub(super) update_info: Option<runtime::update_check::UpdateInfo>,
    pub(super) update_rx:
        Option<tokio::sync::oneshot::Receiver<Option<runtime::update_check::UpdateInfo>>>,
    pub(super) child_tab_panel: child_tabs::ChildTabPanel,
    child_event_rx: Option<std::sync::mpsc::Receiver<crawler::ChildEvent>>,
    pub(super) child_control_registry: Option<crawler::ChildControlRegistry>,
    pub(super) view_mode: ViewMode,
}

impl ReplTuiState {
    fn new() -> Self {
        Self {
            ui_state: AppUiState::WelcomeMode,
            entries: Vec::new(),
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
            vis_cache: None,
            vis_cache_width: 0,
            input: InputEditorState {
                text: String::new(),
                cursor: 0,
                byte_cursor: 0,
                preferred_col: None,
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
            status_entry_index: None,
            cursor_on: true,
            cursor_blink_deadline: Instant::now() + Duration::from_millis(530),
            slash_overlay: None,
            last_slash_overlay_rect: None,
            cached_header: HeaderSnapshot::default(),
            paused: false,
            pause_reason: String::new(),
            last_wait_for_human_reason: None,
            spinner_tick: 0,
            spinner_deadline: Instant::now() + Duration::from_millis(120),
            typewriter: TypewriterState {
                chars: VecDeque::new(),
                live: String::new(),
            },
            paste_burst_chars: Vec::new(),
            last_key_time: None,
            force_next_paste_burst_key: false,
            selection: SelectionState::default(),
            input_undo_stack: Vec::new(),
            input_redo_stack: Vec::new(),
            paste_entries: Vec::new(),
            next_paste_id: 1,
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

    fn tick_input_caret(&mut self) {
        let now = Instant::now();
        let advance_spinner = now >= self.spinner_deadline;
        if now >= self.cursor_blink_deadline {
            self.cursor_on = true;
            self.cursor_blink_deadline = now + Duration::from_millis(530);
        }
        if advance_spinner {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
            self.spinner_deadline = now + Duration::from_millis(120);
        }
        if let Some(modal) = self
            .active_modal
            .as_mut()
            .and_then(ActiveModal::as_auth_mut)
        {
            if let AuthModalStep::OAuthWaiting { tick, .. } = &mut modal.step {
                if advance_spinner {
                    *tick = tick.wrapping_add(1);
                }
            }
        }
    }

    /// Returns the spinner frame matching the current tick.
    pub(super) fn spinner_char(&self) -> char {
        const FRAMES: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];
        if self.cancelling {
            return '◼';
        }
        FRAMES[usize::from(self.spinner_tick) % FRAMES.len()]
    }

    /// Context-aware placeholder shown when the input box is empty.
    fn input_placeholder(&self) -> &'static str {
        if self.busy {
            "AgenticCrawler is working…  (you can queue your next prompt)"
        } else if self.ui_state == AppUiState::WelcomeMode {
            "What is our goal today?"
        } else {
            "Any follow-up instructions?"
        }
    }

    /// Advance the typewriter: reveal `chars_per_tick` chars from the queue.
    ///
    /// Chars accumulate in `typewriter.live`; whenever the buffer reaches a
    /// stream-safe markdown boundary (paragraph end or closed code fence) we
    /// render that chunk through `tui-markdown` and push the resulting lines
    /// to the transcript. This preserves multi-line constructs like fenced
    /// code blocks (which need the whole block to syntax-highlight) at the
    /// cost of a slight delay before they appear during streaming.
    fn tick_typewriter(&mut self, chars_per_tick: usize) {
        for _ in 0..chars_per_tick {
            match self.typewriter.chars.pop_front() {
                None => break,
                Some(c) => self.typewriter.live.push(c),
            }
        }
        while let Some(styled_lines) = drain_safe_boundary(&mut self.typewriter.live) {
            for line in styled_lines {
                self.entries.push(TranscriptEntry::Stream(line));
            }
        }
    }

    fn flush_typewriter(&mut self) {
        if !self.typewriter.chars.is_empty() {
            let count = self.typewriter.chars.len();
            self.tick_typewriter(count);
        }
        if !self.typewriter.live.is_empty() {
            let pending: String = std::mem::take(&mut self.typewriter.live);
            for styled_line in render_lines(&pending) {
                self.entries.push(TranscriptEntry::Stream(styled_line));
            }
        }
    }

    fn wake_input_caret(&mut self) {
        self.cursor_on = true;
        self.cursor_blink_deadline = Instant::now() + Duration::from_millis(530);
    }

    fn current_input_snapshot(&self) -> InputUndoSnapshot {
        InputUndoSnapshot {
            text: self.input.text.clone(),
            cursor: self.input.cursor,
            preferred_col: self.input.preferred_col,
            selection: self.input_selection,
            paste_entries: self.paste_entries.clone(),
            next_paste_id: self.next_paste_id,
        }
    }

    fn apply_input_snapshot(&mut self, snapshot: InputUndoSnapshot) {
        self.input_scroll_manual = false;
        self.input.text = snapshot.text;
        self.vis_cache = None;
        self.input.cursor = snapshot.cursor.min(self.input_char_len());
        self.input.preferred_col = snapshot.preferred_col;
        self.input_selection = snapshot.selection;
        self.input_click_anchor = None;
        self.paste_entries = snapshot.paste_entries;
        self.next_paste_id = snapshot.next_paste_id;
        self.resync_byte_cursor();
        self.input_scroll_offset = usize::MAX;
    }

    const MAX_UNDO_HISTORY: usize = 100;

    fn record_input_undo_snapshot(&mut self) {
        let snapshot = self.current_input_snapshot();
        if self.input_undo_stack.last() != Some(&snapshot) {
            self.input_undo_stack.push(snapshot);
            if self.input_undo_stack.len() > Self::MAX_UNDO_HISTORY {
                self.input_undo_stack.remove(0);
            }
        }
        self.input_redo_stack.clear();
    }

    fn clear_input_history(&mut self) {
        self.input_undo_stack.clear();
        self.input_redo_stack.clear();
    }

    fn reset_input(&mut self) {
        self.input.text.clear();
        self.vis_cache = None;
        self.input.cursor = 0;
        self.input.byte_cursor = 0;
        self.input.preferred_col = None;
        self.paste_entries.clear();
        self.next_paste_id = 1;
    }

    /// Ctrl-W: delete backward to the previous word boundary.
    fn word_backspace(&mut self) {
        self.input_scroll_manual = false;
        self.clamp_input_cursor();
        if self.input.cursor == 0 {
            return;
        }
        self.record_input_undo_snapshot();
        if self.delete_selection_range() {
            return;
        }
        let bc = self.input.byte_cursor;
        let chars_before: Vec<(usize, char)> = self.input.text[..bc].char_indices().collect();
        let mut i = chars_before.len();
        while i > 0 && chars_before[i - 1].1.is_whitespace() {
            i -= 1;
        }
        while i > 0 {
            let (_, ch) = chars_before[i - 1];
            if ch.is_whitespace() || is_paste_sentinel(ch) {
                break;
            }
            i -= 1;
        }
        if i > 0 && is_paste_sentinel(chars_before[i - 1].1) {
            i -= 1;
        }
        let del_start = if i < chars_before.len() {
            chars_before[i].0
        } else {
            bc
        };
        if del_start == bc {
            return;
        }
        for &(_, ch) in &chars_before[i..] {
            if is_paste_sentinel(ch) {
                let id = sentinel_to_id(ch);
                self.paste_entries.retain(|e| e.id != id);
            }
        }
        self.input.text.replace_range(del_start..bc, "");
        self.vis_cache = None;
        self.input.byte_cursor = del_start;
        self.input.cursor = self.input.text[..del_start].chars().count();
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn undo_input_edit(&mut self) -> bool {
        let Some(snapshot) = self.input_undo_stack.pop() else {
            return false;
        };
        self.input_redo_stack.push(self.current_input_snapshot());
        self.apply_input_snapshot(snapshot);
        true
    }

    fn redo_input_edit(&mut self) -> bool {
        let Some(snapshot) = self.input_redo_stack.pop() else {
            return false;
        };
        self.input_undo_stack.push(self.current_input_snapshot());
        self.apply_input_snapshot(snapshot);
        true
    }

    fn input_char_len(&self) -> usize {
        self.input.text.chars().count()
    }

    /// Re-sync `byte_cursor` from `cursor` when the cursor is set directly
    /// (clamp / `set_input_cursor_line_col`).
    fn resync_byte_cursor(&mut self) {
        self.input.byte_cursor = self
            .input
            .text
            .char_indices()
            .nth(self.input.cursor)
            .map_or(self.input.text.len(), |(idx, _)| idx);
    }

    /// Returns the byte offset of character index `char_idx` by scanning the
    /// string.  Hot-path mutators (`insert_input_char`, `insert_input_str`,
    /// etc.) use the cached `byte_cursor` field directly instead.
    fn input_char_to_byte(&self, char_idx: usize) -> usize {
        self.input
            .text
            .char_indices()
            .nth(char_idx)
            .map_or(self.input.text.len(), |(idx, _)| idx)
    }

    fn clamp_input_cursor(&mut self) {
        let old = self.input.cursor;
        self.input.cursor = self.input.cursor.min(self.input_char_len());
        if self.input.cursor != old {
            self.resync_byte_cursor();
        }
    }

    /// If an input selection is active, delete the selected range, move the
    /// cursor to the anchor, and clear the selection.  Returns `true` if a
    /// selection was deleted.
    fn delete_selection_range(&mut self) -> bool {
        if let Some((a, b)) = self.input_selection.take() {
            self.input_click_anchor = None;
            self.input.cursor = a;
            self.resync_byte_cursor();
            let end_byte = self.input_char_to_byte(b);
            self.input
                .text
                .replace_range(self.input.byte_cursor..end_byte, "");
            self.cleanup_orphaned_paste_entries();
            self.vis_cache = None;
            self.input.preferred_col = None;
            self.input_scroll_offset = usize::MAX;
            true
        } else {
            false
        }
    }

    /// Remove any `PasteEntries` whose sentinel char is no longer present in
    /// `input.text`. Called after any bulk text mutation (selection delete,
    /// Ctrl-A + delete, etc.) to keep `paste_entries` in sync.
    fn cleanup_orphaned_paste_entries(&mut self) {
        let live_ids: std::collections::HashSet<u32> = self
            .input
            .text
            .chars()
            .filter(|&c| is_paste_sentinel(c))
            .map(sentinel_to_id)
            .collect();
        self.paste_entries.retain(|e| live_ids.contains(&e.id));
    }

    fn selected_input_text(&self) -> Option<&str> {
        let (a, b) = self.input_selection?;
        let sel_start = self.input_char_to_byte(a);
        let sel_end = self.input_char_to_byte(b);
        self.input.text.get(sel_start..sel_end)
    }

    /// Expand all paste sentinel chars in `text` by replacing each with its
    /// stored content from `paste_entries`. Called on the submit path and
    /// clipboard copy/cut paths.
    fn expand_paste_sentinels(&self, text: &str) -> String {
        if self.paste_entries.is_empty() {
            return text.to_string();
        }
        let mut result = String::with_capacity(text.len());
        for ch in text.chars() {
            if is_paste_sentinel(ch) {
                let id = sentinel_to_id(ch);
                if let Some(entry) = self.paste_entries.iter().find(|e| e.id == id) {
                    result.push_str(&entry.content);
                }
            } else {
                result.push(ch);
            }
        }
        result
    }

    /// Split `text` into styled spans, replacing paste sentinel chars with
    /// dim+italic pill spans showing the entry line count.
    fn spans_from_text_with_pills(&self, text: &str, base_style: Style) -> Vec<Span<'static>> {
        if !text.chars().any(is_paste_sentinel) {
            if text.is_empty() {
                return Vec::new();
            }
            return vec![Span::styled(text.to_string(), base_style)];
        }

        let pill_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM | Modifier::ITALIC);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current = String::new();

        for ch in text.chars() {
            if is_paste_sentinel(ch) {
                if !current.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut current), base_style));
                }

                let id = sentinel_to_id(ch);
                let pill_text = self.paste_entries.iter().find(|e| e.id == id).map_or_else(
                    || format!("[📋 #{id}]"),
                    |e| format_paste_pill(id, &e.content),
                );
                spans.push(Span::styled(pill_text, pill_style));
            } else {
                current.push(ch);
            }
        }

        if !current.is_empty() {
            spans.push(Span::styled(current, base_style));
        }

        spans
    }

    fn selected_input_text_string(&self) -> Option<String> {
        let raw = self.selected_input_text()?;
        Some(self.expand_paste_sentinels(raw))
    }

    fn cut_input_selection_text(&mut self) -> Option<String> {
        let text = self.selected_input_text_string()?;
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        Some(text)
    }

    fn insert_input_char(&mut self, ch: char) {
        self.input_scroll_manual = false;
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        self.clamp_input_cursor();
        self.input.text.insert(self.input.byte_cursor, ch);
        self.vis_cache = None;
        self.input.cursor = self.input.cursor.saturating_add(1);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(ch.len_utf8());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn insert_input_str(&mut self, text: &str) {
        self.input_scroll_manual = false;
        if text.is_empty() {
            return;
        }
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        self.clamp_input_cursor();
        self.input.text.insert_str(self.input.byte_cursor, text);
        self.vis_cache = None;
        let char_count = text.chars().count();
        self.input.cursor = self.input.cursor.saturating_add(char_count);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(text.len());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn insert_paste_mask(&mut self, content: &str) {
        self.input_scroll_manual = false;
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        self.clamp_input_cursor();
        let id = self.next_paste_id;
        self.next_paste_id += 1;
        let sentinel = id_to_sentinel(id);
        debug_assert!(is_paste_sentinel(sentinel));
        debug_assert_eq!(sentinel_to_id(sentinel), id);
        self.input.text.insert(self.input.byte_cursor, sentinel);
        self.vis_cache = None;
        self.input.cursor = self.input.cursor.saturating_add(1);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(sentinel.len_utf8());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
        self.paste_entries.push(PasteEntry {
            id,
            content: content.to_string(),
        });
    }

    /// Single entry point for any pasted text, regardless of source (bracketed
    /// paste, Ctrl+V, Shift+Insert, burst flush).  Normalises newlines and
    /// inserts the result directly into the input.
    ///
    /// Note: this does NOT set `suppress_paste_until` — that suppression is
    /// only appropriate for the bracketed-paste / Ctrl+V paths (where stray
    /// Enter events from the terminal can follow a paste).  The burst-flush
    /// path manages newlines via its own in-burst check on `KeyCode::Enter`,
    /// and adding suppression there would eat subsequent paste characters.
    /// Callers that need post-paste suppression set it themselves.
    fn handle_paste_event(&mut self, raw: &str) {
        let normalised = normalize_pasted_text(raw);
        if should_mask_paste(&normalised) {
            self.insert_paste_mask(&normalised);
        } else {
            self.insert_input_str(&normalised);
        }
    }

    /// Arm the post-paste suppression window used by the bracketed-paste and
    /// Ctrl+V paths.  Stray `KeyCode::Enter` events that some terminals emit
    /// for each `\n` in a paste are discarded for the next 100 ms so they
    /// don't trigger an accidental send.
    fn arm_paste_enter_suppression(&mut self) {
        self.selection.suppress_paste_until = Some(Instant::now() + Duration::from_millis(100));
    }

    /// Maximum gap (ms) between consecutive keystrokes considered a paste burst.
    /// Human typing at 150 WPM averages ≈ 80 ms/char; 30 ms is well below
    /// what any human can sustain.
    const PASTE_BURST_THRESHOLD_MS: u64 = 30;

    /// True if the previous key event arrived within the paste-burst threshold.
    fn in_paste_burst(&self, now: Instant) -> bool {
        self.last_key_time.is_some_and(|t| {
            now.duration_since(t) <= Duration::from_millis(Self::PASTE_BURST_THRESHOLD_MS)
        })
    }

    fn arm_streamed_paste_burst(&mut self, now: Instant) {
        self.last_key_time = Some(now);
        self.force_next_paste_burst_key = true;
    }

    fn should_capture_streamed_paste_key(&self, now: Instant) -> bool {
        self.force_next_paste_burst_key
            || self.in_paste_burst(now)
            || !self.paste_burst_chars.is_empty()
    }

    /// If the burst accumulator has any chars, drain it into the input via
    /// `handle_paste_event`.
    /// Called when the burst ends, or before any non-burst-compatible key.
    fn flush_paste_burst(&mut self) {
        if self.paste_burst_chars.is_empty() {
            return;
        }
        let chars = std::mem::take(&mut self.paste_burst_chars);
        let text: String = chars.iter().collect();
        self.handle_paste_event(&text);
    }

    /// Returns true if `key_code` should be suppressed because a paste that
    /// contained newlines was processed recently.  Used by the event loop and
    /// by tests to verify suppression without running the full event loop.
    fn paste_enter_is_suppressed(&self, key_code: KeyCode) -> bool {
        self.selection
            .suppress_paste_until
            .is_some_and(|deadline| Instant::now() <= deadline)
            && matches!(
                key_code,
                KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace
            )
    }

    fn backspace_input_char(&mut self) {
        self.input_scroll_manual = false;
        let had_selection = self.input_selection.is_some();
        self.clamp_input_cursor();
        if !had_selection && self.input.cursor == 0 {
            return;
        }
        self.record_input_undo_snapshot();
        // If selection is active, delete it instead of a single char.
        if self.delete_selection_range() {
            return;
        }
        // Find byte-offset of the character before cursor
        let prev_byte = if self.input.byte_cursor > 0 {
            // Walk backwards from byte_cursor to the previous char boundary
            let bytes = self.input.text.as_bytes();
            let mut pos = self.input.byte_cursor - 1;
            while pos > 0 && (bytes[pos] & 0xC0) == 0x80 {
                pos -= 1;
            }
            pos
        } else {
            0
        };
        let start = prev_byte;
        let end = self.input.byte_cursor;
        if let Some(prev_ch) = self.input.text[start..].chars().next() {
            if is_paste_sentinel(prev_ch) {
                let id = sentinel_to_id(prev_ch);
                self.paste_entries.retain(|e| e.id != id);
            }
        }
        self.input.text.replace_range(start..end, "");
        self.vis_cache = None;
        self.input.cursor -= 1;
        self.input.byte_cursor = start;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn delete_input_char(&mut self) {
        self.input_scroll_manual = false;
        self.clamp_input_cursor();
        if self.input_selection.is_none() && self.input.cursor >= self.input_char_len() {
            return;
        }
        self.record_input_undo_snapshot();
        if self.delete_selection_range() {
            return;
        }
        // Find byte-offset of the character after cursor
        let bytes = self.input.text.as_bytes();
        let mut end = self.input.byte_cursor + 1;
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        if let Some(ch) = self.input.text[self.input.byte_cursor..].chars().next() {
            if is_paste_sentinel(ch) {
                let id = sentinel_to_id(ch);
                self.paste_entries.retain(|e| e.id != id);
            }
        }
        self.input
            .text
            .replace_range(self.input.byte_cursor..end, "");
        self.vis_cache = None;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn input_cursor_line_col(&self) -> (usize, usize) {
        let mut line = 0usize;
        let mut col = 0usize;
        for (idx, ch) in self.input.text.chars().enumerate() {
            if idx == self.input.cursor {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += char_display_width(ch);
            }
        }
        (line, col)
    }

    fn input_lines(&self) -> Vec<&str> {
        self.input.text.split('\n').collect()
    }

    fn set_input_cursor_line_col(&mut self, target_line: usize, target_col: usize) {
        self.input_scroll_manual = false;
        let lines = self.input_lines();
        let line = target_line.min(lines.len().saturating_sub(1));
        let col = char_count_for_display_col(lines[line], target_col);
        let mut cursor = 0usize;
        for input_line in lines.iter().take(line) {
            cursor += input_line.chars().count() + 1;
        }
        cursor += col;
        self.input.cursor = cursor.min(self.input_char_len());
        self.resync_byte_cursor();
        self.input_scroll_offset = usize::MAX;
    }

    fn move_input_cursor_left(&mut self) {
        self.input_scroll_manual = false;
        self.input_selection = None;
        self.input_click_anchor = None;
        if self.input.cursor == 0 {
            return;
        }
        // Walk backwards from byte_cursor to the previous char boundary
        let bytes = self.input.text.as_bytes();
        let mut pos = self.input.byte_cursor.saturating_sub(1);
        while pos > 0 && (bytes[pos] & 0xC0) == 0x80 {
            pos -= 1;
        }
        self.input.byte_cursor = pos;
        self.input.cursor -= 1;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
        // Atomic skip: if we landed on a paste sentinel, move one more step left.
        if self.input.cursor > 0 {
            if let Some(ch) = self.input.text[self.input.byte_cursor..].chars().next() {
                if is_paste_sentinel(ch) {
                    let bytes = self.input.text.as_bytes();
                    let mut pos = self.input.byte_cursor.saturating_sub(1);
                    while pos > 0 && (bytes[pos] & 0xC0) == 0x80 {
                        pos -= 1;
                    }
                    self.input.byte_cursor = pos;
                    self.input.cursor -= 1;
                }
            }
        }
    }

    fn move_input_cursor_right(&mut self) {
        self.input_scroll_manual = false;
        self.input_selection = None;
        self.input_click_anchor = None;
        if self.input.cursor >= self.input_char_len() {
            return;
        }
        // Walk forward from byte_cursor past the current character
        let bytes = self.input.text.as_bytes();
        let mut end = self.input.byte_cursor + 1;
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        self.input.byte_cursor = end;
        self.input.cursor += 1;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
        // Atomic skip: if we landed on a paste sentinel, move one more step right.
        if self.input.cursor < self.input_char_len() {
            if let Some(ch) = self.input.text[self.input.byte_cursor..].chars().next() {
                if is_paste_sentinel(ch) {
                    let skip = ch.len_utf8();
                    self.input.byte_cursor += skip;
                    self.input.cursor += 1;
                }
            }
        }
    }

    fn move_input_cursor_home(&mut self) {
        let (line, _) = self.input_cursor_line_col();
        self.set_input_cursor_line_col(line, 0);
        self.input.preferred_col = Some(0);
    }

    fn move_input_cursor_end(&mut self) {
        let (line, _) = self.input_cursor_line_col();
        let target = self
            .input_lines()
            .get(line)
            .map_or(0, |input_line| text_display_width(input_line));
        self.set_input_cursor_line_col(line, target);
        self.input.preferred_col = Some(target);
    }

    fn select_all_input(&mut self) {
        let char_len = self.input_char_len();
        if char_len == 0 {
            return;
        }
        self.input_scroll_manual = false;
        self.input_selection = Some((0, char_len));
        self.input_click_anchor = None;
        self.input.cursor = char_len;
        self.resync_byte_cursor();
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    /// Compute visual line boundaries for the input text.
    ///
    /// Returns `Vec<(char_start, display_width, starts_paragraph)>`.
    ///
    /// - `char_start`: char-index of the first character on this visual line.
    /// - `display_width`: width of the visual line in terminal cells.
    /// - `starts_paragraph`: `true` iff this is the first visual line of a
    ///   logical paragraph after a `\n` separator (i.e. the char immediately
    ///   before `char_start` in the text is `\n`). Always `false` for the
    ///   very first visual line and for soft-wrapped continuation lines.
    ///
    /// Callers use `starts_paragraph` instead of re-scanning the text with
    /// `chars().nth()` to check for a trailing `\n` at a line boundary.
    fn visual_line_info(&mut self, safe_width: usize) -> Vec<(usize, usize, bool)> {
        if self.vis_cache_width == safe_width {
            if let Some(ref cached) = self.vis_cache {
                return cached.clone();
            }
        }
        let vis = self.visual_line_info_capped(safe_width, usize::MAX);
        self.vis_cache_width = safe_width;
        self.vis_cache = Some(vis.clone());
        vis
    }

    fn visual_line_info_capped(
        &self,
        safe_width: usize,
        max_lines: usize,
    ) -> Vec<(usize, usize, bool)> {
        if max_lines == 0 {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let mut char_idx = 0usize;
        let mut parts = self.input.text.split('\n').peekable();
        let mut logical_idx = 0usize;
        while let Some(logical_line) = parts.next() {
            let has_more = parts.peek().is_some();
            let prompt_offset = if logical_idx == 0 { 2usize } else { 0 };
            let first_cap = safe_width.saturating_sub(prompt_offset);
            let mut line_start = char_idx;
            let mut col = 0usize;
            let mut is_first_chunk = true;
            let mut has_chars = false;

            if logical_line.is_empty() {
                lines.push((char_idx, 0, logical_idx > 0));
                if lines.len() >= max_lines {
                    return lines;
                }
            } else {
                for ch in logical_line.chars() {
                    let target = if is_first_chunk {
                        first_cap
                    } else {
                        safe_width
                    };
                    let cw = char_display_width(ch);
                    if has_chars && col + cw > target {
                        lines.push((line_start, col, is_first_chunk && logical_idx > 0));
                        if lines.len() >= max_lines {
                            return lines;
                        }
                        line_start = char_idx;
                        col = 0;
                        is_first_chunk = false;
                    }

                    col += cw;
                    char_idx += 1;
                    has_chars = true;

                    if col >= target {
                        lines.push((line_start, col, is_first_chunk && logical_idx > 0));
                        if lines.len() >= max_lines {
                            return lines;
                        }
                        line_start = char_idx;
                        col = 0;
                        is_first_chunk = false;
                        has_chars = false;
                    }
                }

                if has_chars {
                    lines.push((line_start, col, is_first_chunk && logical_idx > 0));
                    if lines.len() >= max_lines {
                        return lines;
                    }
                }
            }

            if has_more {
                char_idx += 1;
            }

            logical_idx += 1;
        }
        lines
    }

    /// Counts total visual lines and locates the caret's visual line index in a
    /// single O(n) pass with **no heap allocation**.  Used by
    /// `calculate_input_dimensions` so the render path never triggers the
    /// allocating `visual_line_info` scan.
    fn count_visual_lines_with_caret(
        &self,
        safe_width: usize,
        caret_char_idx: usize,
    ) -> (usize, usize) {
        let mut total = 0usize;
        let mut caret_line = 0usize;
        let mut char_cursor = 0usize;
        let mut caret_found = false;
        let mut parts = self.input.text.split('\n').peekable();
        let mut logical_idx = 0usize;

        while let Some(part) = parts.next() {
            let has_more = parts.peek().is_some();
            let first_cap = safe_width.saturating_sub(if logical_idx == 0 { 2 } else { 0 });

            if part.is_empty() {
                if !caret_found && char_cursor == caret_char_idx {
                    caret_line = total;
                    caret_found = true;
                }
                total += 1;
            } else {
                if part.is_ascii() {
                    let len = part.len();

                    let caret_at_end = caret_char_idx == char_cursor + len;
                    let (n_visual, ends_on_wrap_boundary) = if len <= first_cap {
                        (1usize, len == first_cap)
                    } else {
                        let remaining = len - first_cap;
                        let full_tail_chunks = remaining / safe_width;
                        let tail_remainder = remaining % safe_width;
                        (
                            1 + full_tail_chunks + usize::from(tail_remainder > 0),
                            tail_remainder == 0,
                        )
                    };

                    if !caret_found
                        && caret_char_idx >= char_cursor
                        && caret_char_idx <= char_cursor + len
                    {
                        let offset = caret_char_idx - char_cursor;
                        caret_line = if offset < first_cap {
                            total
                        } else {
                            total + 1 + (offset - first_cap) / safe_width
                        };
                        caret_found = true;
                    }

                    total += n_visual + usize::from(caret_at_end && ends_on_wrap_boundary);
                    char_cursor += len;

                    if has_more {
                        char_cursor += 1;
                    }

                    logical_idx += 1;
                    continue;
                }

                let mut col = 0usize;
                let mut is_first_chunk = true;
                let mut has_chars = false;

                for c in part.chars() {
                    if !caret_found && char_cursor == caret_char_idx {
                        caret_line = total;
                        caret_found = true;
                    }

                    let target = if is_first_chunk {
                        first_cap
                    } else {
                        safe_width
                    };
                    let cw = char_display_width(c);

                    if has_chars && col + cw > target {
                        total += 1;
                        col = 0;
                        is_first_chunk = false;
                    }

                    col += cw;
                    char_cursor += 1;
                    has_chars = true;

                    if col >= target {
                        total += 1;
                        col = 0;
                        is_first_chunk = false;
                        has_chars = false;
                    }
                }

                if !caret_found && char_cursor == caret_char_idx {
                    caret_line = total;
                    caret_found = true;
                }

                if has_chars || char_cursor == caret_char_idx {
                    total += 1;
                }
            }

            if has_more {
                char_cursor += 1;
            }

            logical_idx += 1;
        }

        if !caret_found {
            caret_line = total.saturating_sub(1);
        }

        (total.max(1), caret_line)
    }

    fn move_input_cursor_up(&mut self) {
        self.move_input_cursor_up_inner();
    }

    fn move_input_cursor_up_inner(&mut self) {
        // Fall back to logical-line navigation until the widget width is known
        // (first render hasn't happened yet).
        if self.input_area_width == 0 {
            self.move_input_cursor_up_logical();
            return;
        }
        let w = usize::from(self.input_area_width).max(10);
        let vis = self.visual_line_info(w.saturating_sub(5).max(5));
        if vis.is_empty() {
            return;
        }
        let cur_char = self.input.cursor;
        // Find which visual line the cursor is on.
        // partition_point gives the last line whose start <= cur_char.
        let cur_vis = vis
            .partition_point(|(start, _, _)| *start <= cur_char)
            .saturating_sub(1);
        if cur_vis == 0 {
            let (start, _, _) = vis[0];
            self.set_input_cursor_line_col_by_char(start);
            self.input.preferred_col = self.input.preferred_col.or(Some(0));
            return;
        }
        let cur_start = vis[cur_vis].0;
        let cur_end = vis.get(cur_vis + 1).map_or(self.input_char_len(), |v| v.0);
        let cur_offset = cur_char.saturating_sub(cur_start);
        let clamped_offset = cur_offset.min(cur_end.saturating_sub(cur_start));
        let cur_display_col = self
            .input
            .text
            .chars()
            .skip(cur_start)
            .take(clamped_offset)
            .map(char_display_width)
            .sum::<usize>();
        let preferred_col = self.input.preferred_col.unwrap_or(cur_display_col);

        let (prev_start, prev_width, _) = vis[cur_vis - 1];
        // vis[cur_vis].2 is true when cur_vis starts a new logical paragraph,
        // meaning the char before it is a \n separator to exclude from the range.
        let raw_prev_end = vis[cur_vis].0;
        let prev_end = if vis[cur_vis].2 {
            raw_prev_end.saturating_sub(1)
        } else {
            raw_prev_end
        };
        if prev_width == 0 {
            self.set_input_cursor_line_col_by_char(prev_start);
            self.input.preferred_col = Some(preferred_col);
            return;
        }
        let target_col = preferred_col.min(prev_width);
        let prev_text: String = self
            .input
            .text
            .chars()
            .skip(prev_start)
            .take(prev_end.saturating_sub(prev_start))
            .collect();
        let col_chars = char_count_for_display_col(&prev_text, target_col);
        self.set_input_cursor_line_col_by_char(prev_start + col_chars);
        self.input.preferred_col = Some(preferred_col);
    }

    fn move_input_cursor_down(&mut self) {
        self.move_input_cursor_down_inner();
    }

    fn move_input_cursor_down_inner(&mut self) {
        // Fall back to logical-line navigation until the widget width is known.
        if self.input_area_width == 0 {
            self.move_input_cursor_down_logical();
            return;
        }
        let w = usize::from(self.input_area_width).max(10);
        let vis = self.visual_line_info(w.saturating_sub(5).max(5));
        if vis.is_empty() {
            return;
        }
        let cur_char = self.input.cursor;
        let cur_vis = vis
            .partition_point(|(start, _, _)| *start <= cur_char)
            .saturating_sub(1);
        if cur_vis + 1 >= vis.len() {
            // Already on the last visual line — go to end
            let (start, width, _) = vis[cur_vis];
            let line_text: String = self.input.text.chars().skip(start).collect();
            self.set_input_cursor_line_col_by_char(
                start + char_count_for_display_col(&line_text, width),
            );
            self.input.preferred_col = self.input.preferred_col.or(Some(width));
            return;
        }
        let cur_start = vis[cur_vis].0;
        let cur_end = vis[cur_vis + 1].0;
        let cur_offset = cur_char.saturating_sub(cur_start);
        let clamped_offset = cur_offset.min(cur_end.saturating_sub(cur_start));
        let cur_display_col = self
            .input
            .text
            .chars()
            .skip(cur_start)
            .take(clamped_offset)
            .map(char_display_width)
            .sum::<usize>();
        // Resolve preferred column from current position (or previous nav).
        let preferred_col = self.input.preferred_col.unwrap_or(cur_display_col);

        let (next_start, next_width, _) = vis[cur_vis + 1];
        let raw_next_end = vis.get(cur_vis + 2).map_or(self.input_char_len(), |v| v.0);
        // vis[cur_vis + 2].2 tells us whether the line after next starts a new
        // paragraph, meaning next's last char is the \n separator to exclude.
        let next_ends_with_nl = vis.get(cur_vis + 2).is_some_and(|v| v.2);
        let next_end = if next_ends_with_nl {
            raw_next_end.saturating_sub(1)
        } else {
            raw_next_end
        };
        // Empty visual line → land at its start; keep preferred_col.
        if next_width == 0 {
            self.set_input_cursor_line_col_by_char(next_start);
            self.input.preferred_col = Some(preferred_col);
            return;
        }
        let target_col = preferred_col.min(next_width);
        let next_text: String = self
            .input
            .text
            .chars()
            .skip(next_start)
            .take(next_end.saturating_sub(next_start))
            .collect();
        let col_chars = char_count_for_display_col(&next_text, target_col);
        self.set_input_cursor_line_col_by_char(next_start + col_chars);
        self.input.preferred_col = Some(preferred_col);
    }

    /// If the cursor is positioned before (on) a paste sentinel char, advance it
    /// forward past the sentinel. Called at the end of any cursor-setting
    /// operation that does not have an explicit direction preference.
    fn snap_cursor_off_sentinel(&mut self) {
        if let Some(ch) = self.input.text[self.input.byte_cursor..].chars().next() {
            if is_paste_sentinel(ch) {
                let skip = ch.len_utf8();
                self.input.byte_cursor += skip;
                self.input.cursor += 1;
            }
        }
    }

    /// Set cursor directly by character index (used by visual-line nav).
    fn set_input_cursor_line_col_by_char(&mut self, char_idx: usize) {
        self.input_scroll_manual = false;
        self.input_selection = None;
        self.input_click_anchor = None;
        self.input.cursor = char_idx.min(self.input_char_len());
        self.resync_byte_cursor();
        self.snap_cursor_off_sentinel();
        self.input_scroll_offset = usize::MAX;
    }

    /// Convert a mouse position (row, col) relative to the input widget into
    /// a character index in `input.text`.  Returns `None` if the position
    /// falls outside the text bounds.
    fn char_index_at_mouse(&mut self, widget_row: usize, widget_col: usize) -> Option<usize> {
        if widget_row == 0 {
            return None;
        }
        let w = usize::from(self.input_area_width).max(10);
        let safe_width = w.saturating_sub(5).max(5);
        let vis = self.visual_line_info(safe_width);
        let content_row = widget_row.saturating_sub(1);
        let abs_row = self.input_scroll_offset + content_row;
        let &(start, width, _) = vis.get(abs_row)?;
        let is_first = abs_row == 0;
        let prompt = if is_first { 2usize } else { 0 };
        if widget_col < prompt {
            return Some(start);
        }
        let target_col = widget_col.saturating_sub(prompt).min(width);
        // Compute the char end of this visual line.  The next visual line's
        // start gives the exclusive end; subtract 1 if it starts a new paragraph
        // (meaning the intervening char is a \n separator not part of either line).
        let raw_end = vis.get(abs_row + 1).map_or(self.input_char_len(), |v| v.0);
        let next_starts_paragraph = vis.get(abs_row + 1).is_some_and(|v| v.2);
        let line_end = if next_starts_paragraph {
            raw_end.saturating_sub(1)
        } else {
            raw_end
        };
        let mut char_idx = start;
        let mut col = 0usize;
        for ch in self
            .input
            .text
            .chars()
            .skip(start)
            .take(line_end.saturating_sub(start))
        {
            let cw = char_display_width(ch);
            if cw == 0 {
                continue;
            }
            if col + cw > target_col {
                break;
            }
            col += cw;
            char_idx += 1;
        }
        // Snap: if the resolved position is a paste sentinel, move past it.
        if self
            .input
            .text
            .chars()
            .nth(char_idx)
            .is_some_and(is_paste_sentinel)
        {
            return Some(char_idx + 1);
        }
        Some(char_idx)
    }

    /// Logical-line up/down (jumps by `\n`, ignoring soft wraps).  Used as a
    /// fallback before the widget width is known, and for Home/End which
    /// operate on logical lines.
    fn move_input_cursor_up_logical(&mut self) {
        let (line, col) = self.input_cursor_line_col();
        if line == 0 {
            self.set_input_cursor_line_col(0, 0);
            self.input.preferred_col = Some(0);
            return;
        }
        let target_col = self.input.preferred_col.unwrap_or(col);
        self.set_input_cursor_line_col(line - 1, target_col);
        self.input.preferred_col = Some(target_col);
    }

    fn move_input_cursor_down_logical(&mut self) {
        let line_widths = self
            .input_lines()
            .into_iter()
            .map(text_display_width)
            .collect::<Vec<_>>();
        let (line, col) = self.input_cursor_line_col();
        if line + 1 >= line_widths.len() {
            self.set_input_cursor_line_col(line, line_widths[line]);
            self.input.preferred_col = Some(line_widths[line]);
            return;
        }
        let target_col = self.input.preferred_col.unwrap_or(col);
        self.set_input_cursor_line_col(line + 1, target_col);
        self.input.preferred_col = Some(target_col);
    }

    fn handle_tool_call_start(&mut self, name: String, input: &str) {
        if !self.typewriter.chars.is_empty() {
            let count = self.typewriter.chars.len();
            self.tick_typewriter(count);
        }
        if !self.typewriter.live.is_empty() {
            let pending: String = std::mem::take(&mut self.typewriter.live);
            for styled_line in render_lines(&pending) {
                self.entries.push(TranscriptEntry::Stream(styled_line));
            }
        }
        if name == "wait_for_human" {
            self.last_wait_for_human_reason = serde_json::from_str::<serde_json::Value>(input)
                .ok()
                .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(String::from));
        }
        let input_summary = tool_input_summary(&name, input);
        self.ui_state = AppUiState::ChatMode;
        self.entries.push(TranscriptEntry::ToolCall {
            name,
            input_summary,
            status: ToolCallStatus::Running,
        });
    }

    fn handle_tool_call_complete(&mut self, name: &str, output: String, is_error: bool) {
        let status = if is_error {
            ToolCallStatus::Error(output)
        } else {
            ToolCallStatus::Success { output }
        };

        // Find the first Running entry with a matching tool name.
        // This correctly handles multiple parallel calls of the same tool
        // (e.g. two navigate calls in one assistant turn) because completions
        // arrive in the same order the calls were started, and each completion
        // consumes exactly the first still-Running entry.
        if let Some(TranscriptEntry::ToolCall {
            status: entry_status,
            ..
        }) = self.entries.iter_mut().find(|entry| {
            matches!(
                entry,
                TranscriptEntry::ToolCall {
                    name: entry_name,
                    status: ToolCallStatus::Running,
                    ..
                } if entry_name == name
            )
        }) {
            *entry_status = status;
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn calculate_input_dimensions(
        &mut self,
        width: u16,
        model_label: &str,
    ) -> (u16, Vec<Line<'static>>, usize, Option<(u16, u16)>) {
        self.input_area_width = width;
        self.clamp_input_cursor();
        let is_placeholder = self.input.text.is_empty();
        let placeholder_text = self.input_placeholder();
        let safe_width = width.saturating_sub(5).max(5) as usize;
        let max_text_lines = MAX_INPUT_LINES;

        let (total_visual, caret_visual_line) = if is_placeholder {
            (1usize, 0usize)
        } else if self.vis_cache_width == safe_width {
            if let Some(ref cached) = self.vis_cache {
                let total = cached.len().max(1);
                let caret = cached
                    .partition_point(|(s, _, _)| *s <= self.input.cursor)
                    .saturating_sub(1);
                (total, caret)
            } else {
                self.count_visual_lines_with_caret(safe_width, self.input.cursor)
            }
        } else {
            self.count_visual_lines_with_caret(safe_width, self.input.cursor)
        };

        let max_scroll = total_visual.saturating_sub(max_text_lines);
        if self.input_scroll_offset == usize::MAX {
            self.input_scroll_offset = max_scroll;
        } else {
            self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);
        }
        if !is_placeholder && !self.input_scroll_manual {
            if caret_visual_line < self.input_scroll_offset {
                self.input_scroll_offset = caret_visual_line;
            } else if caret_visual_line >= self.input_scroll_offset + max_text_lines {
                self.input_scroll_offset = caret_visual_line.saturating_sub(max_text_lines - 1);
            }
        }
        self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);

        let skip = self.input_scroll_offset;
        let visible_end = skip + max_text_lines;
        let mut visual_lines = Vec::with_capacity(max_text_lines);

        let input_char_width = |ch: char| {
            if ch == INPUT_CARET_MARKER || is_paste_sentinel(ch) {
                0
            } else {
                char_display_width(ch)
            }
        };

        if is_placeholder {
            let mut lines_data = placeholder_text
                .split('\n')
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            if lines_data.is_empty() {
                lines_data.push(String::new());
            }

            for (logical_idx, line) in lines_data.into_iter().enumerate() {
                let offset = if logical_idx == 0 { 2 } else { 0 };
                let first_line_width = safe_width.saturating_sub(offset);

                if line.is_empty() {
                    if visual_lines.len() < max_text_lines {
                        visual_lines.push((logical_idx == 0, String::new()));
                    }
                    continue;
                }

                let mut current = String::new();
                let mut w = 0;
                let mut is_first_chunk = true;

                for c in line.chars() {
                    let char_width = input_char_width(c);
                    let target = if is_first_chunk {
                        first_line_width
                    } else {
                        safe_width
                    };
                    if !current.is_empty() && w + char_width > target {
                        if visual_lines.len() < max_text_lines {
                            visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                        }
                        current = String::new();
                        w = 0;
                        is_first_chunk = false;
                    }

                    current.push(c);
                    w += char_width;

                    if w >= target && !current.is_empty() {
                        if visual_lines.len() < max_text_lines {
                            visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                        }
                        current = String::new();
                        w = 0;
                        is_first_chunk = false;
                    }
                }
                if !current.is_empty() && visual_lines.len() < max_text_lines {
                    visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                }
                if visual_lines.len() >= max_text_lines {
                    break;
                }
            }
        } else {
            let mut visual_line_idx = 0usize;
            let mut char_cursor = 0usize;
            let caret_char_idx = self.input.cursor;
            let mut parts = self.input.text.split('\n').peekable();
            let mut logical_idx = 0usize;

            'outer: while let Some(line) = parts.next() {
                let has_more = parts.peek().is_some();
                let offset = if logical_idx == 0 { 2 } else { 0 };
                let first_line_width = safe_width.saturating_sub(offset);
                let mut current = String::new();
                let mut w = 0usize;
                let mut is_first_chunk = true;

                let push_current =
                    |current: &mut String,
                     is_first_chunk: bool,
                     visual_line_idx: &mut usize,
                     visual_lines: &mut Vec<(bool, String)>| {
                        if *visual_line_idx >= skip && *visual_line_idx < visible_end {
                            visual_lines.push((
                                logical_idx == 0 && is_first_chunk,
                                std::mem::take(current),
                            ));
                        } else {
                            current.clear();
                        }
                        *visual_line_idx += 1;
                    };

                let push_char =
                    |c: char,
                     current: &mut String,
                     w: &mut usize,
                     is_first_chunk: &mut bool,
                     visual_line_idx: &mut usize,
                     visual_lines: &mut Vec<(bool, String)>| {
                        let char_width = input_char_width(c);
                        let target = if *is_first_chunk {
                            first_line_width
                        } else {
                            safe_width
                        };

                        if *w > 0 && *w + char_width > target {
                            push_current(current, *is_first_chunk, visual_line_idx, visual_lines);
                            *w = 0;
                            *is_first_chunk = false;
                        }

                        if *visual_line_idx >= skip {
                            current.push(c);
                        }
                        *w += char_width;

                        if *w >= target && *w > 0 {
                            push_current(current, *is_first_chunk, visual_line_idx, visual_lines);
                            *w = 0;
                            *is_first_chunk = false;
                        }
                    };

                if !line.is_empty() && line.is_ascii() {
                    let len = line.len();
                    let n_visual = if len <= first_line_width {
                        1usize
                    } else {
                        1 + (len - first_line_width).div_ceil(safe_width)
                    };

                    if visual_line_idx + n_visual <= skip {
                        visual_line_idx += n_visual;
                        char_cursor += len;
                        if has_more {
                            char_cursor += 1;
                        }
                        logical_idx += 1;
                        continue 'outer;
                    }
                }

                if line.is_empty() {
                    if char_cursor == caret_char_idx {
                        push_char(
                            INPUT_CARET_MARKER,
                            &mut current,
                            &mut w,
                            &mut is_first_chunk,
                            &mut visual_line_idx,
                            &mut visual_lines,
                        );
                    }

                    if current.is_empty() {
                        if visual_line_idx >= skip && visual_line_idx < visible_end {
                            visual_lines.push((logical_idx == 0, String::new()));
                        }
                        visual_line_idx += 1;
                    } else {
                        push_current(
                            &mut current,
                            is_first_chunk,
                            &mut visual_line_idx,
                            &mut visual_lines,
                        );
                    }

                    if visual_line_idx >= visible_end {
                        break;
                    }
                } else {
                    for c in line.chars() {
                        if char_cursor == caret_char_idx {
                            push_char(
                                INPUT_CARET_MARKER,
                                &mut current,
                                &mut w,
                                &mut is_first_chunk,
                                &mut visual_line_idx,
                                &mut visual_lines,
                            );
                            if visual_line_idx >= visible_end {
                                break 'outer;
                            }
                        }

                        push_char(
                            c,
                            &mut current,
                            &mut w,
                            &mut is_first_chunk,
                            &mut visual_line_idx,
                            &mut visual_lines,
                        );
                        char_cursor += 1;

                        if visual_line_idx >= visible_end {
                            break 'outer;
                        }
                    }

                    if char_cursor == caret_char_idx {
                        push_char(
                            INPUT_CARET_MARKER,
                            &mut current,
                            &mut w,
                            &mut is_first_chunk,
                            &mut visual_line_idx,
                            &mut visual_lines,
                        );
                    }

                    if !current.is_empty() || w > 0 {
                        push_current(
                            &mut current,
                            is_first_chunk,
                            &mut visual_line_idx,
                            &mut visual_lines,
                        );
                    }

                    if visual_line_idx >= visible_end {
                        break;
                    }
                }

                if has_more {
                    char_cursor += 1;
                }

                logical_idx += 1;
            }
        }

        // Only needed when a selection is active; line_start/line_end are
        // consumed exclusively in the input_selection branch below.
        let visual_ranges: Vec<(usize, usize)> = if is_placeholder || self.input_selection.is_none()
        {
            Vec::new()
        } else {
            let vis = if self.vis_cache_width == safe_width {
                if let Some(ref cached) = self.vis_cache {
                    cached.clone()
                } else {
                    self.visual_line_info_capped(safe_width, skip + max_text_lines + 1)
                }
            } else {
                self.visual_line_info_capped(safe_width, skip + max_text_lines + 1)
            };
            vis.iter()
                .enumerate()
                .map(|(idx, &(start, _, _))| {
                    let raw_end = vis.get(idx + 1).map_or(self.input_char_len(), |v| v.0);
                    let next_starts_paragraph = vis.get(idx + 1).is_some_and(|v| v.2);
                    let line_end = if next_starts_paragraph {
                        raw_end.saturating_sub(1)
                    } else {
                        raw_end
                    };
                    (start, line_end)
                })
                .collect()
        };

        let total_sliced = visual_lines.len();
        let mut cursor_pos: Option<(u16, u16)> = None;

        let text_style = if is_placeholder {
            Style::default().fg(Color::DarkGray)
        } else if self.busy {
            Style::default().fg(Color::Rgb(100, 100, 100)) // Dimmed text during AI turn
        } else {
            Style::default()
        };

        let mut render_lines = Vec::new();
        render_lines.push(Line::from(""));

        for (i, (has_prompt, row)) in visual_lines.into_iter().enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::new();

            if has_prompt {
                spans.push(Span::styled("❯ ", Style::default().fg(Color::LightCyan)));
            } else if skip > 0 && i == 0 {
                // If skipped first line with prompt, no visual space pad needed per standard terminal behavior
            }

            // Absolute visual-line index before skipping.
            let abs_i = skip + i;
            let (line_start, line_end) = visual_ranges.get(abs_i).copied().unwrap_or((0, 0));

            if is_placeholder && i == 0 {
                spans.push(Span::styled(row, text_style));
                let prompt_width = if has_prompt {
                    u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                } else {
                    0
                };
                cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
            } else if let Some((sel_a, sel_b)) = self.input_selection {
                let sel_a = sel_a.max(line_start).min(line_end);
                let sel_b = sel_b.max(line_start).min(line_end);
                let sel_style = Style::default().fg(Color::White).bg(Color::DarkGray);
                let mut marker_idx = None;
                for (idx, ch) in row.chars().enumerate() {
                    if ch == INPUT_CARET_MARKER {
                        marker_idx = Some(idx);
                        break;
                    }
                }
                if let Some(marker_char_idx) = marker_idx {
                    let left = row.chars().take(marker_char_idx).collect::<String>();
                    let right = row.chars().skip(marker_char_idx + 1).collect::<String>();
                    // Reconstruct the full text without the marker so we can
                    // apply selection-splitting on the full visual line.
                    let full: String = left.clone() + &right;
                    let prompt_width = if has_prompt {
                        u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                    } else {
                        0
                    };
                    if sel_a < sel_b {
                        let row_sel_start = sel_a - line_start;
                        let row_sel_end = sel_b - line_start;
                        let before_s: String = full.chars().take(row_sel_start).collect();
                        let selected_s: String = full
                            .chars()
                            .skip(row_sel_start)
                            .take(row_sel_end.saturating_sub(row_sel_start))
                            .collect();
                        let after_s: String = full.chars().skip(row_sel_end).collect();
                        spans.extend(self.spans_from_text_with_pills(&before_s, text_style));
                        spans.extend(self.spans_from_text_with_pills(&selected_s, sel_style));
                        spans.extend(self.spans_from_text_with_pills(&after_s, text_style));
                    } else {
                        spans.extend(self.spans_from_text_with_pills(&left, text_style));
                        spans.extend(self.spans_from_text_with_pills(&right, text_style));
                    }
                    if left.is_empty() {
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
                    } else {
                        let left_width = text_display_width(&left);
                        let cursor_col =
                            prompt_width + u16::try_from(left_width).unwrap_or(u16::MAX);
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), cursor_col));
                    }
                } else if sel_a < sel_b {
                    let row_sel_start = sel_a - line_start;
                    let row_sel_end = sel_b - line_start;
                    let before: String = row.chars().take(row_sel_start).collect();
                    let selected: String = row
                        .chars()
                        .skip(row_sel_start)
                        .take(row_sel_end - row_sel_start)
                        .collect();
                    let after: String = row.chars().skip(row_sel_end).collect();
                    spans.extend(self.spans_from_text_with_pills(&before, text_style));
                    spans.extend(self.spans_from_text_with_pills(&selected, sel_style));
                    spans.extend(self.spans_from_text_with_pills(&after, text_style));
                } else if !row.is_empty() {
                    spans.extend(self.spans_from_text_with_pills(&row, text_style));
                }
            } else {
                // No active selection — plain rendering (existing logic).
                let mut marker_idx = None;
                for (idx, ch) in row.chars().enumerate() {
                    if ch == INPUT_CARET_MARKER {
                        marker_idx = Some(idx);
                        break;
                    }
                }
                if let Some(marker_char_idx) = marker_idx {
                    let left = row.chars().take(marker_char_idx).collect::<String>();
                    let right = row.chars().skip(marker_char_idx + 1).collect::<String>();
                    let prompt_width = if has_prompt {
                        u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                    } else {
                        0
                    };
                    if left.is_empty() {
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
                    } else {
                        let left_width = text_display_width(&left);
                        spans.extend(self.spans_from_text_with_pills(&left, text_style));
                        let cursor_col =
                            prompt_width + u16::try_from(left_width).unwrap_or(u16::MAX);
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), cursor_col));
                    }
                    if !right.is_empty() {
                        spans.extend(self.spans_from_text_with_pills(&right, text_style));
                    }
                } else if !row.is_empty() {
                    spans.extend(self.spans_from_text_with_pills(&row, text_style));
                }
            }
            render_lines.push(Line::from(spans));
        }

        render_lines.push(Line::from(""));
        render_lines.push(Line::from(Span::styled(
            format!("Model: {model_label}"),
            Style::default()
                .fg(Color::Rgb(128, 136, 146))
                .add_modifier(Modifier::DIM),
        )));

        #[allow(clippy::cast_possible_truncation)]
        let box_height = (total_sliced as u16) + 5;
        (box_height, render_lines, max_scroll, cursor_pos)
    }

    fn push_user_line(&mut self, text: &str) {
        self.ui_state = AppUiState::ChatMode;
        self.entries
            .push(TranscriptEntry::User(text.trim().to_string()));
        self.follow_bottom = true;
    }

    fn push_system(&mut self, msg: &str) {
        self.ui_state = AppUiState::ChatMode;
        for row in msg.lines() {
            if row.is_empty() {
                self.entries.push(TranscriptEntry::System(" ".to_string()));
            } else {
                self.entries.push(TranscriptEntry::System(row.to_string()));
            }
        }
        self.follow_bottom = true;
    }

    fn push_system_card(&mut self, title: impl Into<String>, report: &str) {
        self.ui_state = AppUiState::ChatMode;
        self.entries.push(TranscriptEntry::SystemCard {
            title: title.into(),
            rows: parse_report_rows(report),
        });
        self.follow_bottom = true;
    }

    fn refresh_slash_overlay(&mut self) {
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

    fn selected_slash_command(&self) -> Option<String> {
        self.slash_overlay.as_ref().and_then(|overlay| {
            overlay
                .items
                .get(overlay.selected)
                .map(|item| item.command.clone())
        })
    }

    fn slash_overlay_select_prev(&mut self) {
        if let Some(overlay) = self.slash_overlay.as_mut() {
            if overlay.selected > 0 {
                overlay.selected -= 1;
                if overlay.selected < overlay.scroll_offset {
                    overlay.scroll_offset = overlay.selected;
                }
            }
        }
    }

    fn slash_overlay_select_next(&mut self) {
        if let Some(overlay) = self.slash_overlay.as_mut() {
            overlay.selected = min(overlay.selected + 1, overlay.items.len() - 1);
            let visible_count = min(overlay.items.len(), SLASH_OVERLAY_VISIBLE_ITEMS);
            if overlay.selected >= overlay.scroll_offset + visible_count {
                overlay.scroll_offset = overlay.selected - visible_count + 1;
            }
        }
    }

    pub(super) fn clamp_scroll_offset(&mut self) {
        let max_offset = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        if self.list_state.offset() > max_offset {
            *self.list_state.offset_mut() = max_offset;
        }
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        let max_offset = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        *self.list_state.offset_mut() = max_offset;
    }

    #[allow(clippy::too_many_lines)]
    fn drain_events(&mut self, rx: &Receiver<ReplTuiEvent>) {
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
                    self.current_tool = None;
                    self.status_line = "Thinking...".to_string();
                    self.status_entry_index = Some(self.entries.len());
                    self.entries.push(TranscriptEntry::Status(
                        "· Thinking about next move...".to_string(),
                    ));
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

                    for entry in &mut self.entries {
                        if let TranscriptEntry::ToolCall {
                            status: status @ ToolCallStatus::Running,
                            ..
                        } = entry
                        {
                            *status = ToolCallStatus::Interrupted;
                        }
                    }

                    self.status_line = match &result {
                        Ok(()) => "Ready".to_string(),
                        Err(e) => format!("Error: {e}"),
                    };

                    // Flush any remaining characters in the typewriter and clear status
                    self.flush_typewriter();

                    // Remove status line on finish
                    if let Some(idx) = self.status_entry_index.take() {
                        if idx < self.entries.len()
                            && matches!(self.entries[idx], TranscriptEntry::Status(_))
                        {
                            self.entries.remove(idx);
                        }
                    }
                    if let Err(e) = result {
                        self.push_system(&format!("Error: {e}"));
                    }
                }
                ReplTuiEvent::SystemMessage(s) => {
                    self.push_system(&s);
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
                if matches!(
                    child_ev.event,
                    crawler::ChildEventKind::PauseRequested { .. }
                ) && matches!(self.view_mode, ViewMode::Parent)
                {
                    self.view_mode = ViewMode::Child(child_ev.child_id.clone());
                }
            }
        }
    }

    fn reconcile_child_view_mode(&mut self) {
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

fn build_session_modal_entries(
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
fn handle_session_modal_outcome(
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
                        if let Err(e) = guard.clear_session_command(true) {
                            state.push_system_card(
                                "Session Error",
                                &format!("Deleted current session but failed to reset: {e}"),
                            );
                        }
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
fn handle_slash_command_tui(
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
        SlashCommand::Clear { confirm } => {
            let mut g = cli.lock().expect("cli lock");
            let result = g.clear_session_command(confirm)?;
            if result.persist_after {
                g.persist_session()?;
            }
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
                "Debug mode {label} — tool calls show {}",
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

fn extract_openai_account_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .or_else(|| {
            claims
                .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            claims
                .pointer("/organizations/0/id")
                .and_then(|v| v.as_str())
        })
        .map(String::from)
}

fn spawn_extension_connection_watch(cli: &Arc<Mutex<LiveCli>>, ui_tx: &mpsc::Sender<ReplTuiEvent>) {
    let connection_watch = {
        let g = cli.lock().expect("cli lock");
        g.extension_connection_watch()
    };
    let Some(watch) = connection_watch else {
        return;
    };
    spawn_extension_connection_watch_from_receiver(watch, cli, ui_tx);
}

fn spawn_extension_connection_watch_from_receiver(
    mut connection_watch: tokio::sync::watch::Receiver<bool>,
    cli: &Arc<Mutex<LiveCli>>,
    ui_tx: &mpsc::Sender<ReplTuiEvent>,
) {
    let cli_clone = cli.clone();
    let ui_tx_clone = ui_tx.clone();
    std::thread::spawn(move || {
        let rt = crate::TOKIO_RUNTIME.get().expect("tokio runtime");
        let connected = rt.block_on(async {
            if *connection_watch.borrow() {
                true
            } else {
                connection_watch.changed().await.is_ok() && *connection_watch.borrow()
            }
        });
        if connected {
            let setup = {
                let mut g = cli_clone.lock().expect("cli lock");
                g.prepare_extension_bridge_activation()
            };
            let result = match setup {
                Ok((shared, saved_state)) => {
                    let init_result = rt.block_on(async {
                        prime_extension_bridge(&shared, saved_state.as_ref()).await
                    });
                    match init_result {
                        Ok(()) => {
                            let mut g = cli_clone.lock().expect("cli lock");
                            g.activate_extension_bridge(shared);
                            Ok(())
                        }
                        Err(error) => {
                            let mut g = cli_clone.lock().expect("cli lock");
                            g.restore_pending_extension_state(saved_state);
                            Err(error)
                        }
                    }
                }
                Err(error) => Err(error),
            };
            let _ = ui_tx_clone.send(ReplTuiEvent::ExtensionBridgeResult {
                success: result.is_ok(),
                message: match result {
                    Ok(()) => "Extension bridge\n  \
                              Result           connected — browser commands routed to extension"
                        .to_string(),
                    Err(error) => format!("Extension bridge\n  Error            {error}"),
                },
            });
        }
    });
}

async fn prime_extension_bridge(
    shared: &crawler::SharedBridge,
    saved_state: Option<&crawler::BrowserState>,
) -> Result<(), String> {
    let mut bridge = shared.lock().await;
    if let Some(state) = saved_state {
        bridge
            .new_page(None)
            .await
            .map_err(|error| error.to_string())?;

        bridge
            .import_cookies_only(state)
            .await
            .map_err(|error| error.to_string())?;

        if !state.url.is_empty() && state.url != "about:blank" {
            bridge
                .navigate(&state.url)
                .await
                .map_err(|error| error.to_string())?;
            bridge
                .import_local_storage(state)
                .await
                .map_err(|error| error.to_string())?;
        }
    }

    Ok(())
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let standard = input.replace('-', "+").replace('_', "/");
    let padded = match standard.len() % 4 {
        2 => format!("{standard}=="),
        3 => format!("{standard}="),
        _ => standard,
    };
    let table: [u8; 256] = {
        let mut t = [255u8; 256];
        for (i, &c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            .iter()
            .enumerate()
        {
            #[allow(clippy::cast_possible_truncation)]
            {
                t[c as usize] = i as u8;
            }
        }
        t[b'=' as usize] = 0;
        t
    };
    let bytes: Vec<u8> = padded
        .bytes()
        .filter(|&b| b != b'\n' && b != b'\r')
        .collect();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let (a, b, c, d) = (
            table[chunk[0] as usize],
            table[chunk[1] as usize],
            table[chunk[2] as usize],
            table[chunk[3] as usize],
        );
        if a == 255 || b == 255 || c == 255 || d == 255 {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    Some(out)
}

#[allow(clippy::too_many_lines)]
fn spawn_anthropic_oauth_thread(
    ui_tx: Sender<ReplTuiEvent>,
    active_modal: &mut Option<ActiveModal>,
) {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    if let Some(modal) = active_modal.as_mut().and_then(ActiveModal::as_auth_mut) {
        if let AuthModalStep::OAuthWaiting {
            cancel_tx: ref mut tx,
            ..
        } = modal.step
        {
            *tx = Some(cancel_tx);
        }
    }
    let ui_tx2 = ui_tx.clone();
    thread::spawn(move || {
        let result: Result<(), Box<dyn std::error::Error + Send>> = (|| {
            use crate::app::{
                bind_oauth_listener, default_oauth_config, open_browser,
                wait_for_oauth_callback_cancellable,
            };
            use api::{AnthropicClient, AuthSource};
            use runtime::{
                generate_pkce_pair, generate_state, loopback_redirect_uri,
                OAuthAuthorizationRequest, OAuthTokenExchangeRequest,
            };

            let oauth = default_oauth_config();
            let preferred_port = oauth.callback_port.unwrap_or(4545);
            let (listener, actual_port) = bind_oauth_listener(preferred_port)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let redirect_uri = loopback_redirect_uri(actual_port);
            let pkce = generate_pkce_pair()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let state_val =
                generate_state().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let authorize_url = OAuthAuthorizationRequest::from_config(
                &oauth,
                redirect_uri.clone(),
                state_val.clone(),
                &pkce,
            )
            .build_url();
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: "Opening browser...".to_string(),
            });
            if let Err(err) = open_browser(&authorize_url) {
                let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                    message: format!("Browser failed. Visit: {authorize_url}  ({err})"),
                });
            }
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: format!("Waiting for OAuth callback on port {actual_port}…"),
            });
            let callback = wait_for_oauth_callback_cancellable(listener, cancel_rx)?;
            if let Some(error) = callback.error {
                let desc = callback.error_description.unwrap_or_default();
                return Err(Box::new(std::io::Error::other(format!("{error}: {desc}"))) as _);
            }
            let code = callback.code.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing code",
                )) as Box<dyn std::error::Error + Send>
            })?;
            let returned_state = callback.state.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing state",
                )) as Box<dyn std::error::Error + Send>
            })?;
            if returned_state != state_val {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "oauth state mismatch",
                )) as _);
            }
            let client = AnthropicClient::from_auth(AuthSource::None);
            let exchange = OAuthTokenExchangeRequest::from_config(
                &oauth,
                code,
                state_val,
                pkce.verifier,
                redirect_uri,
            );
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let token_set = rt
                .block_on(client.exchange_oauth_code(&oauth, &exchange))
                .map_err(|e| Box::new(std::io::Error::other(e.to_string())) as _)?;
            let mut store = crate::auth::load_credentials_or_warn();
            api::credentials::set_provider_config(
                &mut store,
                "anthropic",
                api::StoredProviderConfig {
                    auth_method: "oauth".to_string(),
                    oauth: Some(api::StoredOAuthTokens {
                        access_token: token_set.access_token.clone(),
                        refresh_token: token_set.refresh_token.clone(),
                        expires_at: token_set.expires_at.and_then(|v| i64::try_from(v).ok()),
                        scopes: token_set.scopes.clone(),
                        account_id: None,
                    }),
                    ..Default::default()
                },
            );
            api::credentials::save_credentials(&store)
                .map_err(|e| Box::new(std::io::Error::other(e.to_string())) as _)?;
            Ok(())
        })();
        let _ = ui_tx.send(ReplTuiEvent::AuthOAuthComplete {
            provider: "anthropic".to_string(),
            result: result.map_err(|e| e.to_string()),
        });
    });
}

#[allow(clippy::too_many_lines)]
fn spawn_openai_oauth_thread(ui_tx: Sender<ReplTuiEvent>, active_modal: &mut Option<ActiveModal>) {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    if let Some(modal) = active_modal.as_mut().and_then(ActiveModal::as_auth_mut) {
        if let AuthModalStep::OAuthWaiting {
            cancel_tx: ref mut tx,
            ..
        } = modal.step
        {
            *tx = Some(cancel_tx);
        }
    }
    let ui_tx2 = ui_tx.clone();
    thread::spawn(move || {
        let result: Result<(), Box<dyn std::error::Error + Send>> = (|| {
            use crate::app::{
                bind_oauth_listener, open_browser, wait_for_oauth_callback_cancellable,
            };
            use api::{AnthropicClient, AuthSource};
            use runtime::OAuthTokenExchangeRequest;

            let (listener, actual_port) = bind_oauth_listener(api::CODEX_CALLBACK_PORT)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let login_request = api::codex_login(actual_port).map_err(|e| {
                Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error + Send>
            })?;
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: "Opening browser for OpenAI login...".to_string(),
            });
            if let Err(err) = open_browser(&login_request.authorization_url) {
                let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                    message: format!(
                        "Browser failed. Visit: {}  ({err})",
                        login_request.authorization_url
                    ),
                });
            }
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: format!("Waiting for Codex OAuth callback on port {actual_port}…"),
            });
            let callback = wait_for_oauth_callback_cancellable(listener, cancel_rx)?;
            if let Some(error) = callback.error {
                let desc = callback.error_description.unwrap_or_default();
                return Err(Box::new(std::io::Error::other(format!("{error}: {desc}"))) as _);
            }
            let code = callback.code.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing code",
                )) as Box<dyn std::error::Error + Send>
            })?;
            let returned_state = callback.state.ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "callback missing state",
                )) as Box<dyn std::error::Error + Send>
            })?;
            if returned_state != login_request.state {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "oauth state mismatch",
                )) as _);
            }
            let client = AnthropicClient::from_auth(AuthSource::None);
            let exchange = OAuthTokenExchangeRequest::from_config(
                &login_request.config,
                code,
                login_request.state,
                login_request.pkce.verifier,
                login_request.redirect_uri,
            );
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            let token_set = rt
                .block_on(client.exchange_oauth_code(&login_request.config, &exchange))
                .map_err(|e| Box::new(std::io::Error::other(e.to_string())) as _)?;
            let account_id = extract_openai_account_id(&token_set.access_token);
            let oauth_tokens = api::StoredOAuthTokens {
                access_token: token_set.access_token,
                refresh_token: token_set.refresh_token,
                expires_at: token_set.expires_at.and_then(|v| i64::try_from(v).ok()),
                scopes: token_set.scopes,
                account_id,
            };
            let mut store = crate::auth::load_credentials_or_warn();
            let mut cfg = store.providers.get("openai").cloned().unwrap_or_default();
            cfg.auth_method = "oauth".to_string();
            cfg.oauth = Some(oauth_tokens);
            api::credentials::set_provider_config(&mut store, "openai", cfg);
            api::credentials::save_credentials(&store)
                .map_err(|e| Box::new(std::io::Error::other(e.to_string())) as _)?;
            Ok(())
        })();
        let _ = ui_tx.send(ReplTuiEvent::AuthOAuthComplete {
            provider: "openai".to_string(),
            result: result.map_err(|e| e.to_string()),
        });
    });
}

/// Interactive REPL using Ratatui. Requires a TTY on stdout — the caller must gate accordingly.
pub fn run_repl_ratatui(
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

    let mut terminal = ratatui::init();
    let work_shutdown = work_tx.clone();
    let result = run_loop(&mut terminal, &ui_rx, &ui_tx, &work_tx, &cli, &cancel_flag);
    let _ = work_shutdown.send(WorkerMsg::Shutdown);
    ratatui::restore();
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
        // the threshold — covers pastes that end without a subsequent key.
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

        // Detect pause state directly from ControlState — the observer event may not
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
                            if let Some(text) = state.selected_input_text_string() {
                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                    let _ = cb.set_text(text);
                                }
                            }
                            state.input_selection = None;
                            state.input_click_anchor = None;
                        }
                        _ => {
                            // Mouse events that should clear the selection
                            // (e.g. Up, Moved) should NOT clear it here —
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
                state.force_next_paste_burst_key = false;
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
                // Any key that isn't bare Char/Enter ends a paste burst — flush
                // the buffer here so command handlers (Ctrl-A/Z/Y/W/C/X, etc.)
                // see a consistent input state.
                if !matches!(key.code, KeyCode::Char(_) | KeyCode::Enter)
                    || !key.modifiers.is_empty()
                {
                    state.flush_paste_burst();
                    state.force_next_paste_burst_key = false;
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
                    && key.code == KeyCode::Insert
                    && key.modifiers.contains(KeyModifiers::SHIFT)
                {
                    // Manual paste supersedes any in-flight burst accumulation.
                    state.flush_paste_burst();
                    state.last_key_time = None;
                    state.force_next_paste_burst_key = false;
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

                // Ctrl+V: on Windows terminals the clipboard content is streamed as
                // individual keystrokes rather than delivered as a single Event::Paste.
                // Consume this key event and prime last_key_time so the streamed chars
                // land in the burst accumulator; flush_paste_burst then applies masking
                // atomically when the next non-burst event fires (e.g. Enter).
                if state.active_modal.is_none()
                    && key.code == KeyCode::Char('v')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    state.arm_streamed_paste_burst(Instant::now());
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
                    if let Some(text) = state.selected_input_text_string() {
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
                                            state.entries.clear();
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
                        state.push_system("Interrupting…");
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
                        state.push_system(&format!("Reasoning effort → {effort}"));
                    } else {
                        state.push_system("Reasoning effort → off");
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
                        if state.should_capture_streamed_paste_key(now) {
                            state.paste_burst_chars.push('\n');
                            state.force_next_paste_burst_key = false;
                            state.last_key_time = Some(now);
                            state.wake_input_caret();
                            continue;
                        }
                        state.force_next_paste_burst_key = false;
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
                                    state.vis_cache = None;
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
                        let line = state.expand_paste_sentinels(&raw_line);
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
                        if state.should_capture_streamed_paste_key(now) {
                            state.paste_burst_chars.push(c);
                            state.force_next_paste_burst_key = false;
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
                            state.vis_cache = None;
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
                                state.vis_cache = None;
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
                                state.push_system("Interrupting…");
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use std::sync::mpsc;

    use crate::app::Provider;
    use crate::display_width::{char_display_width, text_display_width};
    use crate::tool_format::tool_input_summary;
    use crate::tui::auth_modal::{AuthModal, AuthModalStep, ProviderKind};
    use crate::tui::repl_render::{line_to_plain_text, render_tool_call_lines, wrap_ansi_line};
    use crate::tui::ReplTuiEvent;
    use crossterm::event::KeyCode;
    use ratatui::style::Color;
    use ratatui::text::Line;

    use super::{
        format_paste_pill, id_to_sentinel, is_paste_sentinel, normalize_pasted_text,
        sentinel_to_id, should_mask_paste, ReplTuiState, ToolCallStatus, TranscriptEntry,
    };

    fn test_state() -> ReplTuiState {
        ReplTuiState::new()
    }

    fn simulate_windows_ctrl_v(state: &mut ReplTuiState, at: Instant) {
        state.arm_streamed_paste_burst(at);
    }

    fn simulate_streamed_char(state: &mut ReplTuiState, c: char, at: Instant) {
        if state.should_capture_streamed_paste_key(at) {
            state.paste_burst_chars.push(c);
            state.force_next_paste_burst_key = false;
        } else {
            state.insert_input_char(c);
        }
        state.last_key_time = Some(at);
    }

    fn simulate_idle_burst_flush(state: &mut ReplTuiState, now: Instant) {
        let should_flush = !state.paste_burst_chars.is_empty()
            && state.last_key_time.is_some_and(|t| {
                now.duration_since(t) > Duration::from_millis(ReplTuiState::PASTE_BURST_THRESHOLD_MS)
            });
        if should_flush {
            state.flush_paste_burst();
        }
    }

    #[test]
    fn normalize_pasted_text_handles_crlf_and_cr() {
        assert_eq!(normalize_pasted_text("a\r\nb"), "a\nb");
        assert_eq!(normalize_pasted_text("a\rb"), "a\nb");
        assert_eq!(normalize_pasted_text("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(normalize_pasted_text("plain"), "plain");
    }

    #[test]
    fn normalize_pasted_text_borrows_clean_input() {
        assert!(matches!(
            normalize_pasted_text("plain"),
            std::borrow::Cow::Borrowed("plain")
        ));
    }

    #[test]
    fn should_mask_paste_threshold_bytes() {
        let big = "x".repeat(2048);
        assert!(should_mask_paste(&big));
        let small = "x".repeat(2047);
        assert!(!should_mask_paste(&small));
    }

    #[test]
    fn should_mask_paste_threshold_lines() {
        let thirty_newlines = "\n".repeat(30);
        assert!(should_mask_paste(&thirty_newlines));
        let twenty_nine = "\n".repeat(29);
        assert!(!should_mask_paste(&twenty_nine));
    }

    #[test]
    fn format_paste_pill_formats_correctly() {
        let content = "a\nb\nc\nd\ne\nf";
        let pill = format_paste_pill(1, content);
        assert!(pill.contains("~6 lines"));
        assert!(pill.contains("#1"));
    }

    #[test]
    fn char_display_width_for_sentinel_requires_local_zero_width_guard() {
        let sentinel = id_to_sentinel(1);
        assert_eq!(char_display_width(sentinel), 1);
    }

    #[test]
    fn insert_paste_mask_stores_sentinel_and_entry() {
        let mut state = test_state();
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        assert_eq!(state.input.text.chars().count(), 1);
        let c = state.input.text.chars().next().unwrap();
        assert!(is_paste_sentinel(c));
        assert_eq!(state.paste_entries.len(), 1);
        assert_eq!(state.paste_entries[0].content, big);
    }

    #[test]
    fn reset_input_clears_paste_entries() {
        let mut state = test_state();
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        assert_eq!(state.paste_entries.len(), 1);
        state.reset_input();
        assert!(state.paste_entries.is_empty());
        assert_eq!(state.next_paste_id, 1);
    }

    #[test]
    fn expand_paste_sentinels_noop_when_no_entries() {
        let state = test_state();
        let text = "hello world";
        let expanded = state.expand_paste_sentinels(text);
        assert_eq!(expanded, text);
    }

    #[test]
    fn expand_paste_sentinels_replaces_sentinel_with_content() {
        let mut state = test_state();
        state.insert_input_str("before ");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.insert_input_str(" after");
        let expanded = state.expand_paste_sentinels(&state.input.text.clone());
        assert!(expanded.contains("before "));
        assert!(expanded.contains(&big));
        assert!(expanded.contains(" after"));
        assert!(!expanded.chars().any(is_paste_sentinel));
    }

    #[test]
    fn undo_after_mask_insert_restores_pre_paste_state() {
        let mut state = test_state();
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        assert_eq!(state.paste_entries.len(), 1);
        state.undo_input_edit();
        assert!(state.input.text.is_empty());
        assert!(state.paste_entries.is_empty());
    }

    #[test]
    fn backspace_after_sentinel_removes_entry() {
        let mut state = test_state();
        state.insert_input_str("ab");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);

        assert_eq!(state.paste_entries.len(), 1);

        state.backspace_input_char();

        assert!(state.input.text.chars().all(|c| !is_paste_sentinel(c)));
        assert!(state.paste_entries.is_empty());
        assert_eq!(state.input.text, "ab");
    }

    #[test]
    fn delete_at_sentinel_removes_entry() {
        let mut state = test_state();
        state.insert_input_str("ab");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);

        state.input.cursor = 2;
        state.resync_byte_cursor();
        assert_eq!(state.paste_entries.len(), 1);

        state.delete_input_char();

        assert!(state.input.text.chars().all(|c| !is_paste_sentinel(c)));
        assert!(state.paste_entries.is_empty());
        assert_eq!(state.input.text, "ab");
    }

    #[test]
    fn select_all_delete_clears_paste_entries() {
        let mut state = test_state();
        state.insert_input_str("hello ");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.insert_input_str(" world");

        assert_eq!(state.paste_entries.len(), 1);

        state.select_all_input();
        state.record_input_undo_snapshot();
        state.delete_selection_range();

        assert!(state.input.text.is_empty());
        assert!(state.paste_entries.is_empty());
    }

    #[test]
    fn selected_input_text_string_expands_sentinel() {
        let mut state = test_state();
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.select_all_input();
        let selected = state.selected_input_text_string().unwrap();
        assert_eq!(selected, big);
        assert!(!selected.chars().any(is_paste_sentinel));
    }

    #[test]
    fn submit_path_expand_paste_sentinels() {
        let mut state = test_state();
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        let raw = state.input.text.clone();
        let expanded = state.expand_paste_sentinels(&raw);
        state.reset_input();
        assert_eq!(expanded, big);
        assert!(state.paste_entries.is_empty());
    }

    #[test]
    fn ctrl_w_on_sentinel_deletes_it_and_entry() {
        let mut state = test_state();
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);

        assert_eq!(state.paste_entries.len(), 1);

        state.word_backspace();

        assert!(state.input.text.is_empty());
        assert!(state.paste_entries.is_empty());
    }

    #[test]
    fn move_right_skips_over_paste_sentinel() {
        let mut state = test_state();
        state.insert_input_str("ab");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.insert_input_str("cd");

        state.input.cursor = 1;
        state.resync_byte_cursor();

        state.move_input_cursor_right();

        assert_eq!(state.input.cursor, 3);
    }

    #[test]
    fn move_left_skips_over_paste_sentinel() {
        let mut state = test_state();
        state.insert_input_str("ab");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.insert_input_str("cd");

        state.input.cursor = 3;
        state.resync_byte_cursor();

        state.move_input_cursor_left();

        assert_eq!(state.input.cursor, 1);
    }

    #[test]
    fn set_input_cursor_line_col_by_char_snaps_past_sentinel() {
        let mut state = test_state();
        state.insert_input_str("ab");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.insert_input_str("cd");

        state.set_input_cursor_line_col_by_char(2);

        assert_eq!(state.input.cursor, 3);
    }

    fn count_visual_lines_with_caret_baseline(
        text: &str,
        safe_width: usize,
        caret_char_idx: usize,
    ) -> (usize, usize) {
        let mut total = 0usize;
        let mut caret_line = 0usize;
        let mut char_cursor = 0usize;
        let mut caret_found = false;
        let mut parts = text.split('\n').peekable();
        let mut logical_idx = 0usize;

        while let Some(part) = parts.next() {
            let has_more = parts.peek().is_some();
            let first_cap = safe_width.saturating_sub(if logical_idx == 0 { 2 } else { 0 });

            if part.is_empty() {
                if !caret_found && char_cursor == caret_char_idx {
                    caret_line = total;
                    caret_found = true;
                }
                total += 1;
            } else {
                let mut col = 0usize;
                let mut is_first_chunk = true;
                let mut has_chars = false;

                for c in part.chars() {
                    if !caret_found && char_cursor == caret_char_idx {
                        caret_line = total;
                        caret_found = true;
                    }

                    let target = if is_first_chunk {
                        first_cap
                    } else {
                        safe_width
                    };
                    let cw = super::char_display_width(c);

                    if has_chars && col + cw > target {
                        total += 1;
                        col = 0;
                        is_first_chunk = false;
                    }

                    col += cw;
                    char_cursor += 1;
                    has_chars = true;

                    if col >= target {
                        total += 1;
                        col = 0;
                        is_first_chunk = false;
                        has_chars = false;
                    }
                }

                if !caret_found && char_cursor == caret_char_idx {
                    caret_line = total;
                    caret_found = true;
                }

                if has_chars || char_cursor == caret_char_idx {
                    total += 1;
                }
            }

            if has_more {
                char_cursor += 1;
            }

            logical_idx += 1;
        }

        if !caret_found {
            caret_line = total.saturating_sub(1);
        }

        (total.max(1), caret_line)
    }

    #[test]
    fn count_visual_lines_with_caret_matches_baseline_for_ascii_wraps() {
        let mut state = ReplTuiState::new();
        let safe_width = 8;
        state.input.text = "abcdefghijk\nlmnopqrstuv\nxyz\n123456\nabcdefghi".to_string();
        let caret_limit = state.input.text.chars().count();

        for caret in 0..=caret_limit {
            let expected =
                count_visual_lines_with_caret_baseline(&state.input.text, safe_width, caret);
            let actual = state.count_visual_lines_with_caret(safe_width, caret);

            assert_eq!(actual, expected, "caret {caret}");
        }
    }

    // ── paste-newline suppression e2e tests ───────────────────────────────────

    #[test]
    fn arm_paste_enter_suppression_opens_suppression_window() {
        let mut s = test_state();
        assert!(s.selection.suppress_paste_until.is_none());
        s.arm_paste_enter_suppression();
        assert!(
            s.selection.suppress_paste_until.is_some(),
            "arming must open the suppression window"
        );
        assert!(
            s.paste_enter_is_suppressed(KeyCode::Enter),
            "Enter must be suppressed once the window is armed"
        );
    }

    #[test]
    fn handle_paste_event_does_not_arm_suppression() {
        // Suppression is armed by the bracketed-paste / Ctrl+V call sites only,
        // never by handle_paste_event itself.  This guarantees the burst-flush
        // path (which also goes through handle_paste_event) doesn't accidentally
        // suppress subsequent paste keystrokes.
        let mut s = test_state();
        s.handle_paste_event("line1\nline2");
        assert!(
            s.selection.suppress_paste_until.is_none(),
            "handle_paste_event must not arm suppression on its own"
        );
    }

    #[test]
    fn suppression_blocks_enter_but_not_after_window_expires() {
        let mut s = test_state();
        // Open a suppression window that expired 1 ms ago.
        s.selection.suppress_paste_until =
            std::time::Instant::now().checked_sub(std::time::Duration::from_millis(1));
        assert!(
            !s.paste_enter_is_suppressed(KeyCode::Enter),
            "Enter must not be suppressed once the window has expired"
        );
    }

    #[test]
    fn suppression_covers_enter_tab_backspace_and_chars_not_other_keys() {
        let mut s = test_state();
        s.arm_paste_enter_suppression();
        assert!(s.paste_enter_is_suppressed(KeyCode::Enter));
        assert!(s.paste_enter_is_suppressed(KeyCode::Tab));
        assert!(s.paste_enter_is_suppressed(KeyCode::Backspace));
        assert!(s.paste_enter_is_suppressed(KeyCode::Char('a')));
        // Non-suppressed keys — arrow keys and Esc pass through.
        assert!(!s.paste_enter_is_suppressed(KeyCode::Left));
        assert!(!s.paste_enter_is_suppressed(KeyCode::Esc));
    }

    #[test]
    fn handle_paste_event_inserts_multiline_text_directly() {
        let mut s = test_state();
        s.handle_paste_event("a\nb");
        assert_eq!(s.input.text, "a\nb");
        assert!(s.input.text.contains('\n'), "newline must appear in input");
    }

    #[test]
    fn paste_burst_helpers_buffer_then_flush_through_handle_paste_event() {
        let mut s = test_state();
        for ch in "a".repeat(99).chars() {
            s.paste_burst_chars.push(ch);
        }
        s.paste_burst_chars.push('\n');
        for ch in "b".repeat(100).chars() {
            s.paste_burst_chars.push(ch);
        }
        assert_eq!(s.paste_burst_chars.len(), 200);
        s.flush_paste_burst();
        assert!(s.paste_burst_chars.is_empty());
        assert_eq!(s.input.text.len(), 200);
        assert!(s.input.text.contains('\n'));
        assert!(
            s.selection.suppress_paste_until.is_none(),
            "burst flush must not arm post-paste suppression"
        );
    }

    /// End-to-end simulation of the event loop's paste-burst handling for the
    /// concrete regression we're fixing: a multi-line paste arriving as raw
    /// keystrokes (Windows Terminal + `ConPTY` bypassing `Event::Paste`).
    ///
    /// This walks through the exact sequence of state mutations the event-loop
    /// handlers (`KeyCode::Char(c)`, `KeyCode::Enter`) would perform on each
    /// arriving key event, then asserts the final input state contains the
    /// full multi-line text and that no auto-submit was triggered.
    #[test]
    fn paste_burst_e2e_multi_line_keystroke_paste_does_not_auto_send() {
        let mut s = test_state();
        let now = std::time::Instant::now();
        let burst = std::time::Duration::from_millis(2);

        // Char 'a' — first keystroke, no prior key → inserted directly.
        s.last_key_time = Some(now);
        s.insert_input_char('a');

        // Char 'b' arrives 2 ms later — in burst, accumulate.
        let t_b = now + burst;
        assert!(s.in_paste_burst(t_b));
        s.paste_burst_chars.push('b');
        s.last_key_time = Some(t_b);

        // Enter arrives 2 ms later — in burst → push '\n' to buffer, NO submit.
        // (This is the regression: previously this triggered a send of "ab".)
        let t_enter = t_b + burst;
        assert!(s.in_paste_burst(t_enter));
        s.paste_burst_chars.push('\n');
        s.last_key_time = Some(t_enter);

        // Chars 'c', 'd' — still in burst, accumulate.
        let t_c = t_enter + burst;
        s.paste_burst_chars.push('c');
        s.last_key_time = Some(t_c);
        let t_d = t_c + burst;
        s.paste_burst_chars.push('d');
        s.last_key_time = Some(t_d);

        // Burst goes idle — top-of-loop auto-flush fires.
        s.flush_paste_burst();

        // Full multi-line text now in input.text; no auto-send happened.
        assert_eq!(
            s.input.text, "ab\ncd",
            "all pasted content should land in input.text, with the newline preserved"
        );
        assert!(
            s.paste_burst_chars.is_empty(),
            "burst buffer drained after flush"
        );
        // CRITICAL: post-paste suppression must NOT be armed by the burst flush.
        // If it were, the 100 ms window would eat subsequent paste characters,
        // truncating long multi-line pastes (the bug this regression covers).
        assert!(
            s.selection.suppress_paste_until.is_none(),
            "burst flush must not arm Enter suppression"
        );
    }

    /// Regression test for paste-truncation bug: when a slow render cycle
    /// causes the top-of-loop auto-flush to fire MID-paste, the flush must
    /// not arm `suppress_paste_until` — otherwise the 100 ms window eats the
    /// remaining paste characters and the user sees a truncated input.
    ///
    /// Concrete symptom that motivated this test: pasting a ~300-byte Rust
    /// test function only showed the first ~26 characters in the input bar.
    #[test]
    fn paste_burst_mid_paste_flush_does_not_eat_subsequent_keystrokes() {
        let mut s = test_state();
        let t0 = std::time::Instant::now();

        // First "half" of the paste accumulates in the burst buffer.
        for ch in "    #[test]\n    fn render_".chars() {
            s.paste_burst_chars.push(ch);
        }
        s.last_key_time = Some(t0);

        // Simulate the top-of-loop auto-flush firing because a slow render
        // cycle pushed last_key_time past the burst threshold.
        // (In real life this happens when a draw cycle blocks > 30 ms.)
        s.flush_paste_burst();

        // The flushed text is now in input.text.  Buffer drained.
        assert!(s.input.text.starts_with("    #[test]\n    fn render_"));
        assert!(s.paste_burst_chars.is_empty());

        // CRITICAL: suppression window must NOT be armed.  The next
        // paste-burst keystroke (the 't' from "tool_call_...") MUST NOT
        // be suppressed.
        assert!(
            s.selection.suppress_paste_until.is_none(),
            "mid-paste flush must not arm suppression — that's the bug"
        );
        assert!(
            !s.paste_enter_is_suppressed(KeyCode::Char('t')),
            "subsequent paste chars must not be eaten by a suppression window"
        );
        assert!(
            !s.paste_enter_is_suppressed(KeyCode::Enter),
            "subsequent Enter (handled by burst path) must not be suppressed by handle_paste_event"
        );
    }

    /// E2E: the periodic auto-flush condition.  Verifies the predicate the
    /// event loop uses at the top of each tick — burst is flushed only when
    /// both the buffer is non-empty AND the last key is older than the threshold.
    #[test]
    fn paste_burst_e2e_auto_flush_condition_only_fires_when_idle() {
        let mut s = test_state();
        let now = std::time::Instant::now();
        s.paste_burst_chars.push('x');

        // Recent key → don't flush yet (burst may continue).
        s.last_key_time = now.checked_sub(std::time::Duration::from_millis(5));
        let should_flush_recent = !s.paste_burst_chars.is_empty()
            && s.last_key_time.is_some_and(|t| {
                t.elapsed()
                    > std::time::Duration::from_millis(
                        super::ReplTuiState::PASTE_BURST_THRESHOLD_MS,
                    )
            });
        assert!(
            !should_flush_recent,
            "must not flush while burst is still active"
        );

        // Idle past threshold → flush.
        s.last_key_time = now.checked_sub(std::time::Duration::from_millis(100));
        let should_flush_idle = !s.paste_burst_chars.is_empty()
            && s.last_key_time.is_some_and(|t| {
                t.elapsed()
                    > std::time::Duration::from_millis(
                        super::ReplTuiState::PASTE_BURST_THRESHOLD_MS,
                    )
            });
        assert!(should_flush_idle, "must flush once the burst has gone idle");
    }

    #[test]
    fn paste_burst_e2e_windows_ctrl_v_streamed_chars_mask_large_paste() {
        let mut s = test_state();
        let now = Instant::now();
        let pasted = "x".repeat(2048);

        simulate_windows_ctrl_v(&mut s, now);

        let first_char_at = now + Duration::from_millis(100);
        simulate_streamed_char(&mut s, 'x', first_char_at);
        assert!(
            s.input.text.is_empty(),
            "the first streamed char after Ctrl+V must be captured into the burst, not inserted raw"
        );

        let mut at = first_char_at;
        for ch in pasted.chars().skip(1) {
            at += Duration::from_millis(2);
            simulate_streamed_char(&mut s, ch, at);
        }

        simulate_idle_burst_flush(&mut s, at + Duration::from_millis(100));

        assert!(s.paste_burst_chars.is_empty(), "idle flush should drain the burst");
        assert_eq!(s.paste_entries.len(), 1, "large streamed paste should be masked");
        assert_eq!(s.input.text.chars().count(), 1, "masked paste should insert one sentinel");
        assert!(s.input.text.chars().all(is_paste_sentinel));
        assert_eq!(s.expand_paste_sentinels(&s.input.text), pasted);
        assert!(
            !s.force_next_paste_burst_key,
            "forced-capture flag should clear after the first streamed key"
        );
    }

    #[test]
    fn paste_burst_flush_below_threshold_inserts_raw_without_arming_suppression() {
        // Short burst with newline → not masked, and suppression is NOT armed
        // (burst path manages its own newlines via the Enter-in-burst handler).
        let mut s = test_state();
        for ch in "ab\ncd".chars() {
            s.paste_burst_chars.push(ch);
        }
        s.flush_paste_burst();
        assert!(s.paste_burst_chars.is_empty());
        assert!(!s.input.text.contains("[#1 Pasted"));
        assert_eq!(s.input.text, "ab\ncd");
        assert!(
            s.selection.suppress_paste_until.is_none(),
            "burst flush must not arm post-paste suppression"
        );
    }

    #[test]
    fn flush_paste_burst_is_a_noop_when_buffer_is_empty() {
        let mut s = test_state();
        s.input.text = "hello".to_string();
        s.input.cursor = 5;
        s.input.byte_cursor = 5;
        s.flush_paste_burst();
        assert_eq!(
            s.input.text, "hello",
            "empty buffer flush must not modify text"
        );
    }

    #[test]
    fn in_paste_burst_respects_threshold() {
        let mut s = test_state();
        let now = std::time::Instant::now();
        // No previous key recorded → not in burst.
        assert!(!s.in_paste_burst(now));
        // Previous key within threshold → in burst.
        s.last_key_time = now.checked_sub(std::time::Duration::from_millis(10));
        assert!(s.in_paste_burst(now));
        // Previous key beyond threshold → not in burst.
        s.last_key_time = now.checked_sub(std::time::Duration::from_millis(100));
        assert!(!s.in_paste_burst(now));
    }

    #[test]
    fn crlf_paste_normalises_to_lf_in_input() {
        // normalize_pasted_text converts \r\n → \n so the input always stores LF.
        let mut s = test_state();
        s.handle_paste_event("line1\r\nline2");
        assert!(
            s.input.text.contains('\n'),
            "CRLF should be normalised to LF in input.text"
        );
        assert!(
            !s.input.text.contains('\r'),
            "raw \\r should not survive normalisation"
        );
    }

    fn assert_matching_lengths(items: &[ratatui::widgets::ListItem<'static>], text: &[String]) {
        assert_eq!(items.len(), text.len());
    }

    fn selected_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .filter(|span| span.style.bg == Some(Color::DarkGray))
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn auth_command_blocked_when_busy() {
        let (tx, _rx) = mpsc::channel();
        let modal = AuthModal::new(tx.clone(), None);
        assert!(matches!(modal.step, AuthModalStep::ProviderSelect { .. }));
    }

    #[test]
    fn auth_command_with_provider_arg_skips_selection() {
        let (tx, _rx) = mpsc::channel();
        let modal = AuthModal::new(tx.clone(), Some(Provider::OpenAi));
        assert!(matches!(
            modal.step,
            AuthModalStep::ApiKeyInput {
                provider: ProviderKind::OpenAi,
                ..
            }
        ));
        let modal2 = AuthModal::new(tx, Some(Provider::Anthropic));
        assert!(matches!(
            modal2.step,
            AuthModalStep::OAuthWaiting {
                provider: ProviderKind::Anthropic,
                ..
            }
        ));
    }

    #[test]
    fn render_tool_call_running_status() {
        let (items, text) = render_tool_call_lines(
            "bash",
            "echo hello",
            &ToolCallStatus::Running,
            80,
            '⠋',
            false,
        );
        assert_eq!(items.len(), 1);
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_success_empty_output() {
        let (items, text) = render_tool_call_lines(
            "navigate",
            "https://example.com",
            &ToolCallStatus::Success {
                output: String::new(),
            },
            80,
            '⠋',
            false,
        );
        assert_eq!(items.len(), 1);
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_success_with_output() {
        let (items, text) = render_tool_call_lines(
            "bash",
            "ls -la",
            &ToolCallStatus::Success {
                output: "some result".to_string(),
            },
            80,
            '⠋',
            false,
        );
        assert_eq!(items.len(), 1);
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_error_status() {
        let (items, text) = render_tool_call_lines(
            "bash",
            "bad command",
            &ToolCallStatus::Error("timeout after 30s".to_string()),
            80,
            '⠋',
            false,
        );
        assert!(items.len() >= 2);
        let plain = text.join(" ");
        assert!(plain.contains("bash"));
        assert!(plain.contains("timeout after 30s"));
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_input_truncation() {
        let long_input = "a".repeat(80);
        let (items, text) = render_tool_call_lines(
            "bash",
            &long_input,
            &ToolCallStatus::Running,
            80,
            '⠋',
            false,
        );
        assert_eq!(items.len(), 1);
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_bash_rich_stdout() {
        let output = serde_json::json!({
            "stdout": "line1\nline2\nline3",
            "stderr": ""
        })
        .to_string();
        let (items, text) = render_tool_call_lines(
            "bash",
            r#"{"command":"ls -la"}"#,
            &ToolCallStatus::Success { output },
            80,
            '⠋',
            false,
        );
        assert!(
            items.len() >= 2,
            "Expected header + stdout lines, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_bash_with_stderr() {
        let output = serde_json::json!({
            "stdout": "",
            "stderr": "error line"
        })
        .to_string();
        let (items, text) = render_tool_call_lines(
            "bash",
            "cmd",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
            false,
        );
        assert!(
            items.len() >= 2,
            "Expected header + stderr line, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_unknown_tool_single_line() {
        let output = "navigation complete".to_string();
        let (items, text) = render_tool_call_lines(
            "navigate",
            "https://example.com",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
            false,
        );
        assert_eq!(
            items.len(),
            1,
            "Unknown tool should produce exactly 1 line, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_bash_overflow_truncated() {
        let stdout = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = serde_json::json!({ "stdout": stdout, "stderr": "" }).to_string();
        let (items, text) = render_tool_call_lines(
            "bash",
            "cmd",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
            false,
        );
        assert_eq!(
            items.len(),
            17,
            "Expected 1 header + 15 lines + 1 overflow = 17, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn wrap_ansi_line_respects_wide_character_width() {
        let wrapped = wrap_ansi_line(Line::from("ab中cd中efg"), 10);
        let plain = wrapped.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert_eq!(plain, vec!["ab中cd中ef".to_string(), "g".to_string()]);
    }

    #[test]
    fn tool_input_summary_extracts_key_fields() {
        assert_eq!(tool_input_summary("bash", r#"{"command":"ls"}"#), "ls");
        assert_eq!(
            tool_input_summary("navigate", r#"{"url":"https://example.com"}"#),
            "https://example.com"
        );
    }

    #[test]
    fn input_cursor_uses_display_width_for_wide_chars() {
        let mut state = ReplTuiState::new();
        state.input.text = "a中\nbc".to_string();
        state.input.cursor = 2;

        assert_eq!(state.input_cursor_line_col(), (0, 3));

        state.move_input_cursor_down();
        assert_eq!(state.input_cursor_line_col(), (1, 2));
        assert_eq!(state.input.cursor, 5);
    }

    #[test]
    fn calculate_input_dimensions_places_cursor_after_wide_char() {
        let mut state = ReplTuiState::new();
        state.input.text = "中a".to_string();
        state.input.cursor = 1;

        let (_, _, _, cursor_pos) = state.calculate_input_dimensions(20, "model");
        let prompt_width = u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX);

        assert_eq!(cursor_pos, Some((1, prompt_width + 2)));
    }

    #[test]
    fn calculate_input_dimensions_limits_large_paste_to_viewport_lines() {
        let mut state = ReplTuiState::new();
        state.input.text = (0..1000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.input.cursor = state.input.text.chars().count();
        state.resync_byte_cursor();

        let (_, render_lines, max_scroll, cursor_pos) =
            state.calculate_input_dimensions(20, "model");

        assert!(render_lines.len() <= super::MAX_INPUT_LINES + 3);
        assert!(max_scroll > 0);
        assert!(cursor_pos.is_some());
    }

    #[test]
    fn calculate_input_dimensions_renders_pill_for_masked_paste() {
        let mut state = test_state();
        state.input_area_width = 80;
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);

        let (_, render_lines, _, _) = state.calculate_input_dimensions(80, "Model");

        let has_pill = render_lines
            .iter()
            .any(|line| line.spans.iter().any(|span| span.content.contains('📋')));
        assert!(has_pill, "Expected pill emoji in rendered output");
    }

    #[test]
    fn select_all_input_marks_entire_buffer() {
        let mut state = ReplTuiState::new();
        state.input.text = "ab\ncd".to_string();
        state.input.cursor = 1;
        state.resync_byte_cursor();

        state.select_all_input();

        assert_eq!(state.input_selection, Some((0, 5)));
        assert_eq!(state.input.cursor, 5);
    }

    #[test]
    fn copy_selection_yanks_raw_content() {
        let mut s = test_state();
        s.insert_input_str("a zzz b");

        // Select the whole input.
        let total = s.input.text.chars().count();
        s.input_selection = Some((0, total));

        let yanked = s.selected_input_text_string().unwrap();
        assert_eq!(yanked, "a zzz b");
    }

    #[test]
    fn cut_input_selection_text_returns_text_and_removes_it() {
        let mut state = ReplTuiState::new();
        state.input.text = "ab\ncd".to_string();
        state.input.cursor = 5;
        state.resync_byte_cursor();
        state.input_selection = Some((0, 3));

        let cut = state.cut_input_selection_text();

        assert_eq!(cut.as_deref(), Some("ab\n"));
        assert_eq!(state.input.text, "cd");
        assert_eq!(state.input.cursor, 0);
        assert_eq!(state.input_selection, None);
    }

    #[test]
    fn undo_redo_input_insert_round_trip() {
        let mut state = ReplTuiState::new();

        state.insert_input_str("hello");
        assert_eq!(state.input.text, "hello");

        assert!(state.undo_input_edit());
        assert_eq!(state.input.text, "");
        assert_eq!(state.input.cursor, 0);

        assert!(state.redo_input_edit());
        assert_eq!(state.input.text, "hello");
        assert_eq!(state.input.cursor, 5);
    }

    #[test]
    fn undo_redo_restores_cut_input_selection() {
        let mut state = ReplTuiState::new();
        state.input.text = "ab\ncd".to_string();
        state.input.cursor = 5;
        state.resync_byte_cursor();
        state.input_selection = Some((0, 3));

        let cut = state.cut_input_selection_text();

        assert_eq!(cut.as_deref(), Some("ab\n"));
        assert!(state.undo_input_edit());
        assert_eq!(state.input.text, "ab\ncd");
        assert_eq!(state.input_selection, Some((0, 3)));

        assert!(state.redo_input_edit());
        assert_eq!(state.input.text, "cd");
        assert_eq!(state.input.cursor, 0);
    }

    #[test]
    fn input_selection_preserves_newline_char_offsets_across_paragraphs() {
        let mut state = ReplTuiState::new();
        state.input.text = "ab\ncd\nef".to_string();
        state.input.cursor = state.input.text.chars().count();
        state.input_selection = Some((3, 7));

        let (_, render_lines, _, _) = state.calculate_input_dimensions(20, "model");

        assert_eq!(selected_text(&render_lines[2]), "cd");
        assert_eq!(selected_text(&render_lines[3]), "e");
    }

    #[test]
    fn tool_call_start_flushes_typewriter_first() {
        let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
        let mut state = ReplTuiState::new();

        for c in "hello\n".chars() {
            state.typewriter.chars.push_back(c);
        }

        tx.send(ReplTuiEvent::ToolCallStart {
            name: "bash".to_string(),
            input: r#"{"command":"ls"}"#.to_string(),
        })
        .unwrap();
        state.drain_events(&rx);

        assert!(state.entries.len() >= 2);
        assert!(matches!(state.entries[0], TranscriptEntry::Stream(_)));
        assert!(matches!(
            state.entries[1],
            TranscriptEntry::ToolCall {
                status: ToolCallStatus::Running,
                ..
            }
        ));
    }

    #[test]
    fn tool_call_complete_updates_in_place_success() {
        let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
        let mut state = ReplTuiState::new();

        state.entries.push(TranscriptEntry::ToolCall {
            name: "bash".to_string(),
            input_summary: "ls".to_string(),
            status: ToolCallStatus::Running,
        });

        tx.send(ReplTuiEvent::ToolCallComplete {
            name: "bash".to_string(),
            output: "file.txt".to_string(),
            is_error: false,
        })
        .unwrap();
        state.drain_events(&rx);

        assert_eq!(state.entries.len(), 1);
        assert!(matches!(
            state.entries[0],
            TranscriptEntry::ToolCall {
                status: ToolCallStatus::Success { .. },
                ..
            }
        ));
    }

    #[test]
    fn tool_call_complete_updates_in_place_error() {
        let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
        let mut state = ReplTuiState::new();

        state.entries.push(TranscriptEntry::ToolCall {
            name: "bash".to_string(),
            input_summary: "bad cmd".to_string(),
            status: ToolCallStatus::Running,
        });

        tx.send(ReplTuiEvent::ToolCallComplete {
            name: "bash".to_string(),
            output: "command not found".to_string(),
            is_error: true,
        })
        .unwrap();
        state.drain_events(&rx);

        assert_eq!(state.entries.len(), 1);
        assert!(matches!(
            state.entries[0],
            TranscriptEntry::ToolCall {
                status: ToolCallStatus::Error(_),
                ..
            }
        ));
    }

    #[test]
    fn goal_title_shows_reasoning_effort_for_reasoning_model() {
        let header = super::HeaderSnapshot {
            model: "gpt-5.3-codex".to_string(),
            reasoning_effort: Some("high".to_string()),
            ..Default::default()
        };
        let mut state = ReplTuiState::new();
        state.cached_header = header;

        let title = if let Some(ref effort) = state.cached_header.reasoning_effort {
            format!(" Goal · {} · {effort} ", state.cached_header.model)
        } else {
            format!(" Goal · {} ", state.cached_header.model)
        };
        assert_eq!(title, " Goal · gpt-5.3-codex · high ");
    }

    #[test]
    fn goal_title_omits_effort_for_non_reasoning_model() {
        let header = super::HeaderSnapshot {
            model: "claude-sonnet-4-6".to_string(),
            reasoning_effort: None,
            ..Default::default()
        };
        let mut state = ReplTuiState::new();
        state.cached_header = header;

        let title = if let Some(ref effort) = state.cached_header.reasoning_effort {
            format!(" Goal · {} · {effort} ", state.cached_header.model)
        } else {
            format!(" Goal · {} ", state.cached_header.model)
        };
        assert_eq!(title, " Goal · claude-sonnet-4-6 ");
    }

    #[test]
    fn goal_title_cycles_through_all_effort_levels() {
        let efforts = ["none", "minimal", "low", "medium", "high", "xhigh"];
        for effort in &efforts {
            let header = super::HeaderSnapshot {
                model: "gpt-5.3-codex".to_string(),
                reasoning_effort: Some(effort.to_string()),
                ..Default::default()
            };
            let title = format!(" Goal · {} · {} ", header.model, effort);
            assert!(
                title.contains(&format!("· {effort} ")),
                "title should contain effort level '{effort}': {title}"
            );
        }
    }

    #[test]
    fn test_welcome_card_renders_when_outdated() {
        let mut state = super::ReplTuiState::new();
        state.update_info = Some(runtime::update_check::UpdateInfo {
            latest_version: "9.9.9".to_string(),
            current_version: "1.0.0".to_string(),
            is_outdated: true,
        });

        let backend = ratatui::backend::TestBackend::new(100, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                crate::tui::repl_render::draw_welcome(frame, frame.area(), &mut state, false);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let content = (0..40)
            .map(|y| {
                (0..100)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(content.contains("Update available: v9.9.9 (you have v1.0.0)"));
    }

    #[test]
    fn test_no_card_when_current() {
        let mut state = super::ReplTuiState::new();
        state.update_info = None;

        let backend = ratatui::backend::TestBackend::new(100, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                crate::tui::repl_render::draw_welcome(frame, frame.area(), &mut state, false);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let content = (0..40)
            .map(|y| {
                (0..100)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!content.contains("Update available"));
    }

    // ── paste masking v2: edge-case integration tests ─────────────────────────

    #[test]
    fn paste_only_newlines_exceeding_30_is_masked() {
        let mut state = test_state();
        // 31 newlines = 31 bytes but ≥30 lines → should be masked
        let thirty_one_newlines = "\n".repeat(31);
        state.handle_paste_event(&thirty_one_newlines);
        assert_eq!(state.paste_entries.len(), 1);
        assert_eq!(state.input.text.chars().count(), 1);
        let c = state.input.text.chars().next().unwrap();
        assert!(is_paste_sentinel(c));
    }

    #[test]
    fn adjacent_masks_create_two_entries_and_two_sentinels() {
        let mut state = test_state();
        let big1 = "a".repeat(2048);
        let big2 = "b".repeat(2048);
        state.handle_paste_event(&big1);
        state.handle_paste_event(&big2);
        assert_eq!(state.paste_entries.len(), 2);
        assert_eq!(state.input.text.chars().count(), 2);
        // Both chars in input.text should be sentinels
        let chars: Vec<char> = state.input.text.chars().collect();
        assert!(is_paste_sentinel(chars[0]));
        assert!(is_paste_sentinel(chars[1]));
        // They should have distinct ids
        assert_ne!(sentinel_to_id(chars[0]), sentinel_to_id(chars[1]));
        // Content preserved
        assert_eq!(state.paste_entries[0].content, big1);
        assert_eq!(state.paste_entries[1].content, big2);
    }

    #[test]
    fn ctrl_a_then_type_clears_all_entries() {
        let mut state = test_state();
        state.insert_input_str("prefix ");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.insert_input_str(" suffix");
        assert_eq!(state.paste_entries.len(), 1);
        // Ctrl-A (select all) then backspace replaces all, then type a char
        state.select_all_input();
        state.backspace_input_char();
        state.insert_input_char('z');
        assert!(
            state.paste_entries.is_empty(),
            "entries should be cleared after select-all+delete"
        );
        assert_eq!(state.input.text, "z");
    }

    #[test]
    fn burst_flush_above_threshold_creates_mask() {
        let mut state = test_state();
        // Simulate a paste burst: push 2048 'x' chars then flush
        let chars: Vec<char> = "x".repeat(2048).chars().collect();
        state.paste_burst_chars.extend(chars);
        state.flush_paste_burst();
        assert_eq!(
            state.paste_entries.len(),
            1,
            "burst flush above threshold should create mask"
        );
        assert_eq!(state.input.text.chars().count(), 1);
        assert!(is_paste_sentinel(state.input.text.chars().next().unwrap()));
    }

    #[test]
    fn mixed_content_cursor_navigates_around_sentinel() {
        let mut state = test_state();
        state.insert_input_str("hello ");
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        state.insert_input_str(" world");
        // text: "hello " + sentinel + " world" = 6 + 1 + 6 = 13 chars
        // cursor is at position 13 (after 'd')
        // Move left 6 times to get to position 7 (space before 'world')
        for _ in 0..6 {
            state.move_input_cursor_left();
        }
        assert_eq!(state.input.cursor, 7);
        // Move left one more: should skip the sentinel at position 6, land at 5
        state.move_input_cursor_left();
        assert_eq!(
            state.input.cursor, 5,
            "cursor should have jumped over sentinel from position 7 to 5"
        );
    }

    #[test]
    fn input_text_never_contains_pill_text() {
        let mut state = test_state();
        let big = "x".repeat(2048);
        state.handle_paste_event(&big);
        // input.text should only contain the sentinel char, not any human-readable text
        let text = &state.input.text;
        assert!(
            !text.contains('\u{1F4CB}'),
            "input.text should not contain pill emoji"
        );
        assert!(
            !text.contains("lines"),
            "input.text should not contain 'lines'"
        );
        assert!(!text.contains('['), "input.text should not contain '['");
        // Should only have the single sentinel char
        assert_eq!(text.chars().count(), 1);
        assert!(text.chars().all(is_paste_sentinel));
    }
}
