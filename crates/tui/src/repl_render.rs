use std::cmp::min;
use std::collections::HashMap;
use std::io;

use acrawl_core::message::{ContentBlock, ConversationMessage, MessageRole};
use crossterm::{event, execute};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Padding,
    Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::DefaultTerminal;
use runtime::{format_usd, pricing_for_model};
use serde_json;

use crate::app::LiveCli;
use crate::display_width::{split_at_display_width, text_display_width};
use crate::format::VERSION;
use crate::markdown::strip_ansi;
use crate::tool_format::{format_tool_success_line, tool_input_summary};
use crate::tool_pairing::{build_tool_result_index, ToolResultInfo};

use super::repl_app::{
    HeaderSnapshot, ReplTuiState, ToolCallStatus, SLASH_OVERLAY_HINT_TEXT,
    SLASH_OVERLAY_VISIBLE_ITEMS, WELCOME_BOX_MAX_WIDTH, WELCOME_BOX_MIN_WIDTH,
    WELCOME_BOX_SIDE_GUTTER,
};

pub(super) fn format_compact_tokens(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}m", f64::from(tokens) / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", f64::from(tokens) / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub(super) fn build_header_snapshot(cli: &LiveCli) -> HeaderSnapshot {
    let usage = cli.cumulative_usage();
    let pricing = pricing_for_model(cli.model_name());
    let estimate = pricing.map_or_else(
        || runtime::estimate_cost_usd(usage),
        |model_pricing| runtime::estimate_cost_usd_with_pricing(usage, model_pricing),
    );
    HeaderSnapshot {
        model: cli.model_name().to_string(),
        session_id: cli.session_id().to_string(),
        cost_text: format_usd(estimate.total_cost_usd()),
        context_text: format!("{} ctx", format_compact_tokens(usage.total_tokens())),
        reasoning_effort: cli.reasoning_effort().map(|e| e.as_str().to_string()),
    }
}

pub(super) fn wrap_plain_text(input: &str, width: u16) -> Vec<String> {
    let w = usize::from(width.max(8));
    textwrap::wrap(input, w)
        .into_iter()
        .map(std::borrow::Cow::into_owned)
        .collect()
}

pub(super) fn parse_report_rows(report: &str) -> Vec<(String, String)> {
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
pub(super) fn wrap_ansi_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
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
                let (split_idx, chunk_width) = split_at_display_width(&remaining_text, available);
                let head = remaining_text[..split_idx].to_string();
                remaining_text = remaining_text[split_idx..].to_string();

                current_line_spans.push(Span::styled(head, style));
                current_width += chunk_width;

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

pub(super) fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
}

