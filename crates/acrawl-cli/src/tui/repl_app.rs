//! Ratatui REPL with a welcome screen, sticky-bottom chat transcript, slash overlay, and floating input.

use std::cmp::min;
use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::render::TerminalRenderer;
use ansi_to_tui::IntoText;
use commands::{slash_command_specs, SlashCommand};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use crossterm::execute;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph,
    Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::DefaultTerminal;
use runtime::{
    format_usd, pricing_for_model, PermissionMode, PermissionPromptDecision, PermissionRequest,
};

use crate::app::{
    slash_command_completion_candidates, AllowedToolSet, ChannelPermissionPrompter, LiveCli,
};
use crate::format::{render_repl_help, VERSION};
use crate::tui::auth_modal::{AuthModal, AuthModalStep};
use crate::tui::modal::{Modal, ModalAction};
use crate::tui::ReplTuiEvent;

const MAX_INPUT_LINES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppUiState {
    WelcomeMode,
    ChatMode,
}

#[derive(Clone)]
enum TranscriptEntry {
    System(String),
    Status(String),
    User(String),
    Stream(Line<'static>),
    SystemCard {
        title: String,
        rows: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone)]
struct SlashOverlayItem {
    command: String,
    summary: &'static str,
}

#[derive(Debug, Clone)]
struct SlashOverlay {
    items: Vec<SlashOverlayItem>,
    selected: usize,
    scroll_offset: usize,
}

#[derive(Debug, Clone)]
struct HeaderSnapshot {
    model: String,
    permission_mode: PermissionMode,
    session_id: String,
    cost_text: String,
    context_text: String,
}

impl Default for HeaderSnapshot {
    fn default() -> Self {
        Self {
            model: "--".to_string(),
            permission_mode: PermissionMode::ReadOnly,
            session_id: "--".to_string(),
            cost_text: "--".to_string(),
            context_text: "--".to_string(),
        }
    }
}

fn format_compact_tokens(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}m", f64::from(tokens) / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", f64::from(tokens) / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn build_header_snapshot(cli: &LiveCli) -> HeaderSnapshot {
    let usage = cli.cumulative_usage();
    let pricing = pricing_for_model(cli.model_name());
    let estimate = pricing.map_or_else(
        || usage.estimate_cost_usd(),
        |model_pricing| usage.estimate_cost_usd_with_pricing(model_pricing),
    );
    HeaderSnapshot {
        model: cli.model_name().to_string(),
        permission_mode: cli.permission_mode(),
        session_id: cli.session_id().to_string(),
        cost_text: format_usd(estimate.total_cost_usd()),
        context_text: format!("{} ctx", format_compact_tokens(usage.total_tokens())),
    }
}

fn permission_badge(mode: PermissionMode) -> (String, Style) {
    match mode {
        PermissionMode::ReadOnly => (
            "[ LOCK Read-Only ]".to_string(),
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        ),
        PermissionMode::WorkspaceWrite => (
            "[ WRITE Workspace ]".to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        PermissionMode::DangerFullAccess | PermissionMode::Prompt | PermissionMode::Allow => (
            "[ ! FULL ACCESS ]".to_string(),
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

fn wrap_plain_text(input: &str, width: u16) -> Vec<String> {
    let w = usize::from(width.max(8));
    textwrap::wrap(input, w)
        .into_iter()
        .map(std::borrow::Cow::into_owned)
        .collect()
}

fn parse_report_rows(report: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut section = String::new();
    for line in report.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with("  ") {
            section = line.trim().to_string();
            continue;
        }
        let trimmed = line.trim();
        let bytes = trimmed.as_bytes();
        let mut split = None;
        let mut run = 0usize;
        for (idx, b) in bytes.iter().enumerate() {
            if *b == b' ' {
                run += 1;
                if run >= 2 {
                    split = Some(idx + 1 - run);
                    break;
                }
            } else {
                run = 0;
            }
        }
        if let Some(idx) = split {
            let key = trimmed[..idx].trim();
            let value = trimmed[idx..].trim();
            if !key.is_empty() && !value.is_empty() {
                if section.is_empty() {
                    rows.push((key.to_string(), value.to_string()));
                } else {
                    rows.push((format!("{section} · {key}"), value.to_string()));
                }
            }
        }
    }
    if rows.is_empty() {
        let compact = report.lines().take(10).collect::<Vec<_>>().join(" ");
        rows.push(("detail".to_string(), compact));
    }
    rows
}

/// Simple ANSI-aware line wrapping for Ratatui Lines.
fn wrap_ansi_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let mut current_line_spans = Vec::new();
    let mut current_width = 0;

    let target_width = usize::from(width.max(10));

    for span in line.spans {
        let span_width = span.width();

        if current_width + span_width <= target_width {
            current_width += span_width;
            current_line_spans.push(span);
        } else {
            // Need to split the span
            let mut remaining_text = span.content.to_string();
            let style = span.style;

            while !remaining_text.is_empty() {
                let available = target_width.saturating_sub(current_width);
                if available == 0 {
                    result.push(Line::from(std::mem::take(&mut current_line_spans)));
                    current_width = 0;
                    continue;
                }

                // Find how many chars of remaining_text fit in 'available' width
                // Simplified: assuming each char is 1 width for now (AgenticCrawler mostly uses ASCII/Simple UTF8 here)
                let split_idx = remaining_text
                    .chars()
                    .take(available)
                    .map(char::len_utf8)
                    .sum::<usize>();
                let head = remaining_text[..split_idx].to_string();
                remaining_text = remaining_text[split_idx..].to_string();

                current_line_spans.push(Span::styled(head, style));
                current_width += available; // Or actual width if using unicode_width

                if current_width >= target_width {
                    result.push(Line::from(std::mem::take(&mut current_line_spans)));
                    current_width = 0;
                }
            }
        }
    }

    if !current_line_spans.is_empty() {
        result.push(Line::from(current_line_spans));
    }

    result
}

#[allow(clippy::too_many_lines)]
fn build_wrapped_list(
    entries: &[TranscriptEntry],
    width: u16,
    live_text: Option<&str>,
) -> Vec<ListItem<'static>> {
    let mut out = Vec::new();
    // Restore the top padding margin
    out.push(ListItem::new(Line::from(" ")));

    let system_style = Style::default().fg(Color::DarkGray).italic();
    let user_prefix_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    for entry in entries {
        match entry {
            TranscriptEntry::System(text) => {
                for row in wrap_plain_text(text, width) {
                    out.push(ListItem::new(Line::from(Span::styled(row, system_style))));
                }
            }
            TranscriptEntry::User(text) => {
                let prefixed = format!("  You {text}");
                let rows = wrap_plain_text(&prefixed, width);
                let user_bg = Color::Rgb(35, 45, 60); // Subtle blue-gray background
                for (idx, row) in rows.into_iter().enumerate() {
                    let line = if idx == 0 && row.trim_start().starts_with("You ") {
                        let trimmed = row.trim_start();
                        let rest = trimmed.get(4..).unwrap_or("").to_string();
                        Line::from(vec![
                            Span::raw("  "),
                            Span::styled("You ", user_prefix_style),
                            Span::raw(rest),
                        ])
                    } else {
                        Line::from(Span::raw(row))
                    };
                    out.push(ListItem::new(line).bg(user_bg));
                }
            }
            TranscriptEntry::Status(text) => {
                let status_style = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC);
                for row in wrap_plain_text(text, width) {
                    out.push(ListItem::new(Line::from(Span::styled(row, status_style))));
                }
            }
            TranscriptEntry::Stream(line) => {
                let as_text = line.to_string();
                let style = line.style;
                for row in wrap_plain_text(&as_text, width) {
                    out.push(ListItem::new(Line::from(Span::styled(row, style))));
                }
            }
            TranscriptEntry::SystemCard { title, rows } => {
                let border_style = Style::default().fg(Color::Yellow);
                let key_style = Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD);
                out.push(ListItem::new(Line::from(Span::styled(
                    format!("┌─ {title} "),
                    border_style,
                ))));
                for (key, value) in rows {
                    let key_width = usize::from(width.max(24)).saturating_sub(8).min(30);
                    let value_width = usize::from(width.max(24))
                        .saturating_sub(key_width)
                        .saturating_sub(7)
                        .max(8);
                    let wrapped = textwrap::wrap(value, value_width);
                    for (idx, line) in wrapped.into_iter().enumerate() {
                        if idx == 0 {
                            out.push(ListItem::new(Line::from(vec![
                                Span::styled("│ ", border_style),
                                Span::styled(format!("{key:key_width$}"), key_style),
                                Span::raw(" "),
                                Span::raw(line.into_owned()),
                            ])));
                        } else {
                            out.push(ListItem::new(Line::from(vec![
                                Span::styled("│ ", border_style),
                                Span::raw(" ".repeat(key_width)),
                                Span::raw(" "),
                                Span::raw(line.into_owned()),
                            ])));
                        }
                    }
                }
                out.push(ListItem::new(Line::from(Span::styled(
                    "└────────────────",
                    border_style,
                ))));
            }
        }

        // Add a blank separator line ONLY after User messages or Cards to separate blocks
        match entry {
            TranscriptEntry::User(_) | TranscriptEntry::SystemCard { .. } => {
                out.push(ListItem::new(Line::from(" ")));
            }
            _ => {}
        }
    }

