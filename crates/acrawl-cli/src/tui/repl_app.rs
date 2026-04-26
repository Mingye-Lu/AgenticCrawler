//! Ratatui REPL with a welcome screen, sticky-bottom chat transcript, slash overlay, and floating input.

use std::cmp::min;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::{slash_command_completion_candidates, AllowedToolSet, LiveCli};
use crate::display_width::{char_count_for_display_col, char_display_width, text_display_width};
use crate::format::render_repl_help;
use crate::markdown::PredictiveMarkdownBuffer;
use crate::tui::active_modal::ActiveModal;
use crate::tui::auth_modal::{AuthModal, AuthModalStep};
use crate::tui::modal::{Modal, ModalAction};
use crate::tui::repl_render::{
    ansi_to_lines, build_header_snapshot, draw_chat, draw_welcome, parse_report_rows,
    rect_contains_mouse, suspend_for_stdout, tool_input_summary,
};
use crate::tui::ReplTuiEvent;
use commands::{slash_command_specs, SlashCommand};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;
use ratatui::DefaultTerminal;

const MAX_INPUT_LINES: usize = 5;
pub(super) const WELCOME_BOX_SIDE_GUTTER: u16 = 16;
pub(super) const WELCOME_BOX_MAX_WIDTH: u16 = 82;
pub(super) const WELCOME_BOX_MIN_WIDTH: u16 = 30;
const INPUT_CARET_MARKER: char = '\u{E000}';
pub(super) const SLASH_OVERLAY_VISIBLE_ITEMS: usize = 7;
pub(super) const SLASH_OVERLAY_HINT_TEXT: &str =
    "Up/Down move  Enter accept  Tab complete  Esc close";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AppUiState {
    WelcomeMode,
    ChatMode,
}

#[derive(Clone, Debug)]
pub(super) enum ToolCallStatus {
    Running,
    Success { output: String },
    Error(String),
}

#[derive(Clone)]
pub(super) enum TranscriptEntry {
    System(String),
    Status(String),
    User(String),
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
    }
}

enum WorkerMsg {
    RunTurn(String),
    Shutdown,
}

struct InputEditorState {
    text: String,
    cursor: usize,
    preferred_col: Option<usize>,
}

#[derive(Default)]
pub(super) struct SelectionState {
    pub(super) anchor: Option<(u16, usize)>,
    pub(super) end: Option<(u16, usize)>,
    pub(super) pending_copy: Option<bool>,
    pub(super) mouse_drag_occurred: bool,
}

pub(super) struct TypewriterState {
    chars: VecDeque<char>,
    pub(super) live: String,
    pub(super) live_ansi: String,
    md_buffer: PredictiveMarkdownBuffer,
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
    input: InputEditorState,
    status_line: String,
    pub(super) busy: bool,
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
    spinner_tick: u8,
    spinner_deadline: Instant,
    pub(super) typewriter: TypewriterState,
    pub(super) selection: SelectionState,
    last_esc_at: Option<Instant>,
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
            input: InputEditorState {
                text: String::new(),
                cursor: 0,
                preferred_col: None,
            },
            status_line: String::new(),
            busy: false,
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
            spinner_tick: 0,
            spinner_deadline: Instant::now() + Duration::from_millis(120),
            typewriter: TypewriterState {
                chars: VecDeque::new(),
                live: String::new(),
                live_ansi: String::new(),
                md_buffer: PredictiveMarkdownBuffer::new(),
            },
            selection: SelectionState::default(),
            last_esc_at: None,
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
    fn tick_typewriter(&mut self, chars_per_tick: usize) {
        for _ in 0..chars_per_tick {
            match self.typewriter.chars.pop_front() {
                None => break,
                Some('\n') => {
                    self.typewriter
                        .md_buffer
                        .feed_char('\n', &mut self.typewriter.live_ansi);
                    let ansi = std::mem::take(&mut self.typewriter.live_ansi);
                    for styled_line in ansi_to_lines(&ansi) {
                        self.entries.push(TranscriptEntry::Stream(styled_line));
                    }
                    self.typewriter.live.clear();
                }
                Some(c) => {
                    self.typewriter.live.push(c);
                    self.typewriter
                        .md_buffer
                        .feed_char(c, &mut self.typewriter.live_ansi);
                }
            }
        }
    }

