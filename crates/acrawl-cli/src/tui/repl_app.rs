//! Ratatui REPL with a welcome screen, sticky-bottom chat transcript, slash overlay, and floating input.

use std::cmp::min;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::render::PredictiveMarkdownBuffer;
use ansi_to_tui::IntoText;
use commands::{slash_command_specs, SlashCommand};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Padding,
    Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::DefaultTerminal;
use runtime::{format_usd, pricing_for_model};

use crate::app::{slash_command_completion_candidates, AllowedToolSet, LiveCli};
use crate::format::{render_repl_help, VERSION};
use crate::tui::active_modal::ActiveModal;
use crate::tui::auth_modal::{AuthModal, AuthModalStep};
use crate::tui::modal::{Modal, ModalAction};
use crate::tui::ReplTuiEvent;

const MAX_INPUT_LINES: usize = 5;
const WELCOME_BOX_SIDE_GUTTER: u16 = 16;
const WELCOME_BOX_MAX_WIDTH: u16 = 82;
const WELCOME_BOX_MIN_WIDTH: u16 = 30;
const INPUT_CARET_MARKER: char = '\u{E000}';
const SLASH_OVERLAY_VISIBLE_ITEMS: usize = 7;
const SLASH_OVERLAY_HINT_TEXT: &str = "Up/Down move  Enter accept  Tab complete  Esc close";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppUiState {
    WelcomeMode,
    ChatMode,
}

