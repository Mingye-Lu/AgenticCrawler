use std::cmp::min;
use std::io;

use ansi_to_tui::IntoText;
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

use crate::app::LiveCli;
use crate::display_width::{split_at_display_width, text_display_width};
use crate::format::VERSION;

use super::repl_app::{
    HeaderSnapshot, ReplTuiState, ToolCallStatus, TranscriptEntry, SLASH_OVERLAY_HINT_TEXT,
    SLASH_OVERLAY_VISIBLE_ITEMS,
    WELCOME_BOX_MAX_WIDTH, WELCOME_BOX_MIN_WIDTH, WELCOME_BOX_SIDE_GUTTER,
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

pub(super) fn ansi_to_lines(ansi: &str) -> Vec<Line<'static>> {
    let fallback_style = Style::default().fg(Color::Rgb(215, 225, 235));
    match ansi.as_bytes().into_text() {
        Ok(text) => text.lines,
        Err(_) => vec![Line::from(Span::styled(ansi.to_string(), fallback_style))],
    }
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

pub(super) fn strip_ansi(s: &str) -> String {
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

pub(super) fn extract_json_path(parsed: &serde_json::Value) -> String {
    parsed
        .get("file_path")
        .or_else(|| parsed.get("filePath"))
        .or_else(|| parsed.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string()
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

    let cmd = serde_json::from_str::<serde_json::Value>(input_summary)
        .ok()
        .and_then(|v| {
            v.get("command")
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| input_summary.to_string());

    let cmd_display = truncate_with_ellipsis(&cmd, 60);

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

pub(super) fn render_read_success(
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

pub(super) fn render_write_success(
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

pub(super) fn render_edit_success(
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
                        diff_lines.push(ListItem::new(Line::from(Span::styled(
                            line.clone(),
                            style,
                        ))));
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

pub(super) fn render_glob_success(
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
            items.push(ListItem::new(Line::from(Span::styled(filename.clone(), dim))));
            text_lines.push(filename);
        }
    }
}

pub(super) fn render_grep_success(
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
            items.push(ListItem::new(Line::from(Span::styled(filename.clone(), dim))));
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

pub(super) fn render_tool_call_lines(
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
                        truncate_with_ellipsis(&trimmed, 60)
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
            let truncated = truncate_with_ellipsis(msg, 120);
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

/// Truncate `s` to at most `max_bytes` bytes, snapping to a char boundary,
/// and appending '…' when truncation occurs.
pub(super) fn truncate_with_ellipsis(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub(super) fn tool_input_summary(name: &str, input: &str) -> String {
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
    truncate_with_ellipsis(key_param, 60)
}

#[allow(clippy::too_many_lines)]
pub(super) fn build_wrapped_list(
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
    let user_prefix_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

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
                out.push(ListItem::new(Line::from(Span::styled(header, border_style))));

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
                            format!("│ {} {line_str}{} │", " ".repeat(key_col), " ".repeat(pad))
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
                out.push(ListItem::new(Line::from(Span::styled(bottom, border_style))));
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
    let _ = io::Write::write_all(&mut io::stdout(), format!("\x1b]52;c;{encoded}\x07").as_bytes());
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

pub(super) fn draw_header(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    header: &HeaderSnapshot,
) {
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
    let version_str = format!("v{VERSION} · Playwright ready");
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
    let overlay_x = input_area.x.saturating_add(2).clamp(min_x, max_x.max(min_x));
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
pub(super) fn draw_chat(
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
