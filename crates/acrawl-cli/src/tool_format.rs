use std::fmt::Write as _;

use crate::format::truncate_for_summary;

pub(crate) fn format_tool_call_start(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));

    let detail = match name {
        "bash" | "Bash" => format_bash_call(&parsed),
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|value| value.as_str())
            .unwrap_or("?")
            .to_string(),
        "navigate" => format!(
            "\x1b[38;5;220m⠧ Navigate: {}\x1b[0m",
            parsed.get("url").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        "click" => "\x1b[38;5;220m⠧ Click element\x1b[0m".to_string(),
        "fill_form" => "\x1b[38;5;220m⠧ Fill form\x1b[0m".to_string(),
        "scroll" => "\x1b[38;5;220m⠧ Scroll\x1b[0m".to_string(),
        "hover" => "\x1b[38;5;220m⠧ Hover\x1b[0m".to_string(),
        "press_key" => format!(
            "\x1b[38;5;220m⠧ Press key {}\x1b[0m",
            parsed.get("key").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        "switch_tab" => "\x1b[38;5;220m⠧ Switch tab\x1b[0m".to_string(),
        "wait" => "\x1b[38;5;220m⠧ Wait\x1b[0m".to_string(),
        "select_option" => "\x1b[38;5;220m⠧ Select option\x1b[0m".to_string(),
        "go_back" => "\x1b[38;5;220m⠧ Go back\x1b[0m".to_string(),
        "page_map" => "\x1b[38;5;220m⠧ Mapping page structure...\x1b[0m".to_string(),
        "read_content" => "\x1b[38;5;220m⠧ Reading content...\x1b[0m".to_string(),
        "list_resources" => "\x1b[38;5;220m⠧ Listing resources...\x1b[0m".to_string(),
        "execute_js" => "\x1b[38;5;201m⚙ Executing script...\x1b[0m".to_string(),
        "screenshot" => "\x1b[38;5;220m⠧ Taking screenshot...\x1b[0m".to_string(),
        _ => summarize_tool_payload(input),
    };

    let border = "─".repeat(name.len() + 8);
    format!(
        "\x1b[38;5;245m╭─ \x1b[1;36m{name}\x1b[0;38;5;245m ─╮\x1b[0m\n\x1b[38;5;245m│\x1b[0m {detail}\n\x1b[38;5;245m╰{border}╯\x1b[0m"
    )
}

pub(crate) fn format_tool_result(name: &str, output: &str, is_error: bool) -> String {
    let icon = if is_error {
        "\x1b[1;31m✗\x1b[0m"
    } else {
        "\x1b[1;32m✓\x1b[0m"
    };
    if is_error {
        let trimmed = output.trim();
        return if trimmed.is_empty() {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
        } else {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n\x1b[38;5;203m{trimmed}\x1b[0m")
        };
    }

    let parsed: serde_json::Value =
        serde_json::from_str(output).unwrap_or(serde_json::Value::String(output.to_string()));
    match name {
        "bash" | "Bash" => format_bash_result(icon, &parsed),
        "navigate" | "click" | "fill_form" | "scroll" | "hover" | "press_key" | "switch_tab"
        | "wait" | "select_option" | "go_back" | "execute_js" => {
            format!("{icon} \x1b[38;5;245m{name} done\x1b[0m")
        }
        "page_map" | "read_content" | "list_resources" => {
            let summary = truncate_for_summary(output.trim(), 100);
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m: {summary}")
        }
        "screenshot" => {
            let path = parsed
                .get("saved_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!(
                "{icon} \x1b[38;5;245m{name}\x1b[0m 📸 Screenshot saved to \x1b[4m{path}\x1b[0m"
            )
        }
        _ => {
            let summary = truncate_for_summary(output.trim(), 200);
            format!("{icon} \x1b[38;5;245m{name}:\x1b[0m {summary}")
        }
    }
}

fn format_bash_call(parsed: &serde_json::Value) -> String {
    let command = parsed
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if command.is_empty() {
        String::new()
    } else {
        format!(
            "\x1b[48;5;236;38;5;255m $ {} \x1b[0m",
            truncate_for_summary(command, 160)
        )
    }
}

fn format_bash_result(icon: &str, parsed: &serde_json::Value) -> String {
    let mut lines = vec![format!("{icon} \x1b[38;5;245mbash\x1b[0m")];
    if let Some(task_id) = parsed
        .get("backgroundTaskId")
        .and_then(|value| value.as_str())
    {
        write!(lines[0], " backgrounded ({task_id})").ok();
    } else if let Some(status) = parsed
        .get("returnCodeInterpretation")
        .and_then(|value| value.as_str())
        .filter(|status| !status.is_empty())
    {
        write!(lines[0], " {status}").ok();
    }

    if let Some(stdout) = parsed.get("stdout").and_then(|value| value.as_str()) {
        if !stdout.trim().is_empty() {
            lines.push(stdout.trim_end().to_string());
        }
    }
    if let Some(stderr) = parsed.get("stderr").and_then(|value| value.as_str()) {
        if !stderr.trim().is_empty() {
            lines.push(format!("\x1b[38;5;203m{}\x1b[0m", stderr.trim_end()));
        }
    }

    lines.join("\n\n")
}

fn summarize_tool_payload(payload: &str) -> String {
    let compact = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value.to_string(),
        Err(_) => payload.trim().to_string(),
    };
    truncate_for_summary(&compact, 96)
}

#[cfg(test)]
mod tests {
    use super::{format_tool_call_start, format_tool_result};

    #[test]
    fn tool_rendering_helpers_compact_output() {
        let start = format_tool_call_start("navigate", r#"{"url":"https://example.com"}"#);
        assert!(start.contains("navigate"));
        assert!(start.contains("https://example.com"));

        let done = format_tool_result("navigate", r#"{"ok":true}"#, false);
        assert!(done.contains("navigate done"));
    }
}