#[derive(Clone, Debug)]
enum ToolCallStatus {
    Running,
    Success { output: String },
    Error(String),
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
    ToolCall {
        name: String,
        input_summary: String,
        status: ToolCallStatus,
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
    session_id: String,
    cost_text: String,
    context_text: String,
    reasoning_effort: Option<String>,
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
        session_id: cli.session_id().to_string(),
        cost_text: format_usd(estimate.total_cost_usd()),
        context_text: format!("{} ctx", format_compact_tokens(usage.total_tokens())),
        reasoning_effort: cli.reasoning_effort().map(|e| e.as_str().to_string()),
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

fn ansi_to_lines(ansi: &str) -> Vec<Line<'static>> {
    let fallback_style = Style::default().fg(Color::Rgb(215, 225, 235));
    match ansi.as_bytes().into_text() {
        Ok(text) => text.lines,
        Err(_) => vec![Line::from(Span::styled(ansi.to_string(), fallback_style))],
    }
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

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    let mut in_csi = false;
    for c in s.chars() {
        if in_csi {
            if c.is_ascii_alphabetic() {
                in_csi = false;
                in_escape = false;
            }
        } else if in_escape {
            if c == '[' {
                in_csi = true;
            } else {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            result.push(c);
        }
    }
    result
}

fn extract_json_path(parsed: &serde_json::Value) -> String {
    parsed
        .get("file_path")
        .or_else(|| parsed.get("filePath"))
        .or_else(|| parsed.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string()
}

fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
}

fn cap_content_lines(lines: Vec<String>, max: usize) -> (Vec<ListItem<'static>>, Vec<String>) {
    let total = lines.len();
    let dim = Style::default().add_modifier(Modifier::DIM);
    if total <= max {
        let text_lines = lines;
        let items = text_lines
            .iter()
            .cloned()
            .map(|line| ListItem::new(Line::from(Span::styled(line, dim))))
            .collect();
        (items, text_lines)
    } else {
        let text_lines: Vec<String> = lines.into_iter().take(max).collect();
        let mut items: Vec<ListItem<'static>> = text_lines
            .iter()
            .cloned()
            .map(|line| ListItem::new(Line::from(Span::styled(line, dim))))
            .collect();
        let mut out_text = text_lines;
        let overflow = format!("  \u{2026} ({} more lines)", total - max);
        items.push(ListItem::new(Line::from(Span::styled(
            overflow.clone(),
            dim,
        ))));
        out_text.push(overflow);
        (items, out_text)
    }
}

fn render_bash_success(
    items: &mut Vec<ListItem<'static>>,
    text_lines: &mut Vec<String>,
    name: &str,
    input_summary: &str,
    parsed: &serde_json::Value,
) {
    let green = Style::default().fg(Color::Green);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let red = Style::default().fg(Color::Red);

    let cmd = serde_json::from_str::<serde_json::Value>(input_summary)
        .ok()
        .and_then(|v| {
            v.get("command")
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| input_summary.to_string());

    let cmd_display = if cmd.len() > 60 {
        format!("{}…", &cmd[..60])
    } else {
        cmd
    };

    let mut header_spans = vec![
        Span::styled("✓", green),
        Span::styled(format!(" {name} "), bold),
        Span::styled("$ ", dim),
        Span::styled(cmd_display, dim),
    ];
    if let Some(task_id) = parsed.get("backgroundTaskId").and_then(|v| v.as_str()) {
        header_spans.push(Span::styled(format!(" backgrounded ({task_id})"), dim));
    } else if let Some(interp) = parsed
        .get("returnCodeInterpretation")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        header_spans.push(Span::styled(format!(" {interp}"), dim));
    }
    let header_line = Line::from(header_spans);
    text_lines.push(line_to_plain_text(&header_line));
    items.push(ListItem::new(header_line));

    if let Some(stdout) = parsed.get("stdout").and_then(|v| v.as_str()) {
        let trimmed = stdout.trim_end();
        if !trimmed.is_empty() {
            let lines: Vec<String> = strip_ansi(trimmed).lines().map(str::to_string).collect();
            let (content_items, content_text) = cap_content_lines(lines, 15);
            items.extend(content_items);
            text_lines.extend(content_text);
        }
    }
    if let Some(stderr) = parsed.get("stderr").and_then(|v| v.as_str()) {
        let trimmed = stderr.trim_end();
        if !trimmed.is_empty() {
            for line in strip_ansi(trimmed).lines().take(5) {
                let line = line.to_string();
                items.push(ListItem::new(Line::from(Span::styled(line.clone(), red))));
                text_lines.push(line);
            }
        }
    }
}

fn render_read_success(
    items: &mut Vec<ListItem<'static>>,
    text_lines: &mut Vec<String>,
    name: &str,
    parsed: &serde_json::Value,
) {
    let file = parsed.get("file").unwrap_or(parsed);
    let path = extract_json_path(file);
    let start_line = file
        .get("startLine")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let num_lines = file
        .get("numLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total_lines = file
        .get("totalLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(num_lines);
    let end_line = start_line.saturating_add(num_lines.saturating_sub(1));
    let content = file
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let header_line = Line::from(vec![
        Span::styled("✓", Style::default().fg(Color::Green)),
        Span::styled(format!(" {name} "), bold),
        Span::styled(
            format!(
                "{path} (lines {start_line}-{} of {total_lines})",
                end_line.max(start_line)
            ),
            dim,
        ),
    ]);
    text_lines.push(line_to_plain_text(&header_line));
    items.push(ListItem::new(header_line));
    if !content.is_empty() {
        let lines: Vec<String> = strip_ansi(content.trim_end())
            .lines()
            .map(str::to_string)
            .collect();
        let (content_items, content_text) = cap_content_lines(lines, 15);
        items.extend(content_items);
        text_lines.extend(content_text);
    }
}

fn render_write_success(
    items: &mut Vec<ListItem<'static>>,
    text_lines: &mut Vec<String>,
    name: &str,
    parsed: &serde_json::Value,
) {
    let path = extract_json_path(parsed);
    let kind = parsed
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("write");
    let line_count = parsed
        .get("content")
        .and_then(|v| v.as_str())
        .map_or(0, |c| c.lines().count());
    let verb = if kind == "create" { "Wrote" } else { "Updated" };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let header_line = Line::from(vec![
        Span::styled("✓", Style::default().fg(Color::Green)),
        Span::styled(format!(" {name} "), bold),
        Span::styled(format!("{verb} {path} ({line_count} lines)"), dim),
    ]);
    text_lines.push(line_to_plain_text(&header_line));
    items.push(ListItem::new(header_line));
}

fn render_edit_success(
    items: &mut Vec<ListItem<'static>>,
    text_lines: &mut Vec<String>,
    name: &str,
    parsed: &serde_json::Value,
) {
    let path = extract_json_path(parsed);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let header_line = Line::from(vec![
        Span::styled("✓", Style::default().fg(Color::Green)),
        Span::styled(format!(" {name} "), bold),
        Span::styled(path.clone(), dim),
    ]);
    text_lines.push(line_to_plain_text(&header_line));
    items.push(ListItem::new(header_line));
    let mut diff_lines: Vec<ListItem<'static>> = Vec::new();
    let mut diff_text = Vec::new();
    if let Some(hunks) = parsed.get("structuredPatch").and_then(|v| v.as_array()) {
        for hunk in hunks.iter().take(2) {
            if let Some(lines) = hunk.get("lines").and_then(|v| v.as_array()) {
                for line_val in lines.iter().take(6) {
                    if let Some(line) = line_val.as_str() {
                        let line = line.to_string();
                        let style = match line.chars().next() {
                            Some('+') => Style::default().fg(Color::Green),
                            Some('-') => Style::default().fg(Color::Red),
                            _ => dim,
                        };
                        diff_lines
                            .push(ListItem::new(Line::from(Span::styled(line.clone(), style))));
                        diff_text.push(line);
                    }
                }
            }
        }
    } else {
        if let Some(old) = parsed.get("oldString").and_then(|v| v.as_str()) {
            let first_line = old.lines().find(|l| !l.trim().is_empty()).unwrap_or(old);
            if !first_line.is_empty() {
                let line = format!("- {first_line}");
                diff_lines.push(ListItem::new(Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(Color::Red),
                ))));
                diff_text.push(line);
            }
        }
        if let Some(new) = parsed.get("newString").and_then(|v| v.as_str()) {
            let first_line = new.lines().find(|l| !l.trim().is_empty()).unwrap_or(new);
            if !first_line.is_empty() {
                let line = format!("+ {first_line}");
                diff_lines.push(ListItem::new(Line::from(Span::styled(
                    line.clone(),
                    Style::default().fg(Color::Green),
                ))));
                diff_text.push(line);
            }
        }
    }
    items.extend(diff_lines);
    text_lines.extend(diff_text);
}

fn render_glob_success(
    items: &mut Vec<ListItem<'static>>,
    text_lines: &mut Vec<String>,
    name: &str,
    parsed: &serde_json::Value,
) {
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let header_line = Line::from(vec![
        Span::styled("✓", Style::default().fg(Color::Green)),
        Span::styled(format!(" {name} "), bold),
        Span::styled(format!("matched {num_files} files"), dim),
    ]);
    text_lines.push(line_to_plain_text(&header_line));
    items.push(ListItem::new(header_line));
    if let Some(filenames) = parsed.get("filenames").and_then(|v| v.as_array()) {
        for filename in filenames.iter().take(8).filter_map(|v| v.as_str()) {
            let filename = filename.to_string();
            items.push(ListItem::new(Line::from(Span::styled(
                filename.clone(),
                dim,
            ))));
            text_lines.push(filename);
        }
    }
}

fn render_grep_success(
    items: &mut Vec<ListItem<'static>>,
    text_lines: &mut Vec<String>,
    name: &str,
    output: &str,
    parsed: &serde_json::Value,
) {
    let num_matches = parsed
        .get("numMatches")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let header_line = Line::from(vec![
        Span::styled("✓", Style::default().fg(Color::Green)),
        Span::styled(format!(" {name} "), bold),
        Span::styled(
            format!("{num_matches} matches across {num_files} files"),
            dim,
        ),
    ]);
    text_lines.push(line_to_plain_text(&header_line));
    items.push(ListItem::new(header_line));
    let content = parsed
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !content.trim().is_empty() {
        let lines: Vec<String> = strip_ansi(content.trim_end())
            .lines()
            .map(str::to_string)
            .collect();
        let (content_items, content_text) = cap_content_lines(lines, 15);
        items.extend(content_items);
        text_lines.extend(content_text);
    } else if let Some(filenames) = parsed.get("filenames").and_then(|v| v.as_array()) {
        for filename in filenames.iter().take(8).filter_map(|v| v.as_str()) {
            let filename = filename.to_string();
            items.push(ListItem::new(Line::from(Span::styled(
                filename.clone(),
                dim,
            ))));
            text_lines.push(filename);
        }
    } else {
        let raw = strip_ansi(output.trim());
        if !raw.is_empty() {
            let lines: Vec<String> = raw.lines().map(str::to_string).collect();
            let (content_items, content_text) = cap_content_lines(lines, 15);
            items.extend(content_items);
            text_lines.extend(content_text);
        }
    }
}

fn render_tool_call_lines(
    name: &str,
    input_summary: &str,
    status: &ToolCallStatus,
    _width: u16,
    spinner: char,
) -> (Vec<ListItem<'static>>, Vec<String>) {
    let mut items = Vec::new();
    let mut text_lines = Vec::new();
    match status {
        ToolCallStatus::Running => {
            let spinner_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM);
            let name_style = Style::default().add_modifier(Modifier::BOLD);
            let param_style = Style::default().add_modifier(Modifier::DIM);
            let line = Line::from(vec![
                Span::styled(spinner.to_string(), spinner_style),
                Span::styled(format!(" {name} "), name_style),
                Span::styled(input_summary.to_string(), param_style),
            ]);
            text_lines.push(line_to_plain_text(&line));
            items.push(ListItem::new(line));
        }
        ToolCallStatus::Success { output } => {
            let parsed: serde_json::Value =
                serde_json::from_str(output).unwrap_or(serde_json::Value::String(output.clone()));
            match name {
                "bash" | "Bash" => {
                    render_bash_success(&mut items, &mut text_lines, name, input_summary, &parsed);
                }
                "read_file" | "Read" => {
                    render_read_success(&mut items, &mut text_lines, name, &parsed);
                }
                "write_file" | "Write" => {
                    render_write_success(&mut items, &mut text_lines, name, &parsed);
                }
                "edit_file" | "Edit" => {
                    render_edit_success(&mut items, &mut text_lines, name, &parsed);
                }
                "glob_search" | "Glob" => {
                    render_glob_success(&mut items, &mut text_lines, name, &parsed);
                }
                "grep_search" | "Grep" => {
                    render_grep_success(&mut items, &mut text_lines, name, output, &parsed);
                }
                _ => {
                    let summary = if output.trim().is_empty() {
                        "done".to_string()
                    } else {
                        let trimmed = strip_ansi(output.trim()).replace('\n', " ");
                        if trimmed.len() > 60 {
                            format!("{}…", &trimmed[..60])
                        } else {
                            trimmed
                        }
                    };
                    let line = Line::from(vec![
                        Span::styled("✓", Style::default().fg(Color::Green)),
                        Span::styled(
                            format!(" {name} "),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(summary, Style::default().add_modifier(Modifier::DIM)),
                    ]);
                    text_lines.push(line_to_plain_text(&line));
                    items.push(ListItem::new(line));
                }
            }
        }
        ToolCallStatus::Error(msg) => {
            let icon_style = Style::default().fg(Color::Red);
            let name_style = Style::default().add_modifier(Modifier::BOLD);
            let err_style = Style::default().fg(Color::Red);
            let truncated = if msg.len() > 120 {
                format!("{}…", &msg[..120])
            } else {
                msg.clone()
            };
            let line = Line::from(vec![
                Span::styled("✗", icon_style),
                Span::styled(format!(" {name} "), name_style),
                Span::styled(truncated, err_style),
            ]);
            text_lines.push(line_to_plain_text(&line));
            items.push(ListItem::new(line));
        }
    }
    debug_assert_eq!(items.len(), text_lines.len());
    (items, text_lines)
}

fn tool_input_summary(name: &str, input: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(input).unwrap_or(serde_json::Value::Null);
    let key_param = match name {
        "bash" | "Bash" => parsed
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or(input),
        "navigate" => parsed.get("url").and_then(|v| v.as_str()).unwrap_or(input),
        "read_file" | "Read" | "write_file" | "Write" | "edit_file" | "Edit" => parsed
            .get("file_path")
            .or_else(|| parsed.get("filePath"))
            .or_else(|| parsed.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or(input),
        "glob_search" | "Glob" | "grep_search" | "Grep" => parsed
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or(input),
        "click" | "scroll" | "hover" | "press_key" | "fill_form" | "select_option"
        | "switch_tab" | "wait" | "go_back" | "execute_js" | "screenshot" | "extract_data"
        | "list_resources" | "save_file" => "",
        _ => input,
    };
    if key_param.len() > 60 {
        format!("{}…", &key_param[..60])
    } else {
        key_param.to_string()
    }
}

#[allow(clippy::too_many_lines)]
fn build_wrapped_list(
    entries: &[TranscriptEntry],
    width: u16,
    live_text: Option<&str>,
    spinner_char: char,
) -> (Vec<ListItem<'static>>, Vec<String>) {
    let mut out = Vec::new();
    let mut text_out = Vec::new();
    // Restore the top padding margin
    out.push(ListItem::new(Line::from(" ")));
    text_out.push(" ".to_string());

    let system_style = Style::default().fg(Color::DarkGray).italic();
    let user_prefix_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    for entry in entries {
        match entry {
            TranscriptEntry::System(text) => {
                for row in wrap_plain_text(text, width) {
                    text_out.push(row.clone());
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
                    text_out.push(line_to_plain_text(&line));
                    out.push(ListItem::new(line).bg(user_bg));
                }
            }
            TranscriptEntry::Status(text) => {
                let status_style = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC);
                for row in wrap_plain_text(text, width) {
                    text_out.push(row.clone());
                    out.push(ListItem::new(Line::from(Span::styled(row, status_style))));
                }
            }
            TranscriptEntry::Stream(line) => {
                for wrapped in wrap_ansi_line(line.clone(), width) {
                    text_out.push(line_to_plain_text(&wrapped));
                    out.push(ListItem::new(wrapped));
                }
            }
            TranscriptEntry::SystemCard { title, rows } => {
                let border_style = Style::default().fg(Color::Yellow);
                let key_style = Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD);
                let w = usize::from(width);

                // Header: ┌─ title ───────┐
                let title_prefix = format!("┌─ {title} ");
                let title_fill = w
                    .saturating_sub(title_prefix.chars().count())
                    .saturating_sub(1);
                let header = format!("{title_prefix}{}┐", "─".repeat(title_fill));
                text_out.push(header.clone());
                out.push(ListItem::new(Line::from(Span::styled(
                    header,
                    border_style,
                ))));

                // Content rows: │ key  value   │
                let max_key_len = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
                let key_col = max_key_len.max(w.saturating_sub(8).clamp(8, 30));
                let val_col = w.saturating_sub(key_col + 5).max(8);

                for (key, value) in rows {
                    let wrapped = textwrap::wrap(value, val_col);
                    for (idx, line) in wrapped.into_iter().enumerate() {
                        let line_str = line.into_owned();
                        let pad = w.saturating_sub(2 + key_col + 1 + line_str.len() + 2);

                        let plain_line = if idx == 0 {
                            format!("│ {key:key_col$} {line_str}{} │", " ".repeat(pad))
                        } else {
                            format!("│ {} {line_str}{} │", " ".repeat(key_col), " ".repeat(pad),)
                        };
                        text_out.push(plain_line);

                        let spans = if idx == 0 {
                            vec![
                                Span::styled("│ ", border_style),
                                Span::styled(format!("{key:key_col$}"), key_style),
                                Span::raw(" "),
                                Span::raw(line_str),
                                Span::raw(" ".repeat(pad)),
                                Span::styled(" │", border_style),
                            ]
                        } else {
                            vec![
                                Span::styled("│ ", border_style),
                                Span::raw(" ".repeat(key_col)),
                                Span::raw(" "),
                                Span::raw(line_str),
                                Span::raw(" ".repeat(pad)),
                                Span::styled(" │", border_style),
                            ]
                        };
                        out.push(ListItem::new(Line::from(spans)));
                    }
                }

                // Footer: └───────────────┘
                let bottom_fill = w.saturating_sub(2);
                let bottom = format!("└{}┘", "─".repeat(bottom_fill));
                text_out.push(bottom.clone());
                out.push(ListItem::new(Line::from(Span::styled(
                    bottom,
                    border_style,
                ))));
            }
            TranscriptEntry::ToolCall {
                name,
                input_summary,
                status,
            } => {
                let (call_items, call_text) =
                    render_tool_call_lines(name, input_summary, status, width, spinner_char);
                out.extend(call_items);
                text_out.extend(call_text);
            }
        }

        // Add a blank separator line ONLY after User messages or Cards to separate blocks
        match entry {
            TranscriptEntry::User(_) | TranscriptEntry::SystemCard { .. } => {
                out.push(ListItem::new(Line::from(" ")));
                text_out.push(" ".to_string());
            }
            _ => {}
        }
    }

    // Live typewriter line shown at the bottom during streaming
    if let Some(text) = live_text {
        if !text.is_empty() {
            if let Ok(ansi_text) = text.as_bytes().into_text() {
                for line in ansi_text {
                    for wrapped_line in wrap_ansi_line(line, width) {
                        text_out.push(line_to_plain_text(&wrapped_line));
                        out.push(ListItem::new(wrapped_line));
                    }
                }
            } else {
                let live_style = Style::default().fg(Color::Rgb(215, 225, 235));
                for row in wrap_plain_text(text, width) {
                    text_out.push(row.clone());
                    out.push(ListItem::new(Line::from(Span::styled(row, live_style))));
                }
            }
        }
    }
    debug_assert_eq!(text_out.len(), out.len());
    (out, text_out)
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

fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0u32, u32::from);
        let b2 = chunk.get(2).copied().map_or(0u32, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn copy_osc52(text: &str) {
    let encoded = base64_encode(text.as_bytes());
    let _ = io::Write::write_all(
        &mut io::stdout(),
        format!("\x1b]52;c;{encoded}\x07").as_bytes(),
    );
    let _ = io::Write::flush(&mut io::stdout());
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
    input_cursor: usize,
    input_preferred_col: Option<usize>,
    status_line: String,
    busy: bool,
    #[allow(dead_code)]
    pending_model_after_auth: Option<String>,
    active_modal: Option<ActiveModal>,
    exit: bool,
    current_tool: Option<String>,
    status_entry_index: Option<usize>,
    tool_call_entry_index: Option<usize>,
    persist_on_exit: bool,
    cursor_on: bool,
    cursor_blink_deadline: Instant,
    slash_overlay: Option<SlashOverlay>,
    last_slash_overlay_rect: Option<Rect>,
    cached_header: HeaderSnapshot,
    spinner_tick: u8,
    spinner_deadline: Instant,
    /// Queue of plain-text chars waiting to be revealed by the typewriter.
    typewriter_chars: VecDeque<char>,
    /// The current line being built char-by-char (shown as live line).
    typewriter_live: String,
    /// ANSI-styled version of the current line, built by the predictive markdown buffer.
    typewriter_live_ansi: String,
    /// Streaming markdown state machine that produces styled ANSI as chars are revealed.
    md_buffer: PredictiveMarkdownBuffer,
    /// Mouse selection anchor (col screen-relative, row content-absolute).
    selection_anchor: Option<(u16, usize)>,
    /// Mouse selection moving end (col screen-relative, row content-absolute).
    selection_end: Option<(u16, usize)>,
    /// Set when right-click requests a copy of the current selection.
    pending_copy: Option<bool>,
    mouse_drag_occurred: bool,
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
            input: String::new(),
            input_cursor: 0,
            input_preferred_col: None,
            status_line: String::new(),
            busy: false,
            pending_model_after_auth: None,
            active_modal: None,
            exit: false,
            persist_on_exit: false,
            current_tool: None,
            status_entry_index: None,
            tool_call_entry_index: None,
            cursor_on: true,
            cursor_blink_deadline: Instant::now() + Duration::from_millis(530),
            slash_overlay: None,
            last_slash_overlay_rect: None,
            cached_header: HeaderSnapshot::default(),
            spinner_tick: 0,
            spinner_deadline: Instant::now() + Duration::from_millis(120),
            typewriter_chars: VecDeque::new(),
            typewriter_live: String::new(),
            typewriter_live_ansi: String::new(),
            md_buffer: PredictiveMarkdownBuffer::new(),
            selection_anchor: None,
            selection_end: None,
            pending_copy: None,
            mouse_drag_occurred: false,
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
        if let Some(modal) = self.active_modal.as_mut().map(ActiveModal::as_auth_mut) {
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
            match self.typewriter_chars.pop_front() {
                None => break,
                Some('\n') => {
                    self.md_buffer
                        .feed_char('\n', &mut self.typewriter_live_ansi);
                    let ansi = std::mem::take(&mut self.typewriter_live_ansi);
                    for styled_line in ansi_to_lines(&ansi) {
                        self.entries.push(TranscriptEntry::Stream(styled_line));
                    }
                    self.typewriter_live.clear();
                }
                Some(c) => {
                    self.typewriter_live.push(c);
                    self.md_buffer.feed_char(c, &mut self.typewriter_live_ansi);
                }
            }
        }
    }

    fn flush_typewriter(&mut self) {
        if !self.typewriter_chars.is_empty() {
            let count = self.typewriter_chars.len();
            self.tick_typewriter(count);
        }
        if !self.typewriter_live.is_empty() {
            self.md_buffer.flush(&mut self.typewriter_live_ansi);
            let ansi = std::mem::take(&mut self.typewriter_live_ansi);
            for styled_line in ansi_to_lines(&ansi) {
                self.entries.push(TranscriptEntry::Stream(styled_line));
            }
            self.typewriter_live.clear();
        }
    }

    fn wake_input_caret(&mut self) {
        self.cursor_on = true;
        self.cursor_blink_deadline = Instant::now() + Duration::from_millis(530);
    }

    fn input_char_len(&self) -> usize {
        self.input.chars().count()
    }

    fn input_char_to_byte(&self, char_idx: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_idx)
            .map_or(self.input.len(), |(idx, _)| idx)
    }