pub(super) fn cap_content_lines(
    lines: Vec<String>,
    max: usize,
) -> (Vec<ListItem<'static>>, Vec<String>) {
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

pub(super) fn render_bash_success(
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

    let output_str = serde_json::to_string(parsed).unwrap_or_default();
    let tool_line = format_tool_success_line(name, input_summary, &output_str);

    let header_spans = vec![
        Span::styled("✓", green),
        Span::styled(format!(" {name} "), bold),
        Span::styled("$ ", dim),
        Span::styled(tool_line.summary, dim),
    ];
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

pub(super) fn render_tool_call_lines(
    name: &str,
    input_summary: &str,
    status: &ToolCallStatus,
    width: u16,
    spinner: char,
    debug_mode: bool,
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
        ToolCallStatus::Interrupted => {
            let line = Line::from(vec![
                Span::styled("◼", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!(" {name} "),
                    Style::default().add_modifier(Modifier::BOLD | Modifier::DIM),
                ),
                Span::styled("interrupted", Style::default().fg(Color::DarkGray)),
            ]);
            text_lines.push(line_to_plain_text(&line));
            items.push(ListItem::new(line));
        }
        ToolCallStatus::Success { output } => {
            if debug_mode {
                render_debug_success(&mut items, &mut text_lines, name, output, width);
            } else {
                let parsed: serde_json::Value = serde_json::from_str(output)
                    .unwrap_or(serde_json::Value::String(output.clone()));
                match name {
                    "bash" | "Bash" => {
                        render_bash_success(
                            &mut items,
                            &mut text_lines,
                            name,
                            input_summary,
                            &parsed,
                        );
                    }
                    _ => {
                        let tool_line = format_tool_success_line(name, input_summary, output);
                        let line = Line::from(vec![
                            Span::styled("✓", Style::default().fg(Color::Green)),
                            Span::styled(
                                format!(" {name} "),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                tool_line.summary,
                                Style::default().add_modifier(Modifier::DIM),
                            ),
                        ]);
                        text_lines.push(line_to_plain_text(&line));
                        items.push(ListItem::new(line));
                    }
                }
            }
        }
        ToolCallStatus::Error(msg) => {
            let icon_style = Style::default().fg(Color::Red);
            let name_style = Style::default().add_modifier(Modifier::BOLD);
            let err_style = Style::default().fg(Color::Red);
            let header = Line::from(vec![
                Span::styled("✗", icon_style),
                Span::styled(format!(" {name} "), name_style),
            ]);
            text_lines.push(line_to_plain_text(&header));
            items.push(ListItem::new(header));
            for row in wrap_plain_text(msg, width) {
                text_lines.push(row.clone());
                items.push(ListItem::new(Line::from(Span::styled(row, err_style))));
            }
        }
    }
    debug_assert_eq!(items.len(), text_lines.len());
    (items, text_lines)
}

fn render_debug_success(
    items: &mut Vec<ListItem<'static>>,
    text_lines: &mut Vec<String>,
    name: &str,
    output: &str,
    width: u16,
) {
    let header = Line::from(vec![
        Span::styled("✓", Style::default().fg(Color::Green)),
        Span::styled(
            format!(" {name} "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "[debug]",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        ),
    ]);
    text_lines.push(line_to_plain_text(&header));
    items.push(ListItem::new(header));

    let filtered = filter_base64_fields(output);
    let wrapped: Vec<String> = filtered
        .lines()
        .flat_map(|l| {
            let indented = format!("  {l}");
            wrap_plain_text(&indented, width)
        })
        .collect();
    let (capped_items, capped_text) = cap_content_lines(wrapped, 80);
    items.extend(capped_items);
    text_lines.extend(capped_text);
}

fn filter_base64_fields(output: &str) -> String {
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(output) else {
        return output.to_string();
    };
    if let Some(obj) = val.as_object_mut() {
        for (key, v) in obj.iter_mut() {
            if let Some(s) = v.as_str() {
                if key.contains("base64") || (s.len() > 256 && looks_like_base64(s)) {
                    *v = serde_json::Value::String(format!("<{} bytes>", s.len()));
                }
            }
        }
    }
    serde_json::to_string_pretty(&val).unwrap_or_else(|_| output.to_string())
}

fn looks_like_base64(s: &str) -> bool {
    s.len() > 256
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=' || b == b'\n')
}

#[allow(clippy::too_many_lines)]
#[must_use]
pub fn build_wrapped_list<S: ::std::hash::BuildHasher>(
    messages: &[ConversationMessage],
    tool_results: &HashMap<String, ToolResultInfo, S>,
    live_tool_calls: &[(String, String, crate::repl_app::ToolCallStatus)],
    width: u16,
    live_text: Option<&str>,
    spinner_char: char,
    debug_mode: bool,
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
    for message in messages {
        match message.role {
            MessageRole::User => {
                let text_blocks = message.blocks.iter().filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text),
                    _ => None,
                });

                let user_bg = Color::Rgb(35, 45, 60); // Subtle blue-gray background
                for text in text_blocks {
                    let prefixed = format!("  You {text}");
                    let rows = wrap_plain_text(&prefixed, width);
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

                    out.push(ListItem::new(Line::from(" ")));
                    text_out.push(" ".to_string());
                }
            }
            MessageRole::Assistant => {
                for block in &message.blocks {
                    match block {
                        ContentBlock::Text { text } => {
                            for line in crate::markdown::render_lines(text) {
                                for wrapped in wrap_ansi_line(line, width) {
                                    text_out.push(line_to_plain_text(&wrapped));
                                    out.push(ListItem::new(wrapped));
                                }
                            }
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let input_summary = tool_input_summary(name, input);
                            let status = match tool_results.get(id) {
                                Some(result) if result.is_error => {
                                    ToolCallStatus::Error(result.output.clone())
                                }
                                Some(result) => ToolCallStatus::Success {
                                    output: result.output.clone(),
                                },
                                None if !live_tool_calls.is_empty() => {
                                    // Rendered via live_tool_calls below; skip duplicate.
                                    continue;
                                }
                                None => ToolCallStatus::Interrupted,
                            };
                            let (call_items, call_text) = render_tool_call_lines(
                                name,
                                &input_summary,
                                &status,
                                width,
                                spinner_char,
                                debug_mode,
                            );
                            out.extend(call_items);
                            text_out.extend(call_text);
                        }
                        ContentBlock::Reasoning { data } => {
                            let thinking_text = serde_json::from_str::<serde_json::Value>(data)
                                .ok()
                                .and_then(|v| {
                                    v.get("reasoning_content")?.as_str().map(String::from)
                                })
                                .unwrap_or_else(|| data.clone());
                            for row in wrap_plain_text(&thinking_text, width) {
                                text_out.push(row.clone());
                                out.push(ListItem::new(Line::from(Span::styled(
                                    row,
                                    system_style,
                                ))));
                            }
                        }
                        ContentBlock::ToolResult { .. } => {}
                        ContentBlock::ToolResultImage { .. } => {}
                    }
                }
            }
            MessageRole::Tool => {}
            MessageRole::System => {
                for block in &message.blocks {
                    if let ContentBlock::Text { text } = block {
                        for row in wrap_plain_text(text, width) {
                            text_out.push(row.clone());
                            out.push(ListItem::new(Line::from(Span::styled(row, system_style))));
                        }
                        out.push(ListItem::new(Line::from(" ")));
                        text_out.push(" ".to_string());
                    }
                }
            }
        }
    }

    for (name, input_summary, status) in live_tool_calls {
        let (call_items, call_text) =
            render_tool_call_lines(name, input_summary, status, width, spinner_char, debug_mode);
        out.extend(call_items);
        text_out.extend(call_text);
    }

    // Live typewriter line shown at the bottom during streaming
    if let Some(text) = live_text {
        if !text.is_empty() {
            for line in crate::markdown::render_lines(text) {
                for wrapped_line in wrap_ansi_line(line, width) {
                    text_out.push(line_to_plain_text(&wrapped_line));
                    out.push(ListItem::new(wrapped_line));
                }
            }
        }
    }
    debug_assert_eq!(text_out.len(), out.len());
    (out, text_out)
}