    // Live typewriter line shown at the bottom during streaming
    if let Some(text) = live_text {
        if !text.is_empty() {
            // Render the live fragment using the Markdown renderer to maintain high-fidelity formatting (bold, etc.)
            let renderer = TerminalRenderer::new();
            let rendered_ansi = renderer.render_markdown_fragment(text);

            if let Ok(ansi_text) = rendered_ansi.as_bytes().into_text() {
                // Wrap each rendered line to the available width to prevent overflow
                for line in ansi_text {
                    for wrapped_line in wrap_ansi_line(line, width) {
                        out.push(ListItem::new(wrapped_line));
                    }
                }
            } else {
                // Fallback to plain text if ANSI conversion fails
                let live_style = Style::default().fg(Color::Rgb(215, 225, 235));
                for row in wrap_plain_text(text, width) {
                    out.push(ListItem::new(Line::from(Span::styled(row, live_style))));
                }
            }
        }
    }
    out
}

fn rect_contains_mouse(r: Rect, col: u16, row: u16) -> bool {
    if r.width == 0 || r.height == 0 {
        return false;
    }
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

struct MouseCaptureGuard;

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), event::DisableMouseCapture);
    }
}

enum WorkerMsg {
    RunTurn(String),
    Shutdown,
}

#[allow(clippy::struct_excessive_bools)]
struct ReplTuiState {
    ui_state: AppUiState,
    entries: Vec<TranscriptEntry>,
    list_state: ListState,
    follow_bottom: bool,
    last_transcript_rect: Rect,
    last_wrapped_len: usize,
    last_view_height: usize,
    last_input_rect: Rect,
    input_scroll_offset: usize,
    input: String,
    status_line: String,
    busy: bool,
    pending_permission: Option<(PermissionRequest, Sender<PermissionPromptDecision>)>,
    active_modal: Option<AuthModal>,
    exit: bool,
    current_tool: Option<String>,
    status_entry_index: Option<usize>,
    persist_on_exit: bool,
    cursor_on: bool,
    cursor_blink_deadline: Instant,
    slash_overlay: Option<SlashOverlay>,
    cached_header: HeaderSnapshot,
    spinner_tick: u8,
    spinner_deadline: Instant,
    /// Queue of plain-text chars waiting to be revealed by the typewriter.
    typewriter_chars: VecDeque<char>,
    /// The current line being built char-by-char (shown as live line).
    typewriter_live: String,
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
            input: String::new(),
            status_line: String::new(),
            busy: false,
            pending_permission: None,
            active_modal: None,
            exit: false,
            persist_on_exit: false,
            current_tool: None,
            status_entry_index: None,
            cursor_on: true,
            cursor_blink_deadline: Instant::now() + Duration::from_millis(530),
            slash_overlay: None,
            cached_header: HeaderSnapshot::default(),
            spinner_tick: 0,
            spinner_deadline: Instant::now() + Duration::from_millis(120),
            typewriter_chars: VecDeque::new(),
            typewriter_live: String::new(),
        }
    }

    fn tick_input_caret(&mut self) {
        let now = Instant::now();
        let advance_spinner = now >= self.spinner_deadline;
        if now >= self.cursor_blink_deadline {
            self.cursor_on = !self.cursor_on;
            self.cursor_blink_deadline = now + Duration::from_millis(530);
        }
        if advance_spinner {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
            self.spinner_deadline = now + Duration::from_millis(120);
        }
        if let Some(ref mut modal) = self.active_modal {
            if let AuthModalStep::OAuthWaiting { tick, .. } = &mut modal.step {
                if advance_spinner {
                    *tick = tick.wrapping_add(1);
                }
            }
        }
    }

    /// Returns the spinner frame matching the current tick.
    fn spinner_char(&self) -> char {
        const FRAMES: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];
        FRAMES[usize::from(self.spinner_tick) % FRAMES.len()]
    }

    /// Context-aware placeholder shown when the input box is empty.
    fn input_placeholder(&self) -> &'static str {
        if self.pending_permission.is_some() {
            "Waiting for your authorization  (y / n / Esc)…"
        } else if self.busy {
            "AgenticCrawler is working…  (you can queue your next prompt)"
        } else if self.ui_state == AppUiState::WelcomeMode {
            "What is our goal today?"
        } else {
            "Any follow-up instructions?"
        }
    }

    /// Advance the typewriter: reveal `chars_per_tick` chars from the queue.
    fn tick_typewriter(&mut self, chars_per_tick: usize) {
        let live_style = Style::default().fg(Color::Rgb(215, 225, 235));
        for _ in 0..chars_per_tick {
            match self.typewriter_chars.pop_front() {
                None => break,
                Some('\n') => {
                    let line = std::mem::take(&mut self.typewriter_live);
                    self.entries
                        .push(TranscriptEntry::Stream(Line::from(Span::styled(
                            line, live_style,
                        ))));
                    self.follow_bottom = true;
                }
                Some(c) => {
                    self.typewriter_live.push(c);
                    self.follow_bottom = true;
                }
            }
        }
    }

    fn wake_input_caret(&mut self) {
        self.cursor_on = true;
        self.cursor_blink_deadline = Instant::now() + Duration::from_millis(530);
    }

    #[allow(clippy::too_many_lines)]
    fn calculate_input_dimensions(&mut self, width: u16) -> (u16, Vec<Line<'static>>, usize) {
        let is_placeholder = self.input.is_empty();
        let placeholder_text = self.input_placeholder();
        let mut lines_data = self
            .input
            .split('\n')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if lines_data.is_empty() {
            lines_data.push(String::new());
        }
        let logical_lines = if is_placeholder {
            vec![placeholder_text.to_owned()]
        } else {
            lines_data
        };

        let safe_width = width.saturating_sub(5).max(5) as usize;
        let mut visual_lines = Vec::new();

        for (logical_idx, line) in logical_lines.into_iter().enumerate() {
            let offset = if logical_idx == 0 { 2 } else { 0 };
            let first_line_width = safe_width.saturating_sub(offset);

            if line.is_empty() {
                visual_lines.push((logical_idx == 0, String::new()));
                continue;
            }

            let mut current = String::new();
            let mut w = 0;
            let mut is_first_chunk = true;
            let mut pushed_last = false;

            for c in line.chars() {
                current.push(c);
                w += 1;
                let target = if is_first_chunk {
                    first_line_width
                } else {
                    safe_width
                };
                if w >= target {
                    visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                    current = String::new();
                    w = 0;
                    is_first_chunk = false;
                    pushed_last = true;
                } else {
                    pushed_last = false;
                }
            }
            if !current.is_empty() || pushed_last {
                visual_lines.push((logical_idx == 0 && is_first_chunk, current));
            }
        }

        let max_text_lines = MAX_INPUT_LINES;
        let total_visual = visual_lines.len();
        let max_scroll = total_visual.saturating_sub(max_text_lines);
        self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);

        let skip = self.input_scroll_offset;
        let sliced = visual_lines
            .into_iter()
            .skip(skip)
            .take(max_text_lines)
            .collect::<Vec<_>>();
        let total_sliced = sliced.len();

        let caret = if self.cursor_on { "|" } else { " " };
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
            let is_last = i == total_sliced.saturating_sub(1);
            let mut spans = Vec::new();

            if has_prompt {
                spans.push(Span::styled("❯ ", Style::default().fg(Color::LightCyan)));
            } else if skip > 0 && i == 0 {
                // If skipped first line with prompt, no visual space pad needed per standard terminal behavior
            }

            if is_last && is_placeholder {
                spans.push(Span::styled(
                    caret,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(row, text_style));
            } else {
                spans.push(Span::styled(row, text_style));
                if is_last && !is_placeholder {
                    spans.push(Span::styled(
                        caret,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }
            render_lines.push(Line::from(spans));
        }

        render_lines.push(Line::from(""));

        #[allow(clippy::cast_possible_truncation)]
        let box_height = (total_sliced as u16) + 4;
        (box_height, render_lines, max_scroll)
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
        let trimmed = self.input.trim();
        if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
            self.slash_overlay = None;
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
            return;
        }

        let (selected, mut scroll_offset) = self.slash_overlay.as_ref().map_or((0, 0), |prev| {
            (prev.selected.min(candidates.len() - 1), prev.scroll_offset)
        });
        let visible_count = min(candidates.len(), 7);
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

    fn clamp_scroll_offset(&mut self) {
        let max_offset = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        if self.list_state.offset() > max_offset {
            *self.list_state.offset_mut() = max_offset;
        }
        self.follow_bottom = self.list_state.offset() >= max_offset;
    }

    fn scroll_to_bottom(&mut self) {
        let max_offset = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        *self.list_state.offset_mut() = max_offset;
    }

    fn drain_events(&mut self, rx: &Receiver<ReplTuiEvent>) {
        let mut had_new_rows = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ReplTuiEvent::StreamAnsi(s) => {
                    // Enqueue raw chars for typewriter reveal.
                    for c in s.chars() {
                        self.typewriter_chars.push_back(c);
                    }
                }
                ReplTuiEvent::TurnStarting => {
                    self.busy = true;
                    self.current_tool = None;
                    self.status_line = "Thinking...".to_string();
                    had_new_rows = true;
                    self.status_entry_index = Some(self.entries.len());
                    self.entries.push(TranscriptEntry::Status(
                        "· Thinking about next move...".to_string(),
                    ));
                }
                ReplTuiEvent::ToolStarting { name, input } => {
                    self.current_tool = Some(name.clone());
                    let truncated_input = if input.len() > 30 {
                        format!("{}...", &input[..27])
                    } else {
                        input.clone()
                    };
                    self.status_line = format!("Executing {name}({truncated_input})...");
                    if let Some(idx) = self.status_entry_index {
                        if let Some(TranscriptEntry::Status(s)) = self.entries.get_mut(idx) {
                            *s = format!("· Executing {name}({truncated_input})...");
                        }
                    }
                }
                ReplTuiEvent::TurnFinished(result) => {
                    self.busy = false;
                    self.current_tool = None;
                    self.status_line = match &result {
                        Ok(()) => "Ready".to_string(),
                        Err(e) => format!("Error: {e}"),
                    };

                    // Flush any remaining characters in the typewriter and clear status
                    if !self.typewriter_chars.is_empty() {
                        let count = self.typewriter_chars.len();
                        self.tick_typewriter(count);
                    }
                    if !self.typewriter_live.is_empty() {
                        let live_style = Style::default().fg(Color::Rgb(215, 225, 235));
                        let line = std::mem::take(&mut self.typewriter_live);
                        self.entries
                            .push(TranscriptEntry::Stream(Line::from(Span::styled(
                                line, live_style,
                            ))));
                    }

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
                    had_new_rows = true;
                }
                ReplTuiEvent::PermissionNeeded { request, respond } => {
                    self.pending_permission = Some((request, respond));
                }
                ReplTuiEvent::SystemMessage(s) => {
                    had_new_rows = true;
                    self.push_system(&s);
                }
                ReplTuiEvent::AuthOAuthComplete { provider, result } => {
                    if let Some(ref mut modal) = self.active_modal {
                        modal.step = match result {
                            Ok(()) => AuthModalStep::Success {
                                provider: crate::app::parse_provider_arg(&provider)
                                    .unwrap_or(crate::app::Provider::Anthropic),
                                message: format!("Authenticated as {provider}"),
                            },
                            Err(e) => AuthModalStep::Error { message: e },
                        };
                    }
                    had_new_rows = true;
                }
                ReplTuiEvent::AuthOAuthProgress { message } => {
                    if let Some(ref mut modal) = self.active_modal {
                        if let AuthModalStep::OAuthWaiting { status, .. } = &mut modal.step {
                            *status = message;
                        }
                    }
                    had_new_rows = true;
                }
            }
        }
        if had_new_rows {
            self.follow_bottom = true;
        }
    }
}