    fn clamp_input_cursor(&mut self) {
        self.input_cursor = self.input_cursor.min(self.input_char_len());
    }

    fn insert_input_char(&mut self, ch: char) {
        self.clamp_input_cursor();
        let idx = self.input_char_to_byte(self.input_cursor);
        self.input.insert(idx, ch);
        self.input_cursor = self.input_cursor.saturating_add(1);
        self.input_preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn backspace_input_char(&mut self) {
        self.clamp_input_cursor();
        if self.input_cursor == 0 {
            return;
        }
        let prev = self.input_cursor - 1;
        let start = self.input_char_to_byte(prev);
        let end = self.input_char_to_byte(prev + 1);
        self.input.replace_range(start..end, "");
        self.input_cursor -= 1;
        self.input_preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn delete_input_char(&mut self) {
        self.clamp_input_cursor();
        if self.input_cursor >= self.input_char_len() {
            return;
        }
        let start = self.input_char_to_byte(self.input_cursor);
        let end = self.input_char_to_byte(self.input_cursor + 1);
        self.input.replace_range(start..end, "");
        self.input_preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn input_cursor_line_col(&self) -> (usize, usize) {
        let mut line = 0usize;
        let mut col = 0usize;
        for (idx, ch) in self.input.chars().enumerate() {
            if idx == self.input_cursor {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    fn line_lengths(&self) -> Vec<usize> {
        let mut lengths = vec![0usize];
        for ch in self.input.chars() {
            if ch == '\n' {
                lengths.push(0);
            } else if let Some(last) = lengths.last_mut() {
                *last += 1;
            }
        }
        lengths
    }

    fn set_input_cursor_line_col(&mut self, target_line: usize, target_col: usize) {
        let lengths = self.line_lengths();
        let line = target_line.min(lengths.len().saturating_sub(1));
        let col = target_col.min(lengths[line]);
        let mut cursor = 0usize;
        for len in lengths.iter().take(line) {
            cursor += *len + 1;
        }
        cursor += col;
        self.input_cursor = cursor.min(self.input_char_len());
        self.input_scroll_offset = usize::MAX;
    }

    fn move_input_cursor_left(&mut self) {
        self.input_cursor = self.input_cursor.saturating_sub(1);
        self.input_preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn move_input_cursor_right(&mut self) {
        self.input_cursor = (self.input_cursor + 1).min(self.input_char_len());
        self.input_preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    fn move_input_cursor_home(&mut self) {
        let (line, _) = self.input_cursor_line_col();
        self.set_input_cursor_line_col(line, 0);
        self.input_preferred_col = Some(0);
    }

    fn move_input_cursor_end(&mut self) {
        let (line, _) = self.input_cursor_line_col();
        let target = self.line_lengths().get(line).copied().unwrap_or_default();
        self.set_input_cursor_line_col(line, target);
        self.input_preferred_col = Some(target);
    }

    fn move_input_cursor_up(&mut self) {
        let (line, col) = self.input_cursor_line_col();
        if line == 0 {
            self.set_input_cursor_line_col(0, 0);
            self.input_preferred_col = Some(0);
            return;
        }
        let target_col = self.input_preferred_col.unwrap_or(col);
        self.set_input_cursor_line_col(line - 1, target_col);
        self.input_preferred_col = Some(target_col);
    }

    fn move_input_cursor_down(&mut self) {
        let lengths = self.line_lengths();
        let (line, col) = self.input_cursor_line_col();
        if line + 1 >= lengths.len() {
            self.set_input_cursor_line_col(line, lengths[line]);
            self.input_preferred_col = Some(lengths[line]);
            return;
        }
        let target_col = self.input_preferred_col.unwrap_or(col);
        self.set_input_cursor_line_col(line + 1, target_col);
        self.input_preferred_col = Some(target_col);
    }

    fn handle_tool_call_start(&mut self, name: String, input: &str) {
        if !self.typewriter_chars.is_empty() {
            let count = self.typewriter_chars.len();
            self.tick_typewriter(count);
        }
        if !self.typewriter_live.is_empty() {
            self.md_buffer.flush(&mut self.typewriter_live_ansi);
            let ansi = std::mem::take(&mut self.typewriter_live_ansi);
            for styled_line in ansi_to_lines(&ansi) {
                self.entries.push(TranscriptEntry::Stream(styled_line));
            }
            self.typewriter_live.clear();
        }
        let input_summary = tool_input_summary(&name, input);
        self.ui_state = AppUiState::ChatMode;
        self.tool_call_entry_index = Some(self.entries.len());
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

        if let Some(idx) = self.tool_call_entry_index.take() {
            if let Some(TranscriptEntry::ToolCall {
                name: entry_name,
                status: entry_status,
                ..
            }) = self.entries.get_mut(idx)
            {
                if entry_name == name {
                    *entry_status = status.clone();
                    return;
                }
            }
        }

        if let Some(TranscriptEntry::ToolCall {
            status: entry_status,
            ..
        }) = self.entries.iter_mut().rev().find(
            |entry| matches!(entry, TranscriptEntry::ToolCall { name: entry_name, .. } if entry_name == name),
        ) {
            *entry_status = status;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn calculate_input_dimensions(
        &mut self,
        width: u16,
        model_label: &str,
    ) -> (u16, Vec<Line<'static>>, usize, Option<(u16, u16)>) {
        self.clamp_input_cursor();
        let is_placeholder = self.input.is_empty();
        let placeholder_text = self.input_placeholder();
        let mut input_with_caret = self.input.clone();
        if !is_placeholder {
            let caret_idx = self.input_char_to_byte(self.input_cursor);
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
                    if !is_placeholder && !seen_caret && current.contains(INPUT_CARET_MARKER) {
                        caret_row_idx = visual_lines.len();
                        seen_caret = true;
                    }
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
                let prompt_width = if has_prompt { 2 } else { 0 };
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
                    if !left.is_empty() {
                        spans.push(Span::styled(left, text_style));
                    }
                    if !right.is_empty() {
                        spans.push(Span::styled(right, text_style));
                    }
                    let prompt_width = if has_prompt { 2 } else { 0 };
                    let cursor_col =
                        prompt_width + u16::try_from(marker_char_idx).unwrap_or(u16::MAX);
                    cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), cursor_col));
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
        let trimmed = self.input.trim();
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

    fn clamp_scroll_offset(&mut self) {
        let max_offset = self
            .last_wrapped_len
            .saturating_sub(self.last_view_height.max(1));
        if self.list_state.offset() > max_offset {
            *self.list_state.offset_mut() = max_offset;
        }
    }

    fn scroll_to_bottom(&mut self) {
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
                        self.typewriter_chars.push_back(c);
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
                    if let Some(modal) = self.active_modal.as_mut().map(ActiveModal::as_auth_mut) {
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
                                store.active_provider = Some(provider_str.to_string());
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
                    if let Some(modal) = self.active_modal.as_mut().map(ActiveModal::as_auth_mut) {
                        if let AuthModalStep::OAuthWaiting { status, .. } = &mut modal.step {
                            *status = message;
                        }
                    }
                }
            }
        }
    }
}

fn suspend_for_stdout(terminal: &mut DefaultTerminal, f: impl FnOnce()) -> io::Result<()> {
    ratatui::try_restore()?;
    f();
    *terminal = ratatui::try_init()?;
    let _ = execute!(io::stdout(), event::EnableMouseCapture);
    Ok(())
}

fn draw_header(frame: &mut ratatui::Frame<'_>, area: Rect, header: &HeaderSnapshot) {
    let mut spans = vec![
        Span::styled(
            " ACrawl ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
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

#[allow(clippy::too_many_lines)]
fn draw_welcome(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &mut ReplTuiState,
    show_input_cursor: bool,
) {
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

    let input_w = area
        .width
        .saturating_sub(WELCOME_BOX_SIDE_GUTTER)
        .clamp(WELCOME_BOX_MIN_WIDTH, WELCOME_BOX_MAX_WIDTH);
    let model_label = state.cached_header.model.clone();
    let (box_height, render_lines, max_scroll, cursor_pos) =
        state.calculate_input_dimensions(input_w, &model_label);
    let input_h = box_height;

    let input_x = area.x + area.width.saturating_sub(input_w) / 2;
    let input_y = art_y.saturating_add(art_h).saturating_add(2);
    let input_area = Rect::new(
        input_x,
        input_y.min(area.y.saturating_add(area.height.saturating_sub(input_h))),
        input_w,
        input_h,
    );
    let goal_title = if let Some(ref effort) = state.cached_header.reasoning_effort {
        format!(" Goal · {} · {effort} ", state.cached_header.model)
    } else {
        format!(" Goal · {} ", state.cached_header.model)
    };
    let block = Block::default()
        .title(goal_title)
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
    if show_input_cursor {
        if let Some((row, col)) = cursor_pos {
            frame.set_cursor_position((inner.x.saturating_add(col), inner.y.saturating_add(row)));
        }
    }
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

    state.last_slash_overlay_rect = draw_slash_overlay(frame, state, input_area, area);
}

fn draw_slash_overlay(
    frame: &mut ratatui::Frame<'_>,
    state: &ReplTuiState,
    input_area: Rect,
    bounds: Rect,
) -> Option<Rect> {
    let Some(overlay) = &state.slash_overlay else {
        return None;
    };
    let total = overlay.items.len();
    let visible_count = min(total, SLASH_OVERLAY_VISIBLE_ITEMS);
    let scroll_offset = overlay.scroll_offset;

    let max_summary_w = overlay
        .items
        .iter()
        .map(|item| item.summary.chars().count())
        .max()
        .unwrap_or(0);
    let desired_content_w = (16 + max_summary_w).max(SLASH_OVERLAY_HINT_TEXT.chars().count());
    let desired_total_w = u16::try_from(desired_content_w.saturating_add(6)).unwrap_or(64);

    let list_rows = u16::try_from(visible_count).unwrap_or(1);
    let overlay_h = list_rows.saturating_add(5);
    let max_width = bounds.width.saturating_sub(2).max(1);
    let target_w = desired_total_w.clamp(44, 72);
    let overlay_w = target_w.min(max_width);
    let min_x = bounds.x.saturating_add(1);
    let max_x = bounds
        .x
        .saturating_add(bounds.width.saturating_sub(overlay_w.saturating_add(1)));
    let overlay_x = input_area
        .x
        .saturating_add(2)
        .clamp(min_x, max_x.max(min_x));
    let overlay_y = input_area
        .y
        .saturating_sub(overlay_h)
        .max(bounds.y.saturating_add(1));
    let overlay_area = Rect::new(overlay_x, overlay_y, overlay_w, overlay_h);

    frame.render_widget(Clear, overlay_area);
    let title = if total > visible_count {
        format!(" Slash Commands ({}/{}) ", overlay.selected + 1, total)
    } else {
        " Slash Commands ".to_string()
    };
    let overlay_bg = Color::Rgb(28, 31, 36);
    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(Color::LightCyan))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(90, 115, 132)))
        .padding(Padding::new(1, 1, 0, 0))
        .style(Style::default().bg(overlay_bg));
    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(list_rows),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let list_area = sections[1];
    let hint_area = sections[3];

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
                Span::styled(item.summary, Style::default().fg(Color::Rgb(188, 194, 200))),
            ]))
        })
        .collect::<Vec<_>>();
    let mut list_state = ListState::default();
    list_state.select(Some(overlay.selected.saturating_sub(scroll_offset)));
    let list = List::new(items)
        .style(Style::default().bg(overlay_bg))
        .highlight_style(Style::default().bg(Color::Rgb(52, 64, 79)))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, list_area, &mut list_state);

    let hint_line = Line::from(Span::styled(
        SLASH_OVERLAY_HINT_TEXT,
        Style::default()
            .fg(Color::Rgb(130, 136, 145))
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(Paragraph::new(hint_line), hint_area);

    Some(overlay_area)
}

#[allow(clippy::too_many_lines)]
fn draw_chat(
    frame: &mut ratatui::Frame<'_>,
    state: &mut ReplTuiState,
    header: &HeaderSnapshot,
    show_input_cursor: bool,
) {
    let area = frame.area();
    let (footer_h, render_lines, max_scroll, cursor_pos) =
        state.calculate_input_dimensions(area.width, &header.model);

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
        Some(state.typewriter_live_ansi.as_str())
    };
    let (wrapped, wrapped_text) = build_wrapped_list(
        &state.entries,
        main_inner.width,
        live_line,
        state.spinner_char(),
    );
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

    if let (Some(anchor), Some(end)) = (state.selection_anchor, state.selection_end) {
        let (s_start, s_end) = if (anchor.1, anchor.0) <= (end.1, end.0) {
            (anchor, end)
        } else {
            (end, anchor)
        };
        let scroll_off = state.list_state.offset();
        let viewport_h = usize::from(main_inner.height);
        let max_col = main_inner.width.saturating_sub(1);
        if s_end.1 >= scroll_off && s_start.1 < scroll_off + viewport_h {
            let vis_first = s_start.1.max(scroll_off) - scroll_off;
            let vis_last = (s_end.1 - scroll_off).min(viewport_h.saturating_sub(1));
            let highlight_bg = Color::Rgb(50, 80, 130);
            let buf = frame.buffer_mut();
            for screen_row in vis_first..=vis_last {
                let abs_row = scroll_off + screen_row;
                let c0 = if abs_row == s_start.1 { s_start.0 } else { 0 };
                let c1 = if abs_row == s_end.1 {
                    s_end.0.min(max_col)
                } else {
                    max_col
                };
                let y = main_inner.y + u16::try_from(screen_row).unwrap_or(0);
                for col in c0..=c1 {
                    let x = main_inner.x + col;
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_bg(highlight_bg);
                    }
                }
            }
        }

        if state.pending_copy.take().is_some() {
            let text = (s_start.1..=s_end.1)
                .filter_map(|row| wrapped_text.get(row).map(|line| (row, line)))
                .map(|(row, line)| {
                    let start = if row == s_start.1 {
                        usize::from(s_start.0)
                    } else {
                        0
                    };
                    let end = if row == s_end.1 {
                        usize::from(s_end.0) + 1
                    } else {
                        usize::MAX
                    };
                    line.chars()
                        .skip(start)
                        .take(end.saturating_sub(start))
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !text.trim().is_empty() {
                copy_osc52(&text);
            }
            state.selection_anchor = None;
            state.selection_end = None;
        }
    }

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
    } else if let Some(ref effort) = header.reasoning_effort {
        format!(" Input · {} · {effort} ", header.model)
    } else {
        format!(" Input · {} ", header.model)
    };

    let footer_title_style = if state.busy {
        Style::default().fg(Color::LightCyan)
    } else {
        Style::default().fg(Color::Rgb(100, 140, 180))
    };

    let footer_border_color = if state.busy {
        Color::Rgb(30, 70, 100)
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
    if show_input_cursor {
        if let Some((row, col)) = cursor_pos {
            frame.set_cursor_position((
                footer_inner.x.saturating_add(col),
                footer_inner.y.saturating_add(row),
            ));
        }
    }
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

    state.last_slash_overlay_rect = draw_slash_overlay(frame, state, input_area, main_area);
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
            cli.lock().expect("cli lock").reset_browser();
            state.push_system_card(
                "Browser Mode",
                "Browser mode\n  Result           switched to headed (visible)",
            );
        }
        SlashCommand::Headless => {
            std::env::set_var("HEADLESS", "true");
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
    if let Some(modal) = active_modal.as_mut().map(ActiveModal::as_auth_mut) {
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
            store.active_provider = Some("anthropic".to_string());
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
    if let Some(modal) = active_modal.as_mut().map(ActiveModal::as_auth_mut) {
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
            store.active_provider = Some("openai".to_string());
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
                            state.selection_anchor = Some((col, row));
                            state.selection_end = Some((col, row));
                            state.mouse_drag_occurred = false;
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
                            state.selection_end = Some((col, row));
                            state.mouse_drag_occurred = true;
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if state.mouse_drag_occurred {
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
                                state.selection_end = Some((col, row));
                            } else {
                                state.selection_anchor = None;
                                state.selection_end = None;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Right) => {
                            if state.selection_anchor.is_some() {
                                state.pending_copy = Some(true);
                            }
                        }
                        _ => {}
                    }
                } else if in_input || state.ui_state == AppUiState::WelcomeMode {
                    state.selection_anchor = None;
                    state.selection_end = None;
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
                    state.selection_anchor = None;
                    state.selection_end = None;
                }

                if let Some(ref mut modal) = state.active_modal {
                    let action = modal.handle_key(key);
                    let modal_succeeded =
                        matches!(modal.as_auth().step, AuthModalStep::Success { .. });
                    let oauth_provider = match &modal.as_auth().step {
                        AuthModalStep::OAuthWaiting {
                            cancel_tx: None,
                            provider,
                            ..
                        } => Some(*provider),
                        _ => None,
                    };

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
                    if state.input.is_empty() {
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
                            let trimmed = state.input.trim().to_string();
                            if let Some(selected) = state.selected_slash_command() {
                                if selected != trimmed {
                                    state.input = selected;
                                    state.input.push(' ');
                                    state.input_cursor = state.input.chars().count();
                                    state.input_preferred_col = None;
                                    state.input_scroll_offset = usize::MAX;
                                    state.wake_input_caret();
                                    state.refresh_slash_overlay();
                                    continue;
                                }
                            }
                        }

                        let line = std::mem::take(&mut state.input);
                        state.input_cursor = 0;
                        state.input_preferred_col = None;
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
                        if state.busy || !state.input.trim_start().starts_with('/') {
                            continue;
                        }
                        if let Some(selected) = state.selected_slash_command() {
                            state.input = selected;
                            state.input.push(' ');
                            state.input_cursor = state.input.chars().count();
                            state.input_preferred_col = None;
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
                                state.input_cursor = state.input.chars().count();
                                state.input_preferred_col = None;
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
    use crate::tui::auth_modal::{AuthModal, AuthModalStep, ProviderKind};
    use crate::tui::ReplTuiEvent;

    use super::{
        render_tool_call_lines, tool_input_summary, ReplTuiState, ToolCallStatus, TranscriptEntry,
    };

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
    fn tool_call_start_flushes_typewriter_first() {
        let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
        let mut state = ReplTuiState::new();

        for c in "hello\n".chars() {
            state.typewriter_chars.push_back(c);
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
        state.tool_call_entry_index = Some(0);

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
        assert!(state.tool_call_entry_index.is_none());
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
        state.tool_call_entry_index = Some(0);

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
        assert!(state.tool_call_entry_index.is_none());
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