pub(super) fn build_child_entry_list(
    entries: &[super::child_tabs::TranscriptEntry],
    live: &str,
    width: u16,
    spinner_char: char,
    debug_mode: bool,
) -> (Vec<ListItem<'static>>, Vec<String>) {
    use super::child_tabs::TranscriptEntry;

    let mut out: Vec<ListItem<'static>> = Vec::new();
    let mut text_out: Vec<String> = Vec::new();

    out.push(ListItem::new(Line::from(" ")));
    text_out.push(" ".to_string());

    let system_style = Style::default().fg(Color::DarkGray).italic();
    let user_prefix_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let user_bg = Color::Rgb(35, 45, 60);

    for entry in entries {
        match entry {
            TranscriptEntry::System(text) | TranscriptEntry::Status(text) => {
                for row in wrap_plain_text(text, width) {
                    text_out.push(row.clone());
                    out.push(ListItem::new(Line::from(Span::styled(row, system_style))));
                }
            }
            TranscriptEntry::User(text) | TranscriptEntry::Parent(text) => {
                let prefixed = format!("  ▸ {text}");
                let rows = wrap_plain_text(&prefixed, width);
                for (idx, row) in rows.into_iter().enumerate() {
                    let row_line = if idx == 0 && row.trim_start().starts_with("▸ ") {
                        let trimmed = row.trim_start();
                        let rest = trimmed.get(4..).unwrap_or("").to_string();
                        Line::from(vec![
                            Span::raw("  "),
                            Span::styled("▸ ", user_prefix_style),
                            Span::raw(rest),
                        ])
                    } else {
                        Line::from(Span::raw(row))
                    };
                    text_out.push(line_to_plain_text(&row_line));
                    out.push(ListItem::new(row_line).bg(user_bg));
                }
                out.push(ListItem::new(Line::from(" ")));
                text_out.push(" ".to_string());
            }
            TranscriptEntry::Stream(styled) => {
                for wrapped in wrap_ansi_line(styled.clone(), width) {
                    text_out.push(line_to_plain_text(&wrapped));
                    out.push(ListItem::new(wrapped));
                }
            }
            TranscriptEntry::SystemCard { title, rows } => {
                render_system_card(title, rows, system_style, &mut out, &mut text_out);
            }
            TranscriptEntry::ToolCall {
                name,
                input_summary,
                status,
            } => {
                let (call_items, call_text) = render_tool_call_lines(
                    name,
                    input_summary,
                    status,
                    width,
                    spinner_char,
                    debug_mode,
                );
                out.extend(call_items);
                text_out.extend(call_text);
            }
        }
    }

    if !live.is_empty() {
        for line in crate::markdown::render_lines(live) {
            for wrapped_line in wrap_ansi_line(line, width) {
                text_out.push(line_to_plain_text(&wrapped_line));
                out.push(ListItem::new(wrapped_line));
            }
        }
    }

    debug_assert_eq!(text_out.len(), out.len());
    (out, text_out)
}