fn draw_permission_modal(frame: &mut ratatui::Frame<'_>, request: &PermissionRequest) {
    let area = frame.area();
    let block_area = area.inner(Margin {
        horizontal: area.width / 6,
        vertical: area.height / 4,
    });
    frame.render_widget(Clear, block_area);
    let block = Block::default()
        .title(" Permission Required ")
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(200, 120, 40)));
    let inner = block.inner(block_area);
    frame.render_widget(block, block_area);
    let text = Text::from(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Tool: ", Style::default().fg(Color::DarkGray)),
            Span::styled(request.tool_name.clone(), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("Current mode: ", Style::default().fg(Color::DarkGray)),
            Span::raw(request.current_mode.as_str()),
        ]),
        Line::from(vec![
            Span::styled("Required mode: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                request.required_mode.as_str(),
                Style::default().fg(Color::LightRed),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            request.input.clone(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" allow   "),
            Span::styled(
                "n",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" deny"),
        ]),
    ]);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, inner);
}

fn suspend_for_stdout(terminal: &mut DefaultTerminal, f: impl FnOnce()) -> io::Result<()> {
    ratatui::try_restore()?;
    f();
    *terminal = ratatui::try_init()?;
    let _ = execute!(io::stdout(), event::EnableMouseCapture);
    Ok(())
}

