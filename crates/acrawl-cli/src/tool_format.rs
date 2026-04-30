use std::fmt::Write as _;

use crate::markdown::strip_ansi;

#[derive(Debug, Clone)]
pub(crate) struct ToolLine {
    /// `"⠋"` running, `"✓"` success, `"✗"` error
    pub(crate) icon: &'static str,
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) detail_lines: Vec<String>,
}

/// Truncate to at most `max_bytes` bytes, snapping to a char boundary.
pub(crate) fn truncate_with_ellipsis(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\u{2026}", &s[..end])
}

pub(crate) fn tool_input_summary(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::Null);
    let key_param = match name {
        "bash" | "Bash" => parsed
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or(input),
        "navigate" => parsed
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or(input),
        "click" | "scroll" | "hover" | "press_key" | "fill_form" | "select_option"
        | "switch_tab" | "wait" | "go_back" | "execute_js" | "screenshot"
        | "list_resources" | "save_file" => "",
        _ => input,
    };
    truncate_with_ellipsis(key_param, 60)
}

pub(crate) fn format_tool_start_line(name: &str, input: &str) -> ToolLine {
    ToolLine {
        icon: "\u{280B}",
        name: name.to_string(),
        summary: tool_input_summary(name, input),
        detail_lines: vec![],
    }
}

pub(crate) fn format_tool_success_line(
    name: &str,
    input_summary: &str,
    output: &str,
) -> ToolLine {
    let parsed: serde_json::Value = serde_json::from_str(output)
        .unwrap_or(serde_json::Value::String(output.to_string()));
    match name {
        "bash" | "Bash" => format_bash_success_line(name, input_summary, &parsed),
        _ => {
            let summary = if output.trim().is_empty() {
                "done".to_string()
            } else {
                let trimmed = strip_ansi(output.trim()).replace('\n', " ");
                truncate_with_ellipsis(&trimmed, 60)
            };
            ToolLine {
                icon: "\u{2713}",
                name: name.to_string(),
                summary,
                detail_lines: vec![],
            }
        }
    }
}

pub(crate) fn format_tool_error_line(name: &str, error: &str) -> ToolLine {
    ToolLine {
        icon: "\u{2717}",
        name: name.to_string(),
        summary: error.trim().to_string(),
        detail_lines: vec![],
    }
}

fn format_bash_success_line(
    name: &str,
    input_summary: &str,
    parsed: &serde_json::Value,
) -> ToolLine {
    let cmd = serde_json::from_str::<serde_json::Value>(input_summary)
        .ok()
        .and_then(|v| {
            v.get("command")
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| input_summary.to_string());

    let mut summary = truncate_with_ellipsis(&cmd, 60);

    if let Some(task_id) = parsed
        .get("backgroundTaskId")
        .and_then(|v| v.as_str())
    {
        write!(summary, " backgrounded ({task_id})").ok();
    } else if let Some(interp) = parsed
        .get("returnCodeInterpretation")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        write!(summary, " {interp}").ok();
    }

    let mut detail_lines = Vec::new();

    if let Some(stdout) = parsed.get("stdout").and_then(|v| v.as_str()) {
        let trimmed = stdout.trim_end();
        if !trimmed.is_empty() {
            for line in strip_ansi(trimmed).lines().take(15) {
                detail_lines.push(line.to_string());
            }
        }
    }
    if let Some(stderr) = parsed.get("stderr").and_then(|v| v.as_str()) {
        let trimmed = stderr.trim_end();
        if !trimmed.is_empty() {
            for line in strip_ansi(trimmed).lines().take(5) {
                detail_lines.push(format!("stderr: {line}"));
            }
        }
    }

    ToolLine {
        icon: "\u{2713}",
        name: name.to_string(),
        summary,
        detail_lines,
    }
}

#[allow(dead_code)]
#[deprecated(note = "Use format_tool_start_line instead")]
pub(crate) fn format_tool_call_start(name: &str, input: &str) -> String {
    let line = format_tool_start_line(name, input);
    format!("{} {} {}", line.icon, line.name, line.summary)
}

