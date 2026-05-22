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
    /// Active text selection within the input field: `(anchor_char, end_char)`
    /// where anchor ≤ end.  `None` when no selection is active.
    pub(super) input_selection: Option<(usize, usize)>,
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
    /// Accumulates characters arriving faster than a human can type (≤30 ms
    /// apart) so they can be flushed as a single `insert_input_str` call instead
    /// of being inserted one-by-one.  `None` = not currently in a burst.
    paste_buffer: Option<(Instant, Vec<char>)>,
    /// Timestamp of the most-recently processed `KeyCode::Char` (or `Enter`
    /// treated as a paste newline).  Used together with `paste_buffer` above to
    /// detect burst boundaries.
    last_key_time: Option<Instant>,
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
            input_area_width: 0,
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
            selection: SelectionState::default(),
            paste_buffer: None,
            last_key_time: None,
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

    /// Maximum gap (ms) between consecutive keystrokes to be considered a
    /// paste burst.  Human typing at 120 WPM averages ≈100 ms/char, so 30 ms
    /// cleanly separates programmatic paste from even the fastest typists.
    const PASTE_THRESHOLD_MS: u64 = 30;

    /// If a paste burst is active, flush the accumulated characters into the
    /// input buffer by concatenating prefix + `pasted_text` + suffix directly
    /// (avoids the O(n) `insert_str` tail-shift and `text.chars().count()`).
    fn flush_paste_buffer(&mut self) {
        self.input_scroll_manual = false;
        if let Some((_, chars)) = self.paste_buffer.take() {
            let n = chars.len();
            if n == 0 {
                return;
            }
            self.clamp_input_cursor();
            let prefix = &self.input.text[..self.input.byte_cursor];
            let suffix = &self.input.text[self.input.byte_cursor..];
            // Compute UTF-8 byte length of pasted chars while building the
            // replacement string — one pass instead of two.
            let mut pasted_len = 0usize;
            let cap = prefix.len() + suffix.len() + n; // n ≤ actual UTF-8 bytes
            let mut text = String::with_capacity(cap);
            text.push_str(prefix);
            for c in chars {
                pasted_len += c.len_utf8();
                text.push(c);
            }
            text.push_str(suffix);
            self.input.byte_cursor = self.input.byte_cursor.saturating_add(pasted_len);
            self.input.cursor = self.input.cursor.saturating_add(n);
            self.input.text = text;
            self.input.preferred_col = None;
            self.input_scroll_offset = usize::MAX;
            self.wake_input_caret();
            self.refresh_slash_overlay();
        }
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
            self.input.cursor = a;
            self.resync_byte_cursor();
            let end_byte = self.input_char_to_byte(b);
            self.input
                .text
                .replace_range(self.input.byte_cursor..end_byte, "");
            self.input.preferred_col = None;
            self.input_scroll_offset = usize::MAX;
            true
        } else {
            false
        }
    }

    fn insert_input_char(&mut self, ch: char) {
        self.input_scroll_manual = false;
        self.delete_selection_range();
        self.clamp_input_cursor();
        self.input.text.insert(self.input.byte_cursor, ch);
        self.input.cursor = self.input.cursor.saturating_add(1);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(ch.len_utf8());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn insert_input_str(&mut self, text: &str) {
        self.input_scroll_manual = false;
        self.delete_selection_range();
        self.clamp_input_cursor();
        self.input.text.insert_str(self.input.byte_cursor, text);
        let char_count = text.chars().count();
        self.input.cursor = self.input.cursor.saturating_add(char_count);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(text.len());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn backspace_input_char(&mut self) {
        self.input_scroll_manual = false;
        // If selection is active, delete it instead of a single char.
        if self.delete_selection_range() {
            return;
        }
        self.clamp_input_cursor();
        if self.input.cursor == 0 {
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
        self.input.text.replace_range(start..end, "");
        self.input.cursor -= 1;
        self.input.byte_cursor = start;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn delete_input_char(&mut self) {
        self.input_scroll_manual = false;
        if self.delete_selection_range() {
            return;
        }
        self.clamp_input_cursor();
        if self.input.cursor >= self.input_char_len() {
            return;
        }
        // Find byte-offset of the character after cursor
        let bytes = self.input.text.as_bytes();
        let mut end = self.input.byte_cursor + 1;
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        self.input
            .text
            .replace_range(self.input.byte_cursor..end, "");
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
    }

    fn move_input_cursor_right(&mut self) {
        self.input_scroll_manual = false;
        self.input_selection = None;
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

    /// Compute visual line boundaries for the input text.
    ///
    /// Returns `Vec<(char_start, display_width)>` — the char-index of each
    /// visual line's first character and its display width in terminal cells.
    /// The first visual line of the first logical line has a 2‑cell offset
    /// (the `"> "` prompt prefix); all others wrap at `safe_width` columns.
    fn visual_line_info(&self, safe_width: usize) -> Vec<(usize, usize)> {
        let mut lines = Vec::new();
        let mut char_idx = 0usize;
        for (logical_idx, logical_line) in self.input.text.split('\n').enumerate() {
            let logical_chars: Vec<char> = logical_line.chars().collect();
            let prompt_offset = if logical_idx == 0 { 2usize } else { 0 };
            let first_cap = safe_width.saturating_sub(prompt_offset);
            let cap = safe_width;
            let mut offset = 0usize;
            loop {
                let remaining = logical_chars.len().saturating_sub(offset);
                if remaining == 0 {
                    if offset == 0 {
                        lines.push((char_idx, 0));
                    }
                    break;
                }
                let w = if offset == 0 { first_cap } else { cap };
                // Walk forward up to `w` display cells
                let mut col = 0usize;
                let mut end = offset;
                while end < logical_chars.len() {
                    let cw = char_display_width(logical_chars[end]);
                    if col + cw > w {
                        break;
                    }
                    col += cw;
                    end += 1;
                }
                lines.push((char_idx + offset, col));
                if end == offset {
                    // Zero-width char at start — force advance
                    end = offset + 1;
                }
                offset = end;
            }
            char_idx += logical_chars.len() + 1; // +1 for the \n separator
        }
        lines
    }

    fn move_input_cursor_up(&mut self) {
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
        // Find which visual line the cursor is on
        let mut cur_vis = 0usize;
        for (i, _) in vis.iter().enumerate() {
            let next_start = vis.get(i + 1).map_or(usize::MAX, |v| v.0);
            if cur_char >= next_start {
                continue;
            }
            cur_vis = i;
            break;
        }
        if cur_vis == 0 {
            let (start, _) = vis[0];
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

        let (prev_start, prev_width) = vis[cur_vis - 1];
        let raw_prev_end = vis[cur_vis].0;
        let prev_end = if raw_prev_end > 0
            && self.input.text.chars().nth(raw_prev_end.saturating_sub(1)) == Some('\n')
        {
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
        let mut cur_vis = 0usize;
        for (i, _) in vis.iter().enumerate() {
            let next_start = vis.get(i + 1).map_or(usize::MAX, |v| v.0);
            if cur_char >= next_start {
                continue;
            }
            cur_vis = i;
            break;
        }
        if cur_vis + 1 >= vis.len() {
            // Already on the last visual line — go to end
            let (start, width) = vis[cur_vis];
            let line_text: String = self
                .input
                .text
                .chars()
                .skip(start)
                .take(usize::MAX)
                .collect();
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

        let (next_start, next_width) = vis[cur_vis + 1];
        let raw_next_end = vis.get(cur_vis + 2).map_or(self.input_char_len(), |v| v.0);
        let next_end = if raw_next_end > 0
            && self.input.text.chars().nth(raw_next_end.saturating_sub(1)) == Some('\n')
        {
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

    /// Set cursor directly by character index (used by visual-line nav).
    fn set_input_cursor_line_col_by_char(&mut self, char_idx: usize) {
        self.input_scroll_manual = false;
        self.input_selection = None;
        self.input.cursor = char_idx.min(self.input_char_len());
        self.resync_byte_cursor();
        self.input_scroll_offset = usize::MAX;
    }

    /// Convert a mouse position (row, col) relative to the input widget into
    /// a character index in `input.text`.  Returns `None` if the position
    /// falls outside the text bounds.
    fn char_index_at_mouse(&self, widget_row: usize, widget_col: usize) -> Option<usize> {
        // The render adds a blank padding line above (row 0) and below
        // the content, so the first content visual line is at row 1.
        if widget_row == 0 {
            return None;
        }
        let content_row = widget_row.saturating_sub(1);
        let w = usize::from(self.input_area_width).max(10);
        let safe_width = w.saturating_sub(5).max(5);
        let vis = self.visual_line_info(safe_width);
        if vis.is_empty() {
            return None;
        }
        let abs_row = self.input_scroll_offset + content_row;
        let &(start, width) = vis.get(abs_row)?;
        // First visual line of the first logical line has a 2‑cell prompt prefix.
        let is_first = abs_row == 0;
        let prompt = if is_first { 2usize } else { 0 };
        if widget_col < prompt {
            return Some(start);
        }
        let target_col = widget_col.saturating_sub(prompt).min(width);
        let line_text: String = self
            .input
            .text
            .chars()
            .skip(start)
            .take(
                vis.get(abs_row + 1)
                    .map_or(self.input_char_len(), |v| v.0)
                    .saturating_sub(start),
            )
            .collect();
        let col_chars = char_count_for_display_col(&line_text, target_col);
        Some(start + col_chars)
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
        let mut input_with_caret = self.input.text.clone();
        if !is_placeholder {
            let caret_idx = self.input_char_to_byte(self.input.cursor);
            input_with_caret.insert(caret_idx, INPUT_CARET_MARKER);
        }
        let source = if is_placeholder {
            placeholder_text.to_owned()
        } else {
            input_with_caret
        };
        let mut lines_data = source
            .split('\n')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if lines_data.is_empty() {
            lines_data.push(String::new());
        }

        let safe_width = width.saturating_sub(5).max(5) as usize;
        let mut visual_lines = Vec::new();
        let mut caret_row_idx = 0usize;
        let mut seen_caret = false;

        let input_char_width = |ch: char| {
            if ch == INPUT_CARET_MARKER {
                0
            } else {
                char_display_width(ch)
            }
        };

        for (logical_idx, line) in lines_data.into_iter().enumerate() {
            let offset = if logical_idx == 0 { 2 } else { 0 };
            let first_line_width = safe_width.saturating_sub(offset);

            if line.is_empty() {
                visual_lines.push((logical_idx == 0, String::new()));
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
                    if !is_placeholder && !seen_caret && current.contains(INPUT_CARET_MARKER) {
                        caret_row_idx = visual_lines.len();
                        seen_caret = true;
                    }
                    visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                    current = String::new();
                    w = 0;
                    is_first_chunk = false;
                }

                current.push(c);
                w += char_width;

                if w >= target && !current.is_empty() {
                    if !is_placeholder && !seen_caret && current.contains(INPUT_CARET_MARKER) {
                        caret_row_idx = visual_lines.len();
                        seen_caret = true;
                    }
                    visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                    current = String::new();
                    w = 0;
                    is_first_chunk = false;
                }
            }
            if !current.is_empty() {
                if !is_placeholder && !seen_caret && current.contains(INPUT_CARET_MARKER) {
                    caret_row_idx = visual_lines.len();
                    seen_caret = true;
                }
                visual_lines.push((logical_idx == 0 && is_first_chunk, current));
            }
        }

        let max_text_lines = MAX_INPUT_LINES;
        let total_visual = visual_lines.len();
        let max_scroll = total_visual.saturating_sub(max_text_lines);
        if self.input_scroll_offset == usize::MAX {
            self.input_scroll_offset = max_scroll;
        } else {
            self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);
        }
        if !is_placeholder && seen_caret && !self.input_scroll_manual {
            if caret_row_idx < self.input_scroll_offset {
                self.input_scroll_offset = caret_row_idx;
            } else if caret_row_idx >= self.input_scroll_offset + max_text_lines {
                self.input_scroll_offset = caret_row_idx.saturating_sub(max_text_lines - 1);
            }
        }
        self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);

        // Compute char-index range for each visual line (excluding the caret
        // marker) for selection highlighting — must be done before consuming
        // visual_lines with into_iter below.
        let visual_ranges: Vec<(usize, usize)> = {
            let mut acc = 0usize;
            let mut ranges = Vec::with_capacity(visual_lines.len());
            for (_, line) in &visual_lines {
                let n = line.chars().filter(|&c| c != INPUT_CARET_MARKER).count();
                ranges.push((acc, acc + n));
                acc += n;
            }
            ranges
        };

        let skip = self.input_scroll_offset;
        let sliced = visual_lines
            .into_iter()
            .skip(skip)
            .take(max_text_lines)
            .collect::<Vec<_>>();
        let total_sliced = sliced.len();
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

        for (i, (has_prompt, row)) in sliced.into_iter().enumerate() {
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
                    let (left_sel, right_sel) = if sel_a < sel_b {
                        let left_end = line_start + left.chars().count();
                        let left_sel = sel_a.max(line_start).min(left_end)
                            < sel_b.max(line_start).min(left_end);
                        let right_start = line_start + left.chars().count();
                        let right_sel = sel_a.max(right_start).min(line_end)
                            < sel_b.max(right_start).min(line_end);
                        (left_sel, right_sel)
                    } else {
                        (false, false)
                    };
                    let prompt_width = if has_prompt {
                        u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                    } else {
                        0
                    };
                    if left.is_empty() {
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
                    } else {
                        let left_width = text_display_width(&left);
                        spans.push(Span::styled(
                            left,
                            if left_sel { sel_style } else { text_style },
                        ));
                        let cursor_col =
                            prompt_width + u16::try_from(left_width).unwrap_or(u16::MAX);
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), cursor_col));
                    }
                    if !right.is_empty() {
                        spans.push(Span::styled(
                            right,
                            if right_sel { sel_style } else { text_style },
                        ));
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
                    if !before.is_empty() {
                        spans.push(Span::styled(before, text_style));
                    }
                    if !selected.is_empty() {
                        spans.push(Span::styled(selected, sel_style));
                    }
                    if !after.is_empty() {
                        spans.push(Span::styled(after, text_style));
                    }
                } else {
                    spans.push(Span::styled(row, text_style));
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
                        spans.push(Span::styled(left, text_style));
                        let cursor_col =
                            prompt_width + u16::try_from(left_width).unwrap_or(u16::MAX);
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), cursor_col));
                    }
                    if !right.is_empty() {
                        spans.push(Span::styled(right, text_style));
                    }
                } else {
                    spans.push(Span::styled(row, text_style));
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

        // Auto-flush paste buffer when the burst has been idle longer than
        // the threshold — handles the case where paste ends without a
        // subsequent key event to trigger the flush.
        if state.paste_buffer.is_some()
            && state.last_key_time.is_some_and(|t| {
                t.elapsed() > Duration::from_millis(ReplTuiState::PASTE_THRESHOLD_MS)
            })
        {
            state.flush_paste_buffer();
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
                                // Start a zero-width selection at the click
                                // position so subsequent Drag events use the
                                // click as the anchor, not the drag position.
                                state.input_selection = Some((idx, idx));
                                state.selection.anchor = None;
                                state.selection.end = None;
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some(idx) = char_idx {
                                let anchor = state.input_selection.map_or(idx, |(a, _)| a.min(idx));
                                let end = anchor.max(idx);
                                state.input_selection = Some((anchor, end));
                                state.selection.anchor = None;
                                state.selection.end = None;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Right)
                            if state.input_selection.is_some() =>
                        {
                            // Copy selected text to clipboard.
                            if let Some((a, b)) = state.input_selection {
                                let sel_start = state.input_char_to_byte(a);
                                let sel_end = state.input_char_to_byte(b);
                                if let Some(text) = state.input.text.get(sel_start..sel_end) {
                                    if let Ok(mut cb) = arboard::Clipboard::new() {
                                        let _ = cb.set_text(text.to_string());
                                    }
                                }
                            }
                            state.input_selection = None;
                        }
                        _ => {
                            // Any other click clears input selection.
                            state.input_selection = None;
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

                // Bracketed paste supersedes any ongoing burst accumulation
                state.paste_buffer = None;
                state.last_key_time = None;
                state.insert_input_str(&normalize_pasted_text(&text));
                state.wake_input_caret();
                state.refresh_slash_overlay();
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let suppress_paste_key = state
                    .selection
                    .suppress_paste_until
                    .is_some_and(|deadline| Instant::now() <= deadline)
                    && matches!(
                        key.code,
                        KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace
                    );
                if suppress_paste_key {
                    state.selection.suppress_paste_until =
                        Some(Instant::now() + Duration::from_millis(150));
                    continue;
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
                    // Manual paste supersedes any ongoing burst accumulation
                    state.paste_buffer = None;
                    state.last_key_time = None;
                    if let Some(text) = read_clipboard_text() {
                        state.insert_input_str(&text);
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    continue;
                }

                // Copy input selection (Ctrl+C / Ctrl+Insert).
                if state.active_modal.is_none()
                    && state.input_selection.is_some()
                    && ((key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                        || (key.code == KeyCode::Insert && key.modifiers == KeyModifiers::CONTROL))
                {
                    if let Some((a, b)) = state.input_selection {
                        let sel_start = state.input_char_to_byte(a);
                        let sel_end = state.input_char_to_byte(b);
                        if let Some(text) = state.input.text.get(sel_start..sel_end) {
                            if let Ok(mut cb) = arboard::Clipboard::new() {
                                let _ = cb.set_text(text.to_string());
                            }
                        }
                    }
                    state.input_selection = None;
                    state.selection.anchor = None;
                    state.selection.end = None;
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
                        state.flush_paste_buffer();
                        state.insert_input_char('\n');
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Enter => {
                        // Paste burst detection: if Enter arrives within the burst
                        // threshold, treat it as a literal \n in the pasted text
                        // instead of submitting.
                        let now = Instant::now();
                        let in_paste_burst = state.last_key_time.is_some()
                            && state.paste_buffer.is_some()
                            || state.last_key_time.is_some_and(|t| {
                                now.duration_since(t)
                                    <= Duration::from_millis(ReplTuiState::PASTE_THRESHOLD_MS)
                            });
                        state.last_key_time = Some(now);
                        if in_paste_burst {
                            state
                                .paste_buffer
                                .get_or_insert_with(|| (now, Vec::new()))
                                .1
                                .push('\n');
                            state.wake_input_caret();
                            continue;
                        }
                        state.flush_paste_buffer();

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
                            state.input.text.clear();
                            state.input.cursor = 0;
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
                                    state.input.text = selected;
                                    state.input.text.push(' ');
                                    state.input.cursor = state.input.text.chars().count();
                                    state.input.preferred_col = None;
                                    state.input_scroll_offset = usize::MAX;
                                    state.wake_input_caret();
                                    state.refresh_slash_overlay();
                                    continue;
                                }
                            }
                        }

                        let line = std::mem::take(&mut state.input.text);
                        state.input.cursor = 0;
                        state.input.preferred_col = None;
                        state.input_scroll_offset = 0;
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
                        state.flush_paste_buffer();
                        state.insert_input_char('\n');
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Char(c) => {
                        let now = Instant::now();
                        let is_fast = state.last_key_time.is_some_and(|t| {
                            now.duration_since(t)
                                <= Duration::from_millis(ReplTuiState::PASTE_THRESHOLD_MS)
                        });
                        state.last_key_time = Some(now);

                        if is_fast {
                            // Burst detected — accumulate into paste buffer
                            state
                                .paste_buffer
                                .get_or_insert_with(|| (now, Vec::new()))
                                .1
                                .push(c);
                        } else {
                            // Normal human-paced typing — flush any lingering
                            // paste buffer, then insert immediately.
                            state.flush_paste_buffer();
                            state.insert_input_char(c);
                        }
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Backspace => {
                        state.flush_paste_buffer();
                        state.backspace_input_char();
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Delete => {
                        state.flush_paste_buffer();
                        state.delete_input_char();
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Left => {
                        state.flush_paste_buffer();
                        state.move_input_cursor_left();
                        state.wake_input_caret();
                    }
                    KeyCode::Right => {
                        state.flush_paste_buffer();
                        state.move_input_cursor_right();
                        state.wake_input_caret();
                    }
                    KeyCode::Home => {
                        state.flush_paste_buffer();
                        state.move_input_cursor_home();
                        state.wake_input_caret();
                    }
                    KeyCode::End => {
                        state.flush_paste_buffer();
                        state.move_input_cursor_end();
                        state.wake_input_caret();
                    }
                    KeyCode::Up => {
                        state.flush_paste_buffer();
                        state.move_input_cursor_up();
                        state.wake_input_caret();
                    }
                    KeyCode::Down => {
                        state.flush_paste_buffer();
                        state.move_input_cursor_down();
                        state.wake_input_caret();
                    }
                    KeyCode::Tab => {
                        state.flush_paste_buffer();
                        if state.busy || !state.input.text.trim_start().starts_with('/') {
                            continue;
                        }
                        if let Some(selected) = state.selected_slash_command() {
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
                        state.flush_paste_buffer();
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
                        state.flush_paste_buffer();
                        let cur = state.list_state.offset();
                        *state.list_state.offset_mut() = cur.saturating_sub(10);
                        state.follow_bottom = false;
                    }
                    KeyCode::PageDown => {
                        state.flush_paste_buffer();
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
    use std::sync::mpsc;

    use crate::app::Provider;
    use crate::display_width::text_display_width;
    use crate::tool_format::tool_input_summary;
    use crate::tui::auth_modal::{AuthModal, AuthModalStep, ProviderKind};
    use crate::tui::repl_render::{line_to_plain_text, render_tool_call_lines, wrap_ansi_line};
    use crate::tui::ReplTuiEvent;
    use ratatui::text::Line;

    use super::{ReplTuiState, ToolCallStatus, TranscriptEntry};

    fn assert_matching_lengths(items: &[ratatui::widgets::ListItem<'static>], text: &[String]) {
        assert_eq!(items.len(), text.len());
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
}
