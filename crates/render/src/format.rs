use std::env;
use std::path::{Path, PathBuf};

use commands::render_slash_command_help;
use runtime::{ConfigLoader, ContentBlock, MessageRole, Session, TokenUsage};

pub const DEFAULT_DATE: &str = "2026-03-31";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_TARGET: Option<&str> = option_env!("TARGET");
pub const GIT_SHA: Option<&str> = option_env!("GIT_SHA");

#[derive(Debug, Clone)]
pub struct StatusContext {
    pub session_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub struct StatusUsage {
    pub message_count: usize,
    pub turns: u32,
    pub latest: TokenUsage,
    pub cumulative: TokenUsage,
    pub estimated_tokens: usize,
}

#[must_use]
pub fn format_model_report(model: &str, message_count: usize, turns: u32) -> String {
    format!(
        "Model\n  Current model    {model}\n  Session messages {message_count}\n  Session turns    {turns}\n\nUsage\n  Inspect current model with /model\n  Switch models with /model <name>"
    )
}

#[must_use]
pub fn format_model_switch_report(previous: &str, next: &str, message_count: usize) -> String {
    format!(
        "Model updated\n  Previous         {previous}\n  Current          {next}\n  Preserved msgs   {message_count}"
    )
}

#[must_use]
pub fn format_cost_report(usage: TokenUsage) -> String {
    format!(
        "Cost\n  Input tokens     {}\n  Output tokens    {}\n  Cache create     {}\n  Cache read       {}\n  Total tokens     {}",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
        usage.total_tokens(),
    )
}

#[must_use]
pub fn format_compact_report(removed: usize, resulting_messages: usize, skipped: bool) -> String {
    if skipped {
        format!(
            "Compact\n  Result           skipped\n  Reason           session below compaction threshold\n  Messages kept    {resulting_messages}"
        )
    } else {
        format!(
            "Compact\n  Result           compacted\n  Messages removed {removed}\n  Messages kept    {resulting_messages}"
        )
    }
}

#[must_use]
pub fn format_auto_compaction_notice(removed: usize) -> String {
    format!("[auto-compacted: removed {removed} messages]")
}

#[must_use]
pub fn format_status_report(model: &str, usage: StatusUsage, context: &StatusContext) -> String {
    [
        format!(
            "Status\n  Model            {model}\n  Messages         {}\n  Turns            {}\n  Estimated tokens {}\n  Session          {}",
            usage.message_count, usage.turns, usage.estimated_tokens,
            context.session_path.as_ref().map_or_else(|| "live-repl".to_string(), |path| path.display().to_string()),
        ),
        format!(
            "Usage\n  Latest total     {}\n  Cumulative input {}\n  Cumulative output {}\n  Cumulative total {}",
            usage.latest.total_tokens(),
            usage.cumulative.input_tokens,
            usage.cumulative.output_tokens,
            usage.cumulative.total_tokens(),
        ),
    ]
    .join("\n\n")
}

pub fn status_context(session_path: Option<&Path>) -> StatusContext {
    StatusContext {
        session_path: session_path.map(Path::to_path_buf),
    }
}

#[must_use]
pub fn render_repl_help() -> String {
    [
        "REPL".to_string(),
        "  /exit                Quit the REPL".to_string(),
        "  /quit                Quit the REPL".to_string(),
        "  Up/Down              Navigate prompt history".to_string(),
        "  Tab                  Complete slash commands".to_string(),
        "  Ctrl-C               Clear input (or exit on empty prompt)".to_string(),
        "  Shift+Enter/Ctrl+J   Insert a newline".to_string(),
        String::new(),
        render_slash_command_help(),
    ]
    .join("\n")
}

pub fn render_config_report(section: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let loader = ConfigLoader::default_for();
    let discovered = loader.discover();
    let runtime_config = loader.load()?;

    let mut lines = vec![
        format!(
            "Config\n  Loaded files      {}\n  Merged keys       {}",
            runtime_config.loaded_entries().len(),
            runtime_config.merged().len()
        ),
        "Discovered files".to_string(),
    ];
    for entry in discovered {
        let status = if runtime_config
            .loaded_entries()
            .iter()
            .any(|loaded_entry| loaded_entry.path == entry.path)
        {
            "loaded"
        } else {
            "missing"
        };
        lines.push(format!("  {status:<7} {}", entry.path.display()));
    }

    if let Some(section) = section {
        lines.push(format!("Merged section: {section}"));
        let value = match section {
            "model" => runtime_config.get("model"),
            other => {
                lines.push(format!(
                    "  Unsupported config section '{other}'. Use model."
                ));
                return Ok(lines.join("\n"));
            }
        };
        lines.push(format!(
            "  {}",
            match value {
                Some(value) => value.render(),
                None => "<unset>".to_string(),
            }
        ));
        return Ok(lines.join("\n"));
    }

    lines.push("Merged JSON".to_string());
    lines.push(format!("  {}", runtime_config.as_json().render()));
    Ok(lines.join("\n"))
}

#[must_use]
pub fn render_version_report() -> String {
    let git_sha = GIT_SHA.unwrap_or("unknown");
    let target = BUILD_TARGET.unwrap_or("unknown");
    format!(
        "AgenticCrawler\n  Version          {VERSION}\n  Git SHA          {git_sha}\n  Target           {target}\n  Build date       {DEFAULT_DATE}"
    )
}

#[must_use]
pub fn render_export_text(session: &Session) -> String {
    let mut lines = vec!["# Conversation Export".to_string(), String::new()];
    for (index, message) in session.messages.iter().enumerate() {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        lines.push(format!("## {}. {role}", index + 1));
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => lines.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    lines.push(format!("[tool_use id={id} name={name}] {input}"));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    tool_name,
                    output,
                    is_error,
                } => {
                    lines.push(format!(
                        "[tool_result id={tool_use_id} name={tool_name} error={is_error}] {output}"
                    ));
                }
                ContentBlock::Reasoning { .. } => {}
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

#[must_use]
pub fn default_export_filename(session: &Session) -> String {
    let stem = session
        .messages
        .iter()
        .find_map(|message| match message.role {
            MessageRole::User => message.blocks.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }),
            _ => None,
        })
        .map_or("conversation", |text| {
            text.lines().next().unwrap_or("conversation")
        })
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let fallback = if stem.is_empty() {
        "conversation"
    } else {
        &stem
    };
    format!("{fallback}.txt")
}