#[allow(dead_code)]
#[deprecated(note = "Use format_tool_success_line / format_tool_error_line instead")]
pub(crate) fn format_tool_result(name: &str, output: &str, is_error: bool) -> String {
    let line = if is_error {
        format_tool_error_line(name, output)
    } else {
        format_tool_success_line(name, &tool_input_summary(name, "{}"), output)
    };
    if line.detail_lines.is_empty() {
        format!("{} {} {}", line.icon, line.name, line.summary)
    } else {
        let mut s = format!("{} {} {}", line.icon, line.name, line.summary);
        for l in &line.detail_lines {
            s.push('\n');
            s.push_str(l);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_rendering_helpers_compact_output() {
        let start = format_tool_start_line("navigate", r#"{"url":"https://example.com"}"#);
        assert_eq!(start.icon, "\u{280B}");
        assert_eq!(start.name, "navigate");
        assert!(start.summary.contains("https://example.com"));

        let done = format_tool_success_line("navigate", "", r#"{"ok":true}"#);
        assert_eq!(done.icon, "\u{2713}");
        assert_eq!(done.name, "navigate");
        assert!(!done.summary.is_empty());
    }

    #[test]
    fn truncate_with_ellipsis_no_truncation() {
        assert_eq!(truncate_with_ellipsis("short", 10), "short");
    }

    #[test]
    fn truncate_with_ellipsis_cuts_at_boundary() {
        let result = truncate_with_ellipsis("hello world", 5);
        assert_eq!(result, "hello\u{2026}");
    }

    #[test]
    fn truncate_snaps_back_on_multibyte() {
        let result = truncate_with_ellipsis("\u{4e16}\u{754c}", 2);
        assert_eq!(result, "\u{2026}");
    }

    #[test]
    fn tool_input_summary_bash() {
        let s = tool_input_summary("bash", r#"{"command":"ls -la"}"#);
        assert_eq!(s, "ls -la");
    }

    #[test]
    fn tool_input_summary_navigate() {
        let s = tool_input_summary("navigate", r#"{"url":"https://example.com"}"#);
        assert_eq!(s, "https://example.com");
    }

    #[test]
    fn tool_input_summary_click_empty() {
        let s = tool_input_summary("click", r##"{"selector":"#btn"}"##);
        assert_eq!(s, "");
    }

    #[test]
    fn tool_input_summary_unknown_tool_passthrough() {
        let s = tool_input_summary("some_mcp_tool", r#"{"key":"val"}"#);
        assert_eq!(s, r#"{"key":"val"}"#);
    }

    #[test]
    fn format_start_line_uses_spinner() {
        let line = format_tool_start_line("bash", r#"{"command":"pwd"}"#);
        assert_eq!(line.icon, "\u{280B}");
        assert_eq!(line.name, "bash");
        assert_eq!(line.summary, "pwd");
        assert!(line.detail_lines.is_empty());
    }

    #[test]
    fn format_bash_success_with_stdout() {
        let output =
            r#"{"stdout":"line1\nline2","stderr":"","returnCodeInterpretation":"success"}"#;
        let line = format_tool_success_line("bash", r#"{"command":"echo hi"}"#, output);
        assert_eq!(line.icon, "\u{2713}");
        assert!(line.summary.contains("echo hi"));
        assert!(line.summary.contains("success"));
    }

    #[test]
    fn format_bash_success_with_background_task() {
        let output = r#"{"stdout":"","stderr":"","backgroundTaskId":"task-42"}"#;
        let line = format_tool_success_line("bash", r#"{"command":"sleep 10"}"#, output);
        assert!(line.summary.contains("backgrounded (task-42)"));
    }

    #[test]
    fn format_bash_success_stderr_prefixed() {
        let output = r#"{"stdout":"","stderr":"warning: something\nbad"}"#;
        let line = format_tool_success_line("bash", r#"{"command":"make"}"#, output);
        assert!(line.detail_lines.iter().all(|l| l.starts_with("stderr: ")));
        assert_eq!(line.detail_lines.len(), 2);
    }

    #[test]
    fn format_non_bash_success_empty_output() {
        let line = format_tool_success_line("click", "", "");
        assert_eq!(line.icon, "\u{2713}");
        assert_eq!(line.summary, "done");
    }

    #[test]
    fn format_error_line_trims() {
        let line = format_tool_error_line("bash", "  some error  ");
        assert_eq!(line.icon, "\u{2717}");
        assert_eq!(line.summary, "some error");
    }

    #[test]
    fn no_box_drawing_in_new_api() {
        let start = format_tool_start_line("bash", r#"{"command":"pwd"}"#);
        let success = format_tool_success_line("bash", r#"{"command":"pwd"}"#, r#"{"stdout":"ok"}"#);
        let error = format_tool_error_line("bash", "fail");

        for line in [&start, &success, &error] {
            let combined = format!(
                "{}{}{}{}",
                line.icon,
                line.name,
                line.summary,
                line.detail_lines.join("")
            );
            for ch in ['\u{256D}', '\u{256E}', '\u{2502}', '\u{2570}', '\u{256F}'] {
                assert!(
                    !combined.contains(ch),
                    "found box-drawing char {ch:?} in ToolLine output"
                );
            }
        }
    }

    #[allow(deprecated)]
    #[test]
    fn deprecated_wrappers_still_work() {
        let start = format_tool_call_start("navigate", r#"{"url":"https://example.com"}"#);
        assert!(start.contains("navigate"));
        assert!(start.contains("https://example.com"));

        let done = format_tool_result("navigate", r#"{"ok":true}"#, false);
        assert!(done.contains("navigate"));

        let err = format_tool_result("bash", "command failed", true);
        assert!(err.contains("\u{2717}"));
        assert!(err.contains("command failed"));
    }
}