fn draw_header(frame: &mut ratatui::Frame<'_>, area: Rect, header: &HeaderSnapshot) {
    let (perm_text, perm_style) = permission_badge(header.permission_mode);
    let mut spans = vec![
        Span::styled(
            " ACrawl ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(perm_text, perm_style),
        Span::raw(format!("  model={} ", header.model)),
    ];
    let left_w = Line::from(spans.clone()).width();
    let right_text = format!(
        "session:{}  cost:{} ({})",
        header.session_id, header.cost_text, header.context_text
    );
    let total_w = usize::from(area.width);
    let right_w = right_text.chars().count();
    let gap = total_w.saturating_sub(left_w + right_w).max(1);

    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(
        right_text,
        Style::default()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::DIM),
    ));
    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Rgb(14, 18, 28))),
        area,
    );
}

fn draw_welcome(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut ReplTuiState) {
    let ascii = [
        "  █████╗  ██████╗██████╗  █████╗ ██╗    ██╗██╗",
        " ██╔══██╗██╔════╝██╔══██╗██╔══██╗██║    ██║██║",
        " ███████║██║     ██████╔╝███████║██║ █╗ ██║██║",
        " ██╔══██║██║     ██╔══██╗██╔══██║██║███╗██║██║",
        " ██║  ██║╚██████╗██║  ██║██║  ██║╚███╔███╔╝███████╗",
        " ╚═╝  ╚═╝ ╚═════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝ ╚══════╝",
    ];

    let outer = Block::default();
    frame.render_widget(outer, area);

    let mut lines = Vec::new();
    for row in ascii {
        lines.push(Line::from(Span::styled(
            row,
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    let version_str = format!("v{VERSION} · Playwright ready");
    let max_w = ascii.iter().map(|s| s.chars().count()).max().unwrap_or(0);
    let pad = max_w.saturating_sub(version_str.chars().count()) / 2;
    let version_padded = format!("{}{version_str}", " ".repeat(pad));
    lines.push(Line::from(Span::styled(
        version_padded,
        Style::default().fg(Color::DarkGray),
    )));

    let art_h = u16::try_from(lines.len()).unwrap_or(6);
    let art_w = u16::try_from(max_w).unwrap_or(60);
    let art_x = area.x + area.width.saturating_sub(art_w) / 2;
    let art_y = area.y + area.height.saturating_sub(art_h + 8) / 2;
    let art_area = Rect::new(art_x, art_y, art_w.min(area.width), art_h.min(area.height));
    frame.render_widget(Paragraph::new(Text::from(lines)), art_area);

    let input_w = area.width.saturating_sub(12).clamp(30, 90);
    let (box_height, render_lines, max_scroll) = state.calculate_input_dimensions(input_w);
    let input_h = box_height;

    let input_x = area.x + area.width.saturating_sub(input_w) / 2;
    let input_y = art_y.saturating_add(art_h).saturating_add(2);
    let input_area = Rect::new(
        input_x,
        input_y.min(area.y.saturating_add(area.height.saturating_sub(input_h))),
        input_w,
        input_h,
    );
    let block = Block::default()
        .title(" Goal ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::LightBlue))
        .padding(ratatui::widgets::Padding::symmetric(1, 0));
    let inner = block.inner(input_area);
    state.last_input_rect = inner;
    frame.render_widget(block, input_area);

    frame.render_widget(Paragraph::new(Text::from(render_lines)), inner);
    if max_scroll > 0 {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(" "))
            .thumb_symbol("▐")
            .style(Style::default().fg(Color::LightBlue));
        let mut scrollbar_state =
            ScrollbarState::new(max_scroll).position(state.input_scroll_offset);
        frame.render_stateful_widget(
            scrollbar,
            inner.inner(Margin {
                vertical: 0,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

    draw_slash_overlay(frame, state, input_area, area);
}

fn draw_slash_overlay(
    frame: &mut ratatui::Frame<'_>,
    state: &ReplTuiState,
    input_area: Rect,
    bounds: Rect,
) {
    let Some(overlay) = &state.slash_overlay else {
        return;
    };
    let total = overlay.items.len();
    let visible_count = min(total, 7);
    let scroll_offset = overlay.scroll_offset;
    let overlay_h = u16::try_from(visible_count + 2).unwrap_or(4);
    let overlay_w = min(bounds.width.saturating_sub(2), 70).max(30);
    let overlay_x = input_area.x + 2;
    let overlay_y = input_area.y.saturating_sub(overlay_h).max(bounds.y + 1);
    let overlay_area = Rect::new(overlay_x, overlay_y, overlay_w, overlay_h);
    frame.render_widget(Clear, overlay_area);
    let title = if total > visible_count {
        format!(" Slash Commands ({}/{}) ", overlay.selected + 1, total)
    } else {
        " Slash Commands ".to_string()
    };
    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(Color::LightCyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(70, 120, 150)));
    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);
    let items = overlay
        .items
        .iter()
        .skip(scroll_offset)
        .take(visible_count)
        .map(|item| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<14}", item.command),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(item.summary),
            ]))
        })
        .collect::<Vec<_>>();
    let mut list_state = ListState::default();
    list_state.select(Some(overlay.selected.saturating_sub(scroll_offset)));
    let list = List::new(items)
        .highlight_style(Style::default().bg(Color::Rgb(30, 44, 56)))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, inner, &mut list_state);
}