    fn flush_typewriter(&mut self) {
        if !self.typewriter.chars.is_empty() {
            let count = self.typewriter.chars.len();
            self.tick_typewriter(count);
        }
        if !self.typewriter.live.is_empty() {
            self.typewriter
                .md_buffer
                .flush(&mut self.typewriter.live_ansi);
            let ansi = std::mem::take(&mut self.typewriter.live_ansi);
            for styled_line in ansi_to_lines(&ansi) {
                self.entries.push(TranscriptEntry::Stream(styled_line));
            }
            self.typewriter.live.clear();
        }
    }

    fn wake_input_caret(&mut self) {
        self.cursor_on = true;
        self.cursor_blink_deadline = Instant::now() + Duration::from_millis(530);
    }

    fn input_char_len(&self) -> usize {
        self.input.text.chars().count()
    }

    fn input_char_to_byte(&self, char_idx: usize) -> usize {
        self.input
            .text
            .char_indices()
            .nth(char_idx)
            .map_or(self.input.text.len(), |(idx, _)| idx)
    }

    fn clamp_input_cursor(&mut self) {
        self.input.cursor = self.input.cursor.min(self.input_char_len());
    }

    fn insert_input_char(&mut self, ch: char) {
        self.clamp_input_cursor();
        let idx = self.input_char_to_byte(self.input.cursor);
        self.input.text.insert(idx, ch);
        self.input.cursor = self.input.cursor.saturating_add(1);
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn backspace_input_char(&mut self) {
        self.clamp_input_cursor();
        if self.input.cursor == 0 {
            return;
        }
        let prev = self.input.cursor - 1;
        let start = self.input_char_to_byte(prev);
        let end = self.input_char_to_byte(prev + 1);
        self.input.text.replace_range(start..end, "");
        self.input.cursor -= 1;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn delete_input_char(&mut self) {
        self.clamp_input_cursor();
        if self.input.cursor >= self.input_char_len() {
            return;
        }
        let start = self.input_char_to_byte(self.input.cursor);
        let end = self.input_char_to_byte(self.input.cursor + 1);
        self.input.text.replace_range(start..end, "");
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
        let lines = self.input_lines();
        let line = target_line.min(lines.len().saturating_sub(1));
        let col = char_count_for_display_col(lines[line], target_col);
        let mut cursor = 0usize;
        for input_line in lines.iter().take(line) {
            cursor += input_line.chars().count() + 1;
        }
        cursor += col;
        self.input.cursor = cursor.min(self.input_char_len());
        self.input_scroll_offset = usize::MAX;
    }

    fn move_input_cursor_left(&mut self) {
        self.input.cursor = self.input.cursor.saturating_sub(1);
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn move_input_cursor_right(&mut self) {
        self.input.cursor = (self.input.cursor + 1).min(self.input_char_len());
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

    fn move_input_cursor_up(&mut self) {
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

    fn move_input_cursor_down(&mut self) {
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
            self.typewriter
                .md_buffer
                .flush(&mut self.typewriter.live_ansi);
            let ansi = std::mem::take(&mut self.typewriter.live_ansi);
            for styled_line in ansi_to_lines(&ansi) {
                self.entries.push(TranscriptEntry::Stream(styled_line));
            }
            self.typewriter.live.clear();
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
        if !is_placeholder && seen_caret {
            if caret_row_idx < self.input_scroll_offset {
                self.input_scroll_offset = caret_row_idx;
            } else if caret_row_idx >= self.input_scroll_offset + max_text_lines {
                self.input_scroll_offset = caret_row_idx.saturating_sub(max_text_lines - 1);
            }
        }
        self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);

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
            let mut spans = Vec::new();

            if has_prompt {
                spans.push(Span::styled("❯ ", Style::default().fg(Color::LightCyan)));
            } else if skip > 0 && i == 0 {
                // If skipped first line with prompt, no visual space pad needed per standard terminal behavior
            }

            if is_placeholder && i == 0 {
                spans.push(Span::styled(row, text_style));
                let prompt_width = if has_prompt {
                    u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                } else {
                    0
                };
                cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
            } else {
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
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ReplTuiEvent::StreamAnsi(s) => {
                    // Enqueue raw chars for typewriter reveal.
                    for c in s.chars() {
                        self.typewriter.chars.push_back(c);
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
                    self.current_tool = None;
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

                                let mut store =
                                    api::credentials::load_credentials().unwrap_or_default();
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
            let report = cli.lock().expect("cli lock").status_report()?;
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
        SlashCommand::Resume { session_path } => {
            let mut g = cli.lock().expect("cli lock");
            let result = g.resume_session_command(session_path)?;
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
        SlashCommand::Session { action, target } => {
            let mut g = cli.lock().expect("cli lock");
            let result = g.session_command(action.as_deref(), target.as_deref())?;
            if result.persist_after {
                g.persist_session()?;
            }
            state.push_system_card("Session", &result.message);
        }
        SlashCommand::Debug => {
            let report = cli.lock().expect("cli lock").debug_tool_call_report()?;
            state.push_system("Debug Tool Call");
            state.push_system(&report);
        }
        SlashCommand::Headed => {
            std::env::set_var("HEADLESS", "false");
            let _ = runtime::update_settings(|s| {
                s.headless = Some(false);
            });
            cli.lock().expect("cli lock").reset_browser();
            state.push_system_card(
                "Browser Mode",
                "Browser mode\n  Result           switched to headed (visible)",
            );
        }
        SlashCommand::Headless => {
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
        SlashCommand::Auth { provider } => {
            if state.busy {
                state.push_system("Please wait for the current task to finish.");
                return Ok(());
            }
            let parsed_provider = provider
                .as_deref()
                .and_then(|p| crate::app::parse_provider_arg(p).ok());
            state.active_modal = Some(ActiveModal::Auth(AuthModal::new(
                ui_tx.clone(),
                parsed_provider,
            )));
            if let Some(crate::app::Provider::Anthropic) = parsed_provider {
                spawn_anthropic_oauth_thread(ui_tx.clone(), &mut state.active_modal);
            }
        }
        other @ SlashCommand::Unknown(_) => {
            suspend_for_stdout(terminal, || {
                let mut g = cli.lock().expect("cli lock");
                let _ = g.handle_repl_command(other);
            })?;
            state.push_system("(slash command executed in classic output mode)");
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
            let mut store = api::credentials::load_credentials().unwrap_or_default();
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
            let mut store = api::credentials::load_credentials().unwrap_or_default();
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

/// Interactive REPL using Ratatui when stdout is a TTY (unless `ACRAWL_CLASSIC_REPL` is set).
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
    cancel_flag: &Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = execute!(io::stdout(), event::EnableMouseCapture);
    let _ = execute!(io::stdout(), SetCursorStyle::SteadyBar);
    let _mouse_guard = MouseCaptureGuard;

    let mut state = ReplTuiState::new();

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

        if state.exit {
            if state.persist_on_exit {
                let g = cli.lock().expect("cli lock");
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
            match state.ui_state {
                AppUiState::WelcomeMode => {
                    draw_welcome(frame, frame.area(), &mut state, show_input_cursor);
                }
                AppUiState::ChatMode => draw_chat(frame, &mut state, &header, show_input_cursor),
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

                if state.ui_state == AppUiState::ChatMode && in_transcript {
                    let max_off = state
                        .last_wrapped_len
                        .saturating_sub(state.last_view_height.max(1));
                    let cur = state.list_state.offset();
                    let tr = state.last_transcript_rect;
                    match me.kind {
                        MouseEventKind::ScrollUp => {
                            *state.list_state.offset_mut() = cur.saturating_sub(3);
                            state.follow_bottom = false;
                        }
                        MouseEventKind::ScrollDown => {
                            *state.list_state.offset_mut() = (cur.saturating_add(3)).min(max_off);
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
                        }
                        _ => {}
                    }
                } else if in_input || state.ui_state == AppUiState::WelcomeMode {
                    state.selection.anchor = None;
                    state.selection.end = None;
                    match me.kind {
                        MouseEventKind::ScrollUp => {
                            state.input_scroll_offset = state.input_scroll_offset.saturating_sub(1);
                        }
                        MouseEventKind::ScrollDown => {
                            state.input_scroll_offset = state.input_scroll_offset.saturating_add(1);
                        }
                        _ => {}
                    }
                }
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if !matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                    state.selection.anchor = None;
                    state.selection.end = None;
                }

                let mut modal_action = None;
                let mut modal_succeeded = false;
                let mut oauth_provider = None;
                let mut model_outcome = None;

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
                    if let Some(m) = modal.as_model() {
                        model_outcome = Some(m.outcome().clone());
                    }
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

                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if state.busy {
                        cancel_flag.store(true, Ordering::Release);
                        state.push_system("Interrupting…");
                        continue;
                    }
                    state.exit = true;
                    state.persist_on_exit = true;
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
                    g.cycle_reasoning_effort();
                    state.cached_header = build_header_snapshot(&g);
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

                match key.code {
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        state.insert_input_char('\n');
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Enter => {
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
                        if trimmed.eq_ignore_ascii_case("/exit")
                            || trimmed.eq_ignore_ascii_case("/quit")
                        {
                            state.exit = true;
                            state.persist_on_exit = true;
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
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Char(c) => {
                        state.insert_input_char(c);
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
                        if state.busy {
                            let now = Instant::now();
                            if state
                                .last_esc_at
                                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(500))
                            {
                                cancel_flag.store(true, Ordering::Release);
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
    use std::sync::mpsc;

    use crate::app::Provider;
    use crate::display_width::text_display_width;
    use crate::tui::auth_modal::{AuthModal, AuthModalStep, ProviderKind};
    use crate::tui::repl_render::{
        line_to_plain_text, render_tool_call_lines, tool_input_summary, wrap_ansi_line,
    };
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
        let (items, text) =
            render_tool_call_lines("bash", "echo hello", &ToolCallStatus::Running, 80, '⠋');
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
        );
        assert_eq!(items.len(), 1);
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_input_truncation() {
        let long_input = "a".repeat(80);
        let (items, text) =
            render_tool_call_lines("bash", &long_input, &ToolCallStatus::Running, 80, '⠋');
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
        let (items, text) =
            render_tool_call_lines("bash", "cmd", &ToolCallStatus::Success { output }, 80, '⠋');
        assert!(
            items.len() >= 2,
            "Expected header + stderr line, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_read_file_with_content() {
        let output = serde_json::json!({
            "file": {
                "filePath": "src/main.rs",
                "startLine": 1,
                "numLines": 3,
                "totalLines": 100,
                "content": "fn main() {\n    println!(\"hi\");\n}"
            }
        })
        .to_string();
        let (items, text) = render_tool_call_lines(
            "read_file",
            "src/main.rs",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
        );
        assert!(
            items.len() >= 2,
            "Expected header + content lines, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_write_file_single_line() {
        let output = serde_json::json!({
            "filePath": "out.txt",
            "type": "create",
            "content": "line1\nline2\nline3"
        })
        .to_string();
        let (items, text) = render_tool_call_lines(
            "write_file",
            "out.txt",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
        );
        assert_eq!(items.len(), 1, "write_file should produce exactly 1 line");
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_edit_file_with_patch() {
        let output = serde_json::json!({
            "filePath": "src/lib.rs",
            "structuredPatch": [{
                "lines": ["-old line", "+new line", " context"]
            }]
        })
        .to_string();
        let (items, text) = render_tool_call_lines(
            "edit_file",
            "src/lib.rs",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
        );
        assert!(
            items.len() >= 2,
            "Expected header + diff lines, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_glob_search_files() {
        let output = serde_json::json!({
            "numFiles": 3,
            "filenames": ["a.rs", "b.rs", "c.rs"]
        })
        .to_string();
        let (items, text) = render_tool_call_lines(
            "glob_search",
            "*.rs",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
        );
        assert_eq!(
            items.len(),
            4,
            "Expected header + 3 filenames = 4, got {}",
            items.len()
        );
        assert_matching_lengths(&items, &text);
    }

    #[test]
    fn render_tool_call_grep_search_matches() {
        let output = serde_json::json!({
            "numMatches": 5,
            "numFiles": 2,
            "filenames": ["a.rs", "b.rs"]
        })
        .to_string();
        let (items, text) = render_tool_call_lines(
            "grep_search",
            "pattern",
            &ToolCallStatus::Success { output },
            80,
            '⠋',
        );
        assert!(
            items.len() >= 2,
            "Expected header + filenames, got {}",
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
        let (items, text) =
            render_tool_call_lines("bash", "cmd", &ToolCallStatus::Success { output }, 80, '⠋');
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
        assert_eq!(
            tool_input_summary("read_file", r#"{"filePath":"src/main.rs"}"#),
            "src/main.rs"
        );
        assert_eq!(
            tool_input_summary("glob_search", r#"{"pattern":"*.rs"}"#),
            "*.rs"
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
}