pub fn resolve_export_path(
    requested_path: Option<&str>,
    session: &Session,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let file_name =
        requested_path.map_or_else(|| default_export_filename(session), ToOwned::to_owned);
    let final_name = if Path::new(&file_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
    {
        file_name
    } else {
        format!("{file_name}.txt")
    };
    Ok(cwd.join(final_name))
}

#[deprecated(note = "Use tool_format::truncate_with_ellipsis instead")]
#[allow(dead_code)]
#[must_use]
pub fn truncate_for_summary(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn compact_report_uses_structured_output() {
        let compacted = format_compact_report(8, 5, false);
        assert!(compacted.contains("Compact"));
        assert!(compacted.contains("Result           compacted"));
        assert!(compacted.contains("Messages removed 8"));
        let skipped = format_compact_report(0, 3, true);
        assert!(skipped.contains("Result           skipped"));
    }

    #[test]
    fn cost_report_uses_sectioned_layout() {
        let report = format_cost_report(TokenUsage {
            input_tokens: 20,
            output_tokens: 8,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 1,
        });
        assert!(report.contains("Cost"));
        assert!(report.contains("Input tokens     20"));
        assert!(report.contains("Output tokens    8"));
        assert!(report.contains("Cache create     3"));
        assert!(report.contains("Cache read       1"));
        assert!(report.contains("Total tokens     32"));
    }

    #[test]
    fn model_report_uses_sectioned_layout() {
        let report = format_model_report("claude-sonnet", 12, 4);
        assert!(report.contains("Model"));
        assert!(report.contains("Current model    claude-sonnet"));
        assert!(report.contains("Session messages 12"));
        assert!(report.contains("Switch models with /model <name>"));
    }

    #[test]
    fn model_switch_report_preserves_context_summary() {
        let report = format_model_switch_report("claude-sonnet", "claude-opus", 9);
        assert!(report.contains("Model updated"));
        assert!(report.contains("Previous         claude-sonnet"));
        assert!(report.contains("Current          claude-opus"));
        assert!(report.contains("Preserved msgs   9"));
    }

    #[test]
    fn status_line_reports_model_and_token_totals() {
        let status = format_status_report(
            "claude-sonnet",
            StatusUsage {
                message_count: 7,
                turns: 3,
                latest: TokenUsage {
                    input_tokens: 5,
                    output_tokens: 4,
                    cache_creation_input_tokens: 1,
                    cache_read_input_tokens: 0,
                },
                cumulative: TokenUsage {
                    input_tokens: 20,
                    output_tokens: 8,
                    cache_creation_input_tokens: 2,
                    cache_read_input_tokens: 1,
                },
                estimated_tokens: 128,
            },
            &StatusContext {
                session_path: Some(PathBuf::from("session.json")),
            },
        );
        assert!(status.contains("Status"));
        assert!(status.contains("Model            claude-sonnet"));
        assert!(status.contains("Messages         7"));
        assert!(status.contains("Latest total     10"));
        assert!(status.contains("Cumulative total 31"));
        assert!(status.contains("Session          session.json"));
    }

    #[test]
    fn config_report_supports_section_views() {
        let report = render_config_report(Some("model")).expect("config report should render");
        assert!(report.contains("Merged section: model"));
    }

    #[test]
    fn config_report_uses_sectioned_layout() {
        let report = render_config_report(None).expect("config report should render");
        assert!(report.contains("Config"));
        assert!(report.contains("Discovered files"));
        assert!(report.contains("Merged JSON"));
    }

    #[test]
    fn status_context_returns_session_path() {
        let context = status_context(None);
        assert!(context.session_path.is_none());
        let context = status_context(Some(Path::new("test.json")));
        assert_eq!(
            context.session_path.as_deref(),
            Some(Path::new("test.json"))
        );
    }

    #[test]
    fn repl_help_includes_shared_commands_and_exit() {
        let help = render_repl_help();
        assert!(help.contains("REPL"));
        assert!(help.contains("/help"));
        assert!(help.contains("/status"));
        assert!(help.contains("/model [model]"));
        assert!(help.contains("/clear [--confirm]"));
        assert!(help.contains("/cost"));
        assert!(help.contains("/config [model]"));
        assert!(help.contains("/debug"));
        assert!(help.contains("/version"));
        assert!(help.contains("/export [file]"));
        assert!(help.contains("/sessions"));
        assert!(!help.contains("/resume"));
        assert!(help.contains("/headed"));
        assert!(help.contains("/headless"));
        assert!(help.contains("/exit"));
    }

    #[test]
    fn repl_help_mentions_history_completion_and_multiline() {
        let help = render_repl_help();
        assert!(help.contains("Up/Down"));
        assert!(help.contains("Tab"));
        assert!(help.contains("Shift+Enter/Ctrl+J"));
    }

    #[test]
    fn version_report_contains_all_fields() {
        let report = render_version_report();
        assert!(report.contains("AgenticCrawler"));
        assert!(report.contains("Version"));
        assert!(report.contains(VERSION));
        assert!(report.contains("Git SHA"));
        assert!(report.contains("Target"));
        assert!(report.contains("Build date"));
        assert!(report.contains(DEFAULT_DATE));
    }

    #[test]
    fn auto_compaction_notice_contains_count() {
        let notice = format_auto_compaction_notice(12);
        assert_eq!(notice, "[auto-compacted: removed 12 messages]");
    }

    #[test]
    fn export_text_renders_user_and_assistant_messages() {
        use runtime::{ContentBlock, ConversationMessage, MessageRole};
        let session = Session {
            version: 1,
            model: Some("test-model".to_string()),
            title: None,
            messages: vec![
                ConversationMessage {
                    role: MessageRole::User,
                    blocks: vec![ContentBlock::Text {
                        text: "Hello agent".to_string(),
                    }],
                    usage: None,
                },
                ConversationMessage {
                    role: MessageRole::Assistant,
                    blocks: vec![ContentBlock::Text {
                        text: "Hello human".to_string(),
                    }],
                    usage: None,
                },
            ],
            child_sessions: Vec::new(),
        };
        let exported = render_export_text(&session);
        assert!(exported.contains("# Conversation Export"));
        assert!(exported.contains("## 1. user"));
        assert!(exported.contains("Hello agent"));
        assert!(exported.contains("## 2. assistant"));
        assert!(exported.contains("Hello human"));
    }

    #[test]
    fn default_export_filename_uses_first_user_message() {
        use runtime::{ContentBlock, ConversationMessage, MessageRole};
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![ConversationMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "Scrape all books from example.com".to_string(),
                }],
                usage: None,
            }],
            child_sessions: Vec::new(),
        };
        let filename = default_export_filename(&session);
        assert!(Path::new(&filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("txt")));
        assert!(filename.contains("scrape"));
        assert!(!filename.contains(' '));
    }

    #[test]
    fn default_export_filename_fallback_when_no_user_message() {
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![],
            child_sessions: Vec::new(),
        };
        let filename = default_export_filename(&session);
        assert_eq!(filename, "conversation.txt");
    }
}