#[allow(clippy::too_many_lines)]
fn draw_chat(frame: &mut ratatui::Frame<'_>, state: &mut ReplTuiState, header: &HeaderSnapshot) {
    let area = frame.area();
    let (footer_h, render_lines, max_scroll) = state.calculate_input_dimensions(area.width);

    // Layout: 1-row header | transcript | 1-row spacer | input footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Length(footer_h),
        ])
        .split(area);
    let header_area = chunks[0];
    let main_area = chunks[1];
    // chunks[2] is the spacer gap - intentionally left empty for breathing room
    let input_area = chunks[3];

    draw_header(frame, header_area, header);

    // --- Transcript block (rounded, matches welcome palette) ---
    let transcript_border_color = if state.busy {
        Color::Rgb(40, 80, 110)
    } else {
        Color::Rgb(50, 65, 90)
    };
    let main_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(transcript_border_color));
    let main_inner = main_block.inner(main_area);
    frame.render_widget(main_block, main_area);

    let live_line = if state.typewriter_live.is_empty() {
        None
    } else {
        Some(state.typewriter_live.as_str())
    };
    let wrapped = build_wrapped_list(&state.entries, main_inner.width, live_line);
    state.last_transcript_rect = main_inner;
    state.last_wrapped_len = wrapped.len();
    state.last_view_height = usize::from(main_inner.height.max(1));
    state.clamp_scroll_offset();
    if state.follow_bottom {
        state.scroll_to_bottom();
    }

    let list = List::new(wrapped)
        .highlight_spacing(HighlightSpacing::Never)
        .scroll_padding(2);
    frame.render_stateful_widget(list, main_inner, &mut state.list_state);

    // Busy indicator overlay at bottom-right of transcript
    if state.busy {
        let spinner = state.spinner_char();
        let label = format!(" {spinner} Generating… ");
        let lw = u16::try_from(label.chars().count()).unwrap_or(14);
        if main_inner.width > lw + 2 {
            let ind_area = Rect::new(
                main_inner.x + main_inner.width - lw,
                main_inner.y + main_inner.height.saturating_sub(1),
                lw,
                1,
            );
            frame.render_widget(
                Paragraph::new(label).style(
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::DIM),
                ),
                ind_area,
            );
        }
    }

    // --- Footer / input block (rounded) ---
    let footer_title = if let Some(ref tool) = state.current_tool {
        let s = state.spinner_char();
        format!(" {s} Executing {tool} ")
    } else if state.busy {
        let s = state.spinner_char();
        format!(" {s} Thinking ")
    } else if state.pending_permission.is_some() {
        " ⚠ Permission ".to_string()
    } else {
        " Input ".to_string()
    };

    let footer_title_style = if state.busy {
        Style::default().fg(Color::LightCyan)
    } else if state.pending_permission.is_some() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Rgb(100, 140, 180))
    };

    let footer_border_color = if state.busy {
        Color::Rgb(30, 70, 100)
    } else if state.pending_permission.is_some() {
        Color::Rgb(120, 90, 30)
    } else {
        Color::Rgb(50, 70, 100)
    };

    let footer_block = Block::default()
        .title(footer_title)
        .title_style(footer_title_style)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(footer_border_color))
        .padding(ratatui::widgets::Padding::symmetric(1, 0));
    let footer_inner = footer_block.inner(input_area);
    state.last_input_rect = footer_inner;
    frame.render_widget(footer_block, input_area);

    frame.render_widget(Paragraph::new(Text::from(render_lines)), footer_inner);
    if max_scroll > 0 {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(" "))
            .thumb_symbol("▐")
            .style(Style::default().fg(Color::Rgb(60, 90, 120)));
        let mut scrollbar_state =
            ScrollbarState::new(max_scroll).position(state.input_scroll_offset);
        frame.render_stateful_widget(
            scrollbar,
            footer_inner.inner(Margin {
                vertical: 0,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

    draw_slash_overlay(frame, state, input_area, main_area);
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
            let mut g = cli.lock().expect("cli lock");
            let result = g.model_command(model)?;
            if result.persist_after {
                g.persist_session()?;
            }
            state.push_system_card("Model", &result.message);
        }
        SlashCommand::Permissions { mode } => {
            let mut g = cli.lock().expect("cli lock");
            let result = g.permissions_command(mode)?;
            if result.persist_after {
                g.persist_session()?;
            }
            state.push_system_card("Permissions", &result.message);
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
        SlashCommand::Memory => {
            let report = LiveCli::memory_report()?;
            state.push_system_card("Memory", &report);
        }
        SlashCommand::Diff => {
            let report = LiveCli::diff_report()?;
            state.push_system("Diff");
            state.push_system(&report);
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
        SlashCommand::Teleport { target } => {
            let report = cli
                .lock()
                .expect("cli lock")
                .teleport_report(target.as_deref())?;
            state.push_system("Teleport");
            state.push_system(&report);
        }
        SlashCommand::DebugToolCall => {
            let report = cli.lock().expect("cli lock").debug_tool_call_report()?;
            state.push_system("Debug Tool Call");
            state.push_system(&report);
        }
        SlashCommand::Headed | SlashCommand::NoHeadless => {
            std::env::set_var("HEADLESS", "false");
            state.push_system_card(
                "Browser Mode",
                "Browser mode\n  Result           switched to headed (visible)",
            );
        }
        SlashCommand::Headless => {
            std::env::set_var("HEADLESS", "true");
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
            state.active_modal = Some(AuthModal::new(ui_tx.clone(), parsed_provider));
            match parsed_provider {
                Some(crate::app::Provider::Anthropic) => {
                    spawn_anthropic_oauth_thread(ui_tx.clone(), &mut state.active_modal);
                }
                Some(crate::app::Provider::Codex) => {
                    spawn_codex_oauth_thread(ui_tx.clone(), &mut state.active_modal);
                }
                _ => {}
            }
        }
        other => {
            suspend_for_stdout(terminal, || {
                let mut g = cli.lock().expect("cli lock");
                let _ = g.handle_repl_command(other);
            })?;
            state.push_system("(slash command executed in classic output mode)");
        }
    }
    Ok(())
}

fn spawn_anthropic_oauth_thread(ui_tx: Sender<ReplTuiEvent>, active_modal: &mut Option<AuthModal>) {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    if let Some(ref mut modal) = active_modal {
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
                default_oauth_config, open_browser, wait_for_oauth_callback_cancellable,
            };
            use api::{AnthropicClient, AuthSource};
            use runtime::{
                generate_pkce_pair, generate_state, loopback_redirect_uri, save_oauth_credentials,
                OAuthAuthorizationRequest, OAuthTokenExchangeRequest, OAuthTokenSet,
            };

            let oauth = default_oauth_config();
            let callback_port = oauth.callback_port.unwrap_or(4545);
            let redirect_uri = loopback_redirect_uri(callback_port);
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
                message: format!("Waiting for OAuth callback on port {callback_port}…"),
            });
            let callback = wait_for_oauth_callback_cancellable(callback_port, cancel_rx)?;
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
            let client =
                AnthropicClient::from_auth(AuthSource::None).with_base_url(api::read_base_url());
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
            save_oauth_credentials(&OAuthTokenSet {
                access_token: token_set.access_token,
                refresh_token: token_set.refresh_token,
                expires_at: token_set.expires_at,
                scopes: token_set.scopes,
            })
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            Ok(())
        })();
        let _ = ui_tx.send(ReplTuiEvent::AuthOAuthComplete {
            provider: "anthropic".to_string(),
            result: result.map_err(|e| e.to_string()),
        });
    });
}