fn render_system_card(
    title: &str,
    rows: &[(String, String)],
    system_style: Style,
    out: &mut Vec<ListItem<'static>>,
    text_out: &mut Vec<String>,
) {
    let header_line = Line::from(vec![
        Span::styled("╭─ ", system_style),
        Span::styled(
            title.to_owned(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    text_out.push(line_to_plain_text(&header_line));
    out.push(ListItem::new(header_line));
    for (k, v) in rows {
        let row_line = Line::from(vec![
            Span::styled("│ ", system_style),
            Span::styled(
                format!("{k}: "),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::raw(v.clone()),
        ]);
        text_out.push(line_to_plain_text(&row_line));
        out.push(ListItem::new(row_line));
    }
    let footer_line = Line::from(Span::styled("╰─", system_style));
    text_out.push(line_to_plain_text(&footer_line));
    out.push(ListItem::new(footer_line));
    out.push(ListItem::new(Line::from(" ")));
    text_out.push(" ".to_string());
}

pub(super) fn rect_contains_mouse(r: Rect, col: u16, row: u16) -> bool {
    if r.width == 0 || r.height == 0 {
        return false;
    }
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

pub(super) fn base64_encode(input: &[u8]) -> String {
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

pub(super) fn copy_osc52(text: &str) {
    let encoded = base64_encode(text.as_bytes());
    let _ = io::Write::write_all(
        &mut io::stdout(),
        format!("\x1b]52;c;{encoded}\x07").as_bytes(),
    );
    let _ = io::Write::flush(&mut io::stdout());
}

pub(super) fn suspend_for_stdout(
    terminal: &mut DefaultTerminal,
    f: impl FnOnce(),
) -> io::Result<()> {
    ratatui::try_restore()?;
    f();
    *terminal = ratatui::try_init()?;
    let _ = execute!(io::stdout(), event::EnableMouseCapture);
    Ok(())
}

pub(super) fn draw_header(frame: &mut ratatui::Frame<'_>, area: Rect, header: &HeaderSnapshot) {
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
    let right_w = text_display_width(&right_text);
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
pub(super) fn draw_welcome(
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
    let version_str = format!("v{VERSION} · CloakBrowser ready");
    let max_w = ascii
        .iter()
        .map(|s| text_display_width(s))
        .max()
        .unwrap_or(0);
    let pad = max_w.saturating_sub(text_display_width(&version_str)) / 2;
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

    let mut input_y = art_y.saturating_add(art_h).saturating_add(2);

    if let Some(ref info) = state.update_info {
        if info.is_outdated {
            let install_cmd = if cfg!(target_os = "windows") {
                "  Run: irm https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.ps1 | iex".to_string()
            } else {
                "  Run: curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | sh".to_string()
            };
            let card_text = vec![
                Line::from(Span::styled(
                    format!(
                        "  Update available: v{} (you have v{})",
                        info.latest_version, info.current_version
                    ),
                    Style::default().fg(Color::Yellow),
                )),
                Line::from(Span::styled(
                    install_cmd,
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            let card = Paragraph::new(card_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().add_modifier(Modifier::DIM)),
            );

            let card_h = 4;
            let card_w = art_w.max(60);
            let card_x = area.x + area.width.saturating_sub(card_w) / 2;
            let card_y = art_y.saturating_add(art_h).saturating_add(1);
            let card_area = Rect::new(
                card_x,
                card_y,
                card_w.min(area.width),
                card_h.min(area.height),
            );
            frame.render_widget(card, card_area);

            input_y = card_y.saturating_add(card_h).saturating_add(1);
        }
    }

    let input_x = area.x + area.width.saturating_sub(input_w) / 2;
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
        .padding(Padding::symmetric(1, 0));
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

pub(super) fn draw_slash_overlay(
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
        .map(|item| text_display_width(item.summary))
        .max()
        .unwrap_or(0);
    let desired_content_w = (16 + max_summary_w).max(text_display_width(SLASH_OVERLAY_HINT_TEXT));
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
pub(super) fn draw_child_view(frame: &mut ratatui::Frame<'_>, state: &mut ReplTuiState) {
    let child_id = match &state.view_mode {
        super::repl_app::ViewMode::Child(id) => id.clone(),
        super::repl_app::ViewMode::Parent => return,
    };

    let spinner = state.spinner_char();
    let debug_mode = state.debug_mode;
    let tab_idx = state
        .child_tab_panel
        .tabs
        .iter()
        .position(|t| t.child_id == child_id)
        .unwrap_or(0);
    let total_tabs = state.child_tab_panel.tabs.len();

    let area = frame.area();
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(4),
            ratatui::layout::Constraint::Length(1),
        ])
        .split(area);
    let header_area = chunks[0];
    let main_area = chunks[1];
    let footer_area = chunks[2];

    let Some(tab) = state.child_tab_panel.find_tab_mut(&child_id) else {
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(
                ratatui::style::Style::default().fg(ratatui::style::Color::Rgb(50, 65, 90)),
            );
        let inner = block.inner(main_area);
        frame.render_widget(block, main_area);
        frame.render_widget(ratatui::widgets::Paragraph::new("Child not found"), inner);
        return;
    };

    let status_text = match &tab.status {
        super::child_tabs::ChildTabStatus::Running => {
            if let Some(ref tool) = tab.tool_in_progress {
                format!("running {tool} -- step {}/{}", tab.step, tab.max_steps)
            } else {
                format!("step {}/{}", tab.step, tab.max_steps)
            }
        }
        super::child_tabs::ChildTabStatus::Done => {
            format!("✓ Done -- {} items extracted", tab.items_extracted)
        }
        super::child_tabs::ChildTabStatus::Error(e) => format!("✗ Error: {e}"),
    };

    let status_color = match &tab.status {
        super::child_tabs::ChildTabStatus::Running => ratatui::style::Color::Cyan,
        super::child_tabs::ChildTabStatus::Done => ratatui::style::Color::Green,
        super::child_tabs::ChildTabStatus::Error(_) => ratatui::style::Color::Red,
    };

    let header_spans = vec![
        ratatui::text::Span::styled(
            " Child ",
            ratatui::style::Style::default()
                .fg(ratatui::style::Color::Cyan)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        ratatui::text::Span::styled(
            format!("  {status_text} "),
            ratatui::style::Style::default().fg(status_color),
        ),
        ratatui::text::Span::raw("  "),
        ratatui::text::Span::styled(
            format!("{} of {}", tab_idx + 1, total_tabs),
            ratatui::style::Style::default()
                .fg(ratatui::style::Color::LightBlue)
                .add_modifier(ratatui::style::Modifier::DIM),
        ),
    ];
    frame.render_widget(
        ratatui::widgets::Paragraph::new(ratatui::text::Line::from(header_spans))
            .style(ratatui::style::Style::default().bg(ratatui::style::Color::Rgb(14, 18, 28))),
        header_area,
    );

    let border_color = match &tab.status {
        super::child_tabs::ChildTabStatus::Running => ratatui::style::Color::Rgb(40, 80, 110),
        super::child_tabs::ChildTabStatus::Done => ratatui::style::Color::Rgb(40, 100, 60),
        super::child_tabs::ChildTabStatus::Error(_) => ratatui::style::Color::Rgb(140, 40, 40),
    };
    let main_block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(ratatui::style::Style::default().fg(border_color));
    let main_inner = main_block.inner(main_area);
    frame.render_widget(main_block, main_area);

    state.last_transcript_rect = main_inner;

    let (wrapped, wrapped_text) = build_child_entry_list(
        &tab.entries,
        &tab.live,
        main_inner.width,
        spinner,
        debug_mode,
    );

    let scroll_offset = {
        tab.last_wrapped_len = wrapped.len();
        tab.last_view_height = usize::from(main_inner.height.max(1));

        let max_offset = tab
            .last_wrapped_len
            .saturating_sub(tab.last_view_height.max(1));
        if tab.list_state.offset() > max_offset {
            *tab.list_state.offset_mut() = max_offset;
        }
        if tab.follow_bottom {
            *tab.list_state.offset_mut() = max_offset;
        }

        let list = List::new(wrapped)
            .highlight_spacing(HighlightSpacing::Never)
            .scroll_padding(2);
        frame.render_stateful_widget(list, main_inner, &mut tab.list_state);

        tab.list_state.offset()
    };

    if let (Some(anchor), Some(end)) = (state.selection.anchor, state.selection.end) {
        let (s_start, s_end) = if (anchor.1, anchor.0) <= (end.1, end.0) {
            (anchor, end)
        } else {
            (end, anchor)
        };
        let viewport_h = usize::from(main_inner.height);
        let max_col = main_inner.width.saturating_sub(1);
        if s_end.1 >= scroll_offset && s_start.1 < scroll_offset + viewport_h {
            let vis_first = s_start.1.max(scroll_offset) - scroll_offset;
            let vis_last = (s_end.1 - scroll_offset).min(viewport_h.saturating_sub(1));
            let highlight_bg = ratatui::style::Color::Rgb(50, 80, 130);
            let buf = frame.buffer_mut();
            for screen_row in vis_first..=vis_last {
                let abs_row = scroll_offset + screen_row;
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

        if state.selection.pending_copy.take().is_some() {
            let text = (s_start.1..=s_end.1)
                .filter_map(|row| wrapped_text.get(row).map(|line| (row, line)))
                .map(|(row, line)| {
                    let start = if row == s_start.1 {
                        usize::from(s_start.0)
                    } else {
                        0
                    };
                    let end_col = if row == s_end.1 {
                        usize::from(s_end.0) + 1
                    } else {
                        usize::MAX
                    };
                    line.chars()
                        .skip(start)
                        .take(end_col.saturating_sub(start))
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !text.trim().is_empty() {
                copy_osc52(&text);
            }
            state.selection.anchor = None;
            state.selection.end = None;
        }
    }

    let footer_spans = vec![
        ratatui::text::Span::styled(
            " ←",
            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
        ),
        ratatui::text::Span::styled(
            "Prev",
            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
        ),
        ratatui::text::Span::styled(
            "  →",
            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
        ),
        ratatui::text::Span::styled(
            "Next",
            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
        ),
        ratatui::text::Span::styled(
            "  Esc/↑",
            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
        ),
        ratatui::text::Span::styled(
            "Parent",
            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
        ),
        ratatui::text::Span::styled(
            "  j/k",
            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
        ),
        ratatui::text::Span::styled(
            "Scroll",
            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
        ),
        ratatui::text::Span::styled(
            "  Enter",
            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
        ),
        ratatui::text::Span::styled(
            "Resume",
            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
        ),
    ];
    frame.render_widget(
        ratatui::widgets::Paragraph::new(ratatui::text::Line::from(footer_spans))
            .style(ratatui::style::Style::default().bg(ratatui::style::Color::Rgb(14, 18, 28))),
        footer_area,
    );
}

#[allow(clippy::too_many_lines)]
pub(super) fn draw_chat(
    frame: &mut ratatui::Frame<'_>,
    state: &mut ReplTuiState,
    header: &HeaderSnapshot,
    show_input_cursor: bool,
) {
    let area = frame.area();
    let (footer_h, render_lines, max_scroll, cursor_pos) =
        state.calculate_input_dimensions(area.width, &header.model);

    let has_children = !state.child_tab_panel.tabs.is_empty();
    let hint_h: u16 = u16::from(has_children);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(hint_h),
            Constraint::Length(1),
            Constraint::Length(footer_h),
        ])
        .split(area);
    let header_area = chunks[0];
    let main_area = chunks[1];
    let hint_area = chunks[2];
    let input_area = chunks[4];

    draw_header(frame, header_area, header);

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

    let live_line = if state.typewriter.live.is_empty() {
        None
    } else {
        Some(state.typewriter.live.as_str())
    };
    let tool_results = build_tool_result_index(&state.messages);
    let (wrapped, wrapped_text) = build_wrapped_list(
        &state.messages,
        &tool_results,
        &state.live_tool_calls,
        main_inner.width,
        live_line,
        state.spinner_char(),
        state.debug_mode,
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

    if has_children {
        let running_count = state
            .child_tab_panel
            .tabs
            .iter()
            .filter(|t| matches!(t.status, crate::tui::child_tabs::ChildTabStatus::Running))
            .count();
        let hint_line = Line::from(Span::styled(
            format!("Ctrl+X view children ({running_count} running)"),
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(Paragraph::new(hint_line), hint_area);
    }

    if let (Some(anchor), Some(end)) = (state.selection.anchor, state.selection.end) {
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

        if state.selection.pending_copy.take().is_some() {
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
            state.selection.anchor = None;
            state.selection.end = None;
        }
    }

    // Busy indicator overlay at bottom-right of transcript
    if state.busy {
        let spinner = state.spinner_char();
        let label = if state.cancelling {
            format!(" {spinner} Interrupting… ")
        } else {
            format!(" {spinner} Generating… ")
        };
        let lw = u16::try_from(text_display_width(&label)).unwrap_or(14);
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
    let footer_title = if state.cancelling {
        let s = state.spinner_char();
        format!(" {s} Interrupting… ")
    } else if let Some(ref tool) = state.current_tool {
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
        .padding(Padding::symmetric(1, 0));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_lines(items: &[String]) -> Vec<String> {
        items.to_vec()
    }

    #[test]
    fn build_wrapped_list_renders_user_message() {
        let messages = vec![ConversationMessage::user_text("hello")];

        let (_, text) = build_wrapped_list(&messages, &HashMap::new(), &[], 80, None, '⠋', false);

        assert!(plain_lines(&text)
            .iter()
            .any(|line| line.contains("You hello")));
    }

    #[test]
    fn build_wrapped_list_renders_assistant_text() {
        let messages = vec![ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "assistant reply".to_string(),
        }])];

        let (_, text) = build_wrapped_list(&messages, &HashMap::new(), &[], 80, None, '⠋', false);

        assert!(text.iter().any(|line| line.contains("assistant reply")));
    }

    #[test]
    fn build_wrapped_list_renders_tool_success() {
        let messages = vec![ConversationMessage::assistant(vec![
            ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "navigate".to_string(),
                input: r#"{"url":"https://example.com"}"#.to_string(),
            },
        ])];
        let tool_results = HashMap::from([(
            "tool_1".to_string(),
            ToolResultInfo {
                output: "navigation complete".to_string(),
                is_error: false,
            },
        )]);

        let (_, text) = build_wrapped_list(&messages, &tool_results, &[], 80, None, '⠋', false);

        assert!(text.iter().any(|line| line.contains("navigate")));
    }

    #[test]
    fn build_wrapped_list_renders_tool_interrupted() {
        let messages = vec![ConversationMessage::assistant(vec![
            ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "click".to_string(),
                input: r##"{"selector":"#go"}"##.to_string(),
            },
        ])];

        let (_, text) = build_wrapped_list(&messages, &HashMap::new(), &[], 80, None, '⠋', false);

        assert!(text.iter().any(|line| line.contains("interrupted")));
    }

    #[test]
    fn build_wrapped_list_skips_unresolved_tool_use_when_live_tools_active() {
        let messages = vec![ConversationMessage::assistant(vec![
            ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "navigate".to_string(),
                input: r#"{"url":"https://example.com"}"#.to_string(),
            },
        ])];
        let live = vec![(
            "navigate".to_string(),
            "https://example.com".to_string(),
            ToolCallStatus::Running,
        )];

        let (_, text) = build_wrapped_list(&messages, &HashMap::new(), &live, 80, None, '⠋', false);

        assert!(
            !text.iter().any(|line| line.contains("interrupted")),
            "should not show interrupted when live_tool_calls is active"
        );
    }

    #[test]
    fn build_wrapped_list_renders_reasoning_dimmed() {
        let messages = vec![ConversationMessage::assistant(vec![
            ContentBlock::Reasoning {
                data: "thinking...".to_string(),
            },
        ])];

        let (_, text) = build_wrapped_list(&messages, &HashMap::new(), &[], 80, None, '⠋', false);

        assert!(text.iter().any(|line| line.contains("thinking...")));
    }

    #[test]
    fn build_wrapped_list_skips_tool_role_messages() {
        let messages = vec![
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "navigate".to_string(),
                input: r#"{"url":"https://example.com"}"#.to_string(),
            }]),
            ConversationMessage::tool_result("tool_1", "navigate", "navigation complete", false),
        ];
        let tool_results = HashMap::from([(
            "tool_1".to_string(),
            ToolResultInfo {
                output: "navigation complete".to_string(),
                is_error: false,
            },
        )]);

        let (_, text) = build_wrapped_list(&messages, &tool_results, &[], 80, None, '⠋', false);

        assert_eq!(
            text.iter().filter(|line| line.contains("navigate")).count(),
            1
        );
    }

    #[test]
    fn build_wrapped_list_renders_live_text_after_messages() {
        let (_, text) =
            build_wrapped_list(&[], &HashMap::new(), &[], 80, Some("hello"), '⠋', false);

        assert!(text.iter().any(|line| line.contains("hello")));
    }
}