fn spawn_codex_oauth_thread(ui_tx: Sender<ReplTuiEvent>, active_modal: &mut Option<AuthModal>) {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    if let Some(ref mut modal) = active_modal {
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
            use crate::app::{open_browser, wait_for_oauth_callback_cancellable};
            use api::{AnthropicClient, AuthSource};
            use runtime::{OAuthTokenExchangeRequest, OAuthTokenSet};

            let login_request = api::codex_login().map_err(|e| {
                Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error + Send>
            })?;
            let port = login_request
                .config
                .callback_port
                .unwrap_or(api::CODEX_CALLBACK_PORT);
            let _ = ui_tx2.send(ReplTuiEvent::AuthOAuthProgress {
                message: "Opening browser for Codex login...".to_string(),
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
                message: format!("Waiting for Codex OAuth callback on port {port}…"),
            });
            let callback = wait_for_oauth_callback_cancellable(port, cancel_rx)?;
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
            api::save_codex_credentials(&OAuthTokenSet {
                access_token: token_set.access_token,
                refresh_token: token_set.refresh_token,
                expires_at: token_set.expires_at,
                scopes: token_set.scopes,
            })
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            Ok(())
        })();
        let _ = ui_tx.send(ReplTuiEvent::AuthOAuthComplete {
            provider: "codex".to_string(),
            result: result.map_err(|e| e.to_string()),
        });
    });
}

/// Interactive REPL using Ratatui when stdout is a TTY (unless `ACRAWL_CLASSIC_REPL` is set).
pub fn run_repl_ratatui(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let (ui_tx, ui_rx) = mpsc::channel::<ReplTuiEvent>();
    let (work_tx, work_rx) = mpsc::channel::<WorkerMsg>();

    let cli = Arc::new(Mutex::new(LiveCli::new_with_ui_tx(
        model,
        true,
        allowed_tools,
        permission_mode,
        ui_tx.clone(),
    )?));

    let cli_worker = Arc::clone(&cli);
    let ui_tx_worker = ui_tx.clone();
    thread::spawn(move || {
        while let Ok(msg) = work_rx.recv() {
            match msg {
                WorkerMsg::RunTurn(line) => {
                    let mut g = cli_worker.lock().expect("cli lock");
                    let prompter = ChannelPermissionPrompter::new(ui_tx_worker.clone());
                    let _ = g.run_turn_tui(&line, prompter);
                }
                WorkerMsg::Shutdown => break,
            }
        }
    });

    let mut terminal = ratatui::init();
    let work_shutdown = work_tx.clone();
    let result = run_loop(&mut terminal, &ui_rx, &ui_tx, &work_tx, &cli);
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
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = execute!(io::stdout(), event::EnableMouseCapture);
    let _mouse_guard = MouseCaptureGuard;

    let mut state = ReplTuiState::new();

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
            match state.ui_state {
                AppUiState::WelcomeMode => draw_welcome(frame, frame.area(), &mut state),
                AppUiState::ChatMode => draw_chat(frame, &mut state, &header),
            }
            if let Some((ref req, _)) = state.pending_permission {
                draw_permission_modal(frame, req);
            }
            if let Some(ref modal) = state.active_modal {
                modal.draw(frame, frame.area());
            }
        })?;

        // Advance typewriter: dynamic speed to catch up if buffer accumulates
        if !state.typewriter_chars.is_empty() {
            let q_len = state.typewriter_chars.len();
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
        let poll_ms = if state.typewriter_chars.is_empty() {
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
                if state.busy {
                    continue;
                }
                let in_transcript =
                    rect_contains_mouse(state.last_transcript_rect, me.column, me.row);
                let in_input = rect_contains_mouse(state.last_input_rect, me.column, me.row);

                if state.ui_state == AppUiState::ChatMode && in_transcript {
                    let max_off = state
                        .last_wrapped_len
                        .saturating_sub(state.last_view_height.max(1));
                    let cur = state.list_state.offset();
                    match me.kind {
                        MouseEventKind::ScrollUp => {
                            *state.list_state.offset_mut() = cur.saturating_sub(3);
                            state.follow_bottom = false;
                        }
                        MouseEventKind::ScrollDown => {
                            *state.list_state.offset_mut() = (cur.saturating_add(3)).min(max_off);
                            state.follow_bottom = state.list_state.offset() >= max_off;
                        }
                        _ => {}
                    }
                } else if in_input || state.ui_state == AppUiState::WelcomeMode {
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
                if let Some((req, respond)) = state.pending_permission.take() {
                    #[allow(clippy::unnested_or_patterns)]
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            let _ = respond.send(PermissionPromptDecision::Allow);
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            let _ = respond.send(PermissionPromptDecision::Deny {
                                reason: format!(
                                    "tool '{}' denied from TUI permission dialog",
                                    req.tool_name
                                ),
                            });
                        }
                        _ => {
                            state.pending_permission = Some((req, respond));
                        }
                    }
                    continue;
                }

                if let Some(ref mut modal) = state.active_modal {
                    let api_key_to_set: Option<String> = if key.code == KeyCode::Enter {
                        if let AuthModalStep::ApiKeyInput {
                            provider: crate::app::Provider::OpenAi,
                            ref key_buffer,
                            ..
                        } = modal.step
                        {
                            if key_buffer.is_empty() {
                                None
                            } else {
                                Some(key_buffer.clone())
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let action = modal.handle_key(key);

                    if let Some(api_key) = api_key_to_set {
                        if matches!(modal.step, AuthModalStep::Success { .. }) {
                            std::env::set_var("OPENAI_API_KEY", &api_key);
                        }
                    }

                    if let AuthModalStep::OAuthWaiting {
                        cancel_tx: None,
                        provider,
                        ..
                    } = &modal.step
                    {
                        let prov = *provider;
                        match prov {
                            crate::app::Provider::Anthropic => {
                                spawn_anthropic_oauth_thread(
                                    ui_tx.clone(),
                                    &mut state.active_modal,
                                );
                            }
                            crate::app::Provider::Codex => {
                                spawn_codex_oauth_thread(ui_tx.clone(), &mut state.active_modal);
                            }
                            crate::app::Provider::OpenAi => {}
                        }
                    }

                    match action {
                        ModalAction::Consumed => continue,
                        ModalAction::Dismiss => {
                            state.active_modal = None;
                            continue;
                        }
                        ModalAction::Passthrough => {}
                    }
                }

                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if state.busy {
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
                    if state.input.is_empty() {
                        state.exit = true;
                        state.persist_on_exit = true;
                    }
                    continue;
                }

                if key.code == KeyCode::Up && state.slash_overlay.is_some() {
                    if let Some(overlay) = state.slash_overlay.as_mut() {
                        if overlay.selected > 0 {
                            overlay.selected -= 1;
                            if overlay.selected < overlay.scroll_offset {
                                overlay.scroll_offset = overlay.selected;
                            }
                        }
                    }
                    continue;
                }

                if key.code == KeyCode::Down && state.slash_overlay.is_some() {
                    if let Some(overlay) = state.slash_overlay.as_mut() {
                        overlay.selected = min(overlay.selected + 1, overlay.items.len() - 1);
                        let visible_count = min(overlay.items.len(), 7);
                        if overlay.selected >= overlay.scroll_offset + visible_count {
                            overlay.scroll_offset = overlay.selected - visible_count + 1;
                        }
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        state.input.push('\n');
                        state.input_scroll_offset = usize::MAX;
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
                            let trimmed = state.input.trim().to_string();
                            if let Some(selected) = state.selected_slash_command() {
                                if selected != trimmed {
                                    state.input = selected;
                                    state.input.push(' ');
                                    state.input_scroll_offset = usize::MAX;
                                    state.wake_input_caret();
                                    state.refresh_slash_overlay();
                                    continue;
                                }
                            }
                        }

                        let line = std::mem::take(&mut state.input);
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
                        state.input.push('\n');
                        state.input_scroll_offset = usize::MAX;
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Char(c) => {
                        state.input.push(c);
                        state.input_scroll_offset = usize::MAX;
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Backspace => {
                        state.input.pop();
                        state.input_scroll_offset = usize::MAX;
                        state.wake_input_caret();
                        state.refresh_slash_overlay();
                    }
                    KeyCode::Tab => {
                        if state.busy || !state.input.trim_start().starts_with('/') {
                            continue;
                        }
                        if let Some(selected) = state.selected_slash_command() {
                            state.input = selected;
                            state.input.push(' ');
                            state.input_scroll_offset = usize::MAX;
                            state.wake_input_caret();
                            state.refresh_slash_overlay();
                        } else {
                            let prefix = state.input.to_ascii_lowercase();
                            let candidates = slash_command_completion_candidates();
                            let matches: Vec<_> = candidates
                                .into_iter()
                                .filter(|candidate| candidate.starts_with(&prefix))
                                .collect();
                            if matches.len() == 1 {
                                state.input.clone_from(&matches[0]);
                                state.input.push(' ');
                                state.input_scroll_offset = usize::MAX;
                                state.wake_input_caret();
                                state.refresh_slash_overlay();
                            }
                        }
                    }
                    KeyCode::Esc => {
                        state.slash_overlay = None;
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
    use crate::tui::auth_modal::{AuthModal, AuthModalStep};

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
                provider: Provider::OpenAi,
                ..
            }
        ));
        let modal2 = AuthModal::new(tx, Some(Provider::Anthropic));
        assert!(matches!(
            modal2.step,
            AuthModalStep::OAuthWaiting {
                provider: Provider::Anthropic,
                ..
            }
        ));
    }
}
