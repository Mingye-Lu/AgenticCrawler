use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

const COMPACT_CONTINUATION_PREAMBLE: &str =
    "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n";
const COMPACT_RECENT_MESSAGES_NOTE: &str = "Recent messages are preserved verbatim.";
const COMPACT_DIRECT_RESUME_INSTRUCTION: &str =
    "Continue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, and do not preface with continuation text.";

const PRUNE_PROTECT_TOKENS: usize = 40_000;
const PRUNE_MAX_OUTPUT_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub preserve_recent_messages: usize,
    pub max_estimated_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            preserve_recent_messages: 4,
            max_estimated_tokens: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub summary: String,
    pub formatted_summary: String,
    pub compacted_session: Session,
    pub removed_message_count: usize,
}

#[must_use]
pub fn estimate_session_tokens(session: &Session) -> usize {
    session.messages.iter().map(estimate_message_tokens).sum()
}

#[must_use]
pub fn should_compact(session: &Session, config: CompactionConfig) -> bool {
    session.messages.len() > config.preserve_recent_messages
        && estimate_session_tokens(session) >= config.max_estimated_tokens
}

#[must_use]
pub fn format_compact_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    let formatted = if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        without_analysis.replace(
            &format!("<summary>{content}</summary>"),
            &format!("Summary:\n{}", content.trim()),
        )
    } else {
        without_analysis
    };

    collapse_blank_lines(&formatted).trim().to_string()
}

#[must_use]
pub fn get_compact_continuation_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    recent_messages_preserved: bool,
) -> String {
    let mut base = format!(
        "{COMPACT_CONTINUATION_PREAMBLE}{}",
        format_compact_summary(summary)
    );

    if recent_messages_preserved {
        base.push_str("\n\n");
        base.push_str(COMPACT_RECENT_MESSAGES_NOTE);
    }

    if suppress_follow_up_questions {
        base.push('\n');
        base.push_str(COMPACT_DIRECT_RESUME_INSTRUCTION);
    }

    base
}

#[must_use]
pub fn compact_session(session: &Session, config: CompactionConfig) -> CompactionResult {
    if !should_compact(session, config) {
        return CompactionResult {
            summary: String::new(),
            formatted_summary: String::new(),
            compacted_session: session.clone(),
            removed_message_count: 0,
        };
    }

    let mut working_messages = session.messages.clone();
    prune_tool_outputs(&mut working_messages);

    let mut keep_from = working_messages
        .len()
        .saturating_sub(config.preserve_recent_messages);

    // Never split a tool_use/tool_result pair. If the preserved window starts
    // with a Tool message (containing tool_result blocks), walk backwards to
    // include the preceding Assistant message that holds the matching tool_use.
    while keep_from > 0 && working_messages[keep_from].role == MessageRole::Tool {
        keep_from -= 1;
    }

    let removed = &working_messages[..keep_from];
    let preserved = working_messages[keep_from..].to_vec();
    let summary = summarize_messages(removed);
    let formatted_summary = format_compact_summary(&summary);
    let continuation = get_compact_continuation_message(&summary, true, !preserved.is_empty());

    let mut compacted_messages = vec![ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text { text: continuation }],
        usage: None,
    }];
    compacted_messages.extend(preserved);

    CompactionResult {
        summary,
        formatted_summary,
        compacted_session: Session {
            version: session.version,
            model: session.model.clone(),
            title: session.title.clone(),
            messages: compacted_messages,
        },
        removed_message_count: removed.len(),
    }
}

fn summarize_messages(messages: &[ConversationMessage]) -> String {
    let user_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    let assistant_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    let tool_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .count();

    let mut tool_names = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            ContentBlock::ToolResult { tool_name, .. } => Some(tool_name.as_str()),
            ContentBlock::Text { .. } | ContentBlock::Reasoning { .. } => None,
        })
        .collect::<Vec<_>>();
    tool_names.sort_unstable();
    tool_names.dedup();

    let mut lines = vec![
        "<summary>".to_string(),
        "Conversation summary:".to_string(),
        format!(
            "- Scope: {} earlier messages compacted (user={}, assistant={}, tool={}).",
            messages.len(),
            user_messages,
            assistant_messages,
            tool_messages
        ),
    ];

    if !tool_names.is_empty() {
        lines.push(format!("- Tools mentioned: {}.", tool_names.join(", ")));
    }

    let recent_user_requests = collect_recent_role_summaries(messages, MessageRole::User, 3);
    if !recent_user_requests.is_empty() {
        lines.push("- Recent user requests:".to_string());
        lines.extend(
            recent_user_requests
                .into_iter()
                .map(|request| format!("  - {request}")),
        );
    }

    let pending_work = infer_pending_work(messages);
    if !pending_work.is_empty() {
        lines.push("- Pending work:".to_string());
        lines.extend(pending_work.into_iter().map(|item| format!("  - {item}")));
    }

    let key_files = collect_key_files(messages);
    if !key_files.is_empty() {
        lines.push(format!("- Key files referenced: {}.", key_files.join(", ")));
    }

    if let Some(current_work) = infer_current_work(messages) {
        lines.push(format!("- Current work: {current_work}"));
    }

    lines.push("- Key timeline:".to_string());
    for message in messages {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let content = message
            .blocks
            .iter()
            .map(summarize_block)
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("  - {role}: {content}"));
    }
    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn summarize_block(block: &ContentBlock) -> String {
    let raw = match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::ToolUse { name, input, .. } => format!("tool_use {name}({input})"),
        ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } => format!(
            "tool_result {tool_name}: {}{output}",
            if *is_error { "error " } else { "" }
        ),
        ContentBlock::Reasoning { .. } => "reasoning".to_string(),
    };
    truncate_summary(&raw, 160)
}

fn collect_recent_role_summaries(
    messages: &[ConversationMessage],
    role: MessageRole,
    limit: usize,
) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == role)
        .rev()
        .filter_map(|message| first_text_block(message))
        .take(limit)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn infer_pending_work(messages: &[ConversationMessage]) -> Vec<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .filter(|text| {
            let lowered = text.to_ascii_lowercase();
            lowered.contains("todo")
                || lowered.contains("next")
                || lowered.contains("pending")
                || lowered.contains("follow up")
                || lowered.contains("remaining")
        })
        .take(3)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn collect_key_files(messages: &[ConversationMessage]) -> Vec<String> {
    let mut files = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.as_str(),
            ContentBlock::ToolUse { input, .. } => input.as_str(),
            ContentBlock::ToolResult { output, .. } => output.as_str(),
            ContentBlock::Reasoning { data } => data.as_str(),
        })
        .flat_map(extract_file_candidates)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files.into_iter().take(8).collect()
}

fn infer_current_work(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .find(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, 200))
}

fn first_text_block(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
        ContentBlock::ToolUse { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Reasoning { .. }
        | ContentBlock::Text { .. } => None,
    })
}

fn has_interesting_extension(candidate: &str) -> bool {
    std::path::Path::new(candidate)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["rs", "ts", "tsx", "js", "json", "md"]
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

fn extract_file_candidates(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter_map(|token| {
            let candidate = token.trim_matches(|char: char| {
                matches!(char, ',' | '.' | ':' | ';' | ')' | '(' | '"' | '\'' | '`')
            });
            if candidate.contains('/') && has_interesting_extension(candidate) {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

fn estimate_message_tokens(message: &ConversationMessage) -> usize {
    message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len() / 4 + 1,
            ContentBlock::ToolUse { name, input, .. } => (name.len() + input.len()) / 4 + 1,
            ContentBlock::ToolResult {
                tool_name, output, ..
            } => (tool_name.len() + output.len()) / 4 + 1,
            ContentBlock::Reasoning { data } => data.len() / 4 + 1,
        })
        .sum()
}

fn extract_tag_block(content: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let start_index = content.find(&start)? + start.len();
    let end_index = content[start_index..].find(&end)? + start_index;
    Some(content[start_index..end_index].to_string())
}

fn strip_tag_block(content: &str, tag: &str) -> String {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    if let (Some(start_index), Some(end_index_rel)) = (content.find(&start), content.find(&end)) {
        let end_index = end_index_rel + end.len();
        let mut stripped = String::new();
        stripped.push_str(&content[..start_index]);
        stripped.push_str(&content[end_index..]);
        stripped
    } else {
        content.to_string()
    }
}

fn collapse_blank_lines(content: &str) -> String {
    let mut result = String::new();
    let mut last_blank = false;
    for line in content.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && last_blank {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        last_blank = is_blank;
    }
    result
}

#[allow(dead_code)]
fn extract_existing_compacted_summary(message: &ConversationMessage) -> Option<String> {
    if message.role != MessageRole::System {
        return None;
    }

    let text = first_text_block(message)?;
    let summary = text.strip_prefix(COMPACT_CONTINUATION_PREAMBLE)?;
    let summary = summary
        .split_once(&format!("\n\n{COMPACT_RECENT_MESSAGES_NOTE}"))
        .map_or(summary, |(value, _)| value);
    let summary = summary
        .split_once(&format!("\n{COMPACT_DIRECT_RESUME_INSTRUCTION}"))
        .map_or(summary, |(value, _)| value);
    Some(summary.trim().to_string())
}

#[allow(dead_code)]
fn compacted_summary_prefix_len(session: &Session) -> usize {
    usize::from(
        session
            .messages
            .first()
            .and_then(extract_existing_compacted_summary)
            .is_some(),
    )
}

fn prune_tool_outputs(messages: &mut [ConversationMessage]) {
    let mut cumulative_tokens: usize = 0;
    for msg in messages.iter_mut().rev() {
        if cumulative_tokens >= PRUNE_PROTECT_TOKENS {
            // Outside the protected window — truncate large ToolResult outputs
            for block in &mut msg.blocks {
                if let ContentBlock::ToolResult { output, .. } = block {
                    let char_count = output.chars().count();
                    if char_count > PRUNE_MAX_OUTPUT_CHARS {
                        let truncated: String =
                            output.chars().take(PRUNE_MAX_OUTPUT_CHARS).collect();
                        *output = format!(
                            "{truncated}\n\n[… output truncated from {char_count} chars]"
                        );
                    }
                }
            }
        }
        cumulative_tokens = cumulative_tokens.saturating_add(estimate_message_tokens(msg));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        collect_key_files, compact_session, estimate_session_tokens, format_compact_summary,
        get_compact_continuation_message, infer_pending_work, should_compact, CompactionConfig,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    #[test]
    fn formats_compact_summary_like_upstream() {
        let summary = "<analysis>scratch</analysis>\n<summary>Kept work</summary>";
        assert_eq!(format_compact_summary(summary), "Summary:\nKept work");
    }

    #[test]
    fn leaves_small_sessions_unchanged() {
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![ConversationMessage::user_text("hello")],
        };

        let result = compact_session(&session, CompactionConfig::default());
        assert_eq!(result.removed_message_count, 0);
        assert_eq!(result.compacted_session, session);
        assert!(result.summary.is_empty());
        assert!(result.formatted_summary.is_empty());
    }

    #[test]
    fn compacts_older_messages_into_a_system_summary() {
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text("one ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "two ".repeat(200),
                }]),
                ConversationMessage::user_text("three ".repeat(200)),
                ConversationMessage {
                    role: MessageRole::Assistant,
                    blocks: vec![ContentBlock::Text {
                        text: "recent".to_string(),
                    }],
                    usage: None,
                },
            ],
        };

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            },
        );

        assert_eq!(result.removed_message_count, 2);
        assert_eq!(
            result.compacted_session.messages[0].role,
            MessageRole::System
        );
        assert!(matches!(
            &result.compacted_session.messages[0].blocks[0],
            ContentBlock::Text { text } if text.contains("Summary:")
        ));
        assert!(result.formatted_summary.contains("Scope:"));
        assert!(result.formatted_summary.contains("Key timeline:"));
        assert!(should_compact(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            }
        ));
        assert!(
            estimate_session_tokens(&result.compacted_session) < estimate_session_tokens(&session)
        );
    }

    #[test]
    fn truncates_long_blocks_in_summary() {
        let summary = super::summarize_block(&ContentBlock::Text {
            text: "x".repeat(400),
        });
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= 161);
    }

    #[test]
    fn extracts_key_files_from_message_content() {
        let files = collect_key_files(&[ConversationMessage::user_text(
            "Update rust/crates/runtime/src/compact.rs and rust/crates/rusty-claude-cli/src/main.rs next.",
        )]);
        assert!(files.contains(&"rust/crates/runtime/src/compact.rs".to_string()));
        assert!(files.contains(&"rust/crates/rusty-claude-cli/src/main.rs".to_string()));
    }

    #[test]
    fn infers_pending_work_from_recent_messages() {
        let pending = infer_pending_work(&[
            ConversationMessage::user_text("done"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Next: update tests and follow up on remaining CLI polish.".to_string(),
            }]),
        ]);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains("Next: update tests"));
    }

    #[test]
    fn compaction_never_orphans_tool_result() {
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text("x ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "navigate".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("call_1", "navigate", "page content", false),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "call_2".to_string(),
                    name: "click".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("call_2", "click", "ok", false),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "done".to_string(),
                }]),
            ],
        };

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 3,
                max_estimated_tokens: 1,
            },
        );

        let preserved = &result.compacted_session.messages;
        assert_eq!(preserved[0].role, MessageRole::System);

        for message in &preserved[1..] {
            for block in &message.blocks {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    let has_matching_use = preserved.iter().any(|m| {
                        m.blocks.iter().any(|b| matches!(
                            b,
                            ContentBlock::ToolUse { id, .. } if id == tool_use_id
                        ))
                    });
                    assert!(
                        has_matching_use,
                        "orphaned tool_result with tool_use_id={tool_use_id}"
                    );
                }
            }
        }
    }

    #[test]
    fn compaction_pulls_back_past_tool_boundary() {
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text("x ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "navigate".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("call_1", "navigate", "page", false),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "final".to_string(),
                }]),
            ],
        };

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            },
        );

        let preserved = &result.compacted_session.messages;
        assert_ne!(preserved[1].role, MessageRole::Tool,
            "preserved window must not start with a Tool message");

        let has_tool_use = preserved.iter().any(|m| {
            m.blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "call_1"))
        });
        let has_tool_result = preserved.iter().any(|m| {
            m.blocks.iter().any(|b| matches!(
                b,
                ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1"
            ))
        });
        assert_eq!(has_tool_use, has_tool_result,
            "tool_use and tool_result for call_1 must both be present or both absent");
    }

    #[test]
    fn prefix_detection_finds_compacted_summary() {
        let summary = "<summary>Scope: 5 messages compacted.</summary>";
        let continuation = get_compact_continuation_message(summary, true, true);
        let msg = ConversationMessage {
            role: MessageRole::System,
            blocks: vec![ContentBlock::Text { text: continuation }],
            usage: None,
        };
        let result = super::extract_existing_compacted_summary(&msg);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Scope:"));
    }

    #[test]
    fn prefix_detection_returns_none_for_user_message() {
        let msg = ConversationMessage::user_text("hello");
        assert!(super::extract_existing_compacted_summary(&msg).is_none());
    }

    #[test]
    fn prefix_detection_returns_none_for_empty_session() {
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![],
        };
        assert_eq!(super::compacted_summary_prefix_len(&session), 0);
    }

    #[test]
    fn prune_tool_outputs_truncates_large_old_outputs() {
        let large_output = "x".repeat(10_000);
        // Need enough content after the large output to push it outside the 40K token window.
        // 40K tokens ≈ 160K chars. Use 42 messages of 4000 chars each (~42K tokens).
        let padding = "p".repeat(4_000);
        let mut messages = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "navigate".to_string(),
                input: "{}".to_string(),
            }]),
            ConversationMessage::tool_result("call_1", "navigate", &large_output, false),
        ];
        for _ in 0..42 {
            messages.push(ConversationMessage::user_text(&padding));
        }
        messages.push(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "done".to_string(),
        }]));

        super::prune_tool_outputs(&mut messages);

        let block = &messages[2].blocks[0];
        if let ContentBlock::ToolResult { output, .. } = block {
            assert!(
                output.contains("[… output truncated from 10000 chars]"),
                "large old output should be truncated"
            );
            assert!(
                output.chars().count() < 10_000,
                "truncated output should be shorter than original"
            );
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn prune_tool_outputs_small_outputs_unchanged() {
        let small_output = "small content";
        let mut messages = vec![
            ConversationMessage::user_text("start"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "navigate".to_string(),
                input: "{}".to_string(),
            }]),
            ConversationMessage::tool_result("call_1", "navigate", small_output, false),
        ];

        super::prune_tool_outputs(&mut messages);

        let block = &messages[2].blocks[0];
        if let ContentBlock::ToolResult { output, .. } = block {
            assert_eq!(output, small_output);
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn prune_tool_outputs_recent_outputs_protected() {
        let large_output = "z".repeat(10_000);
        let mut messages: Vec<ConversationMessage> = (0..200)
            .map(|i| {
                if i % 3 == 2 {
                    ConversationMessage::tool_result(
                        &format!("call_{i}"),
                        "navigate",
                        &large_output,
                        false,
                    )
                } else if i % 3 == 1 {
                    ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                        id: format!("call_{i}"),
                        name: "navigate".to_string(),
                        input: "{}".to_string(),
                    }])
                } else {
                    ConversationMessage::user_text("go")
                }
            })
            .collect();

        super::prune_tool_outputs(&mut messages);

        let last_tool_result = messages
            .iter()
            .rev()
            .find(|m| {
                m.blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            })
            .unwrap();

        let block = &last_tool_result.blocks[0];
        if let ContentBlock::ToolResult { output, .. } = block {
            assert!(
                !output.contains("[… output truncated"),
                "recent output should not be truncated"
            );
        }
    }

    #[test]
    fn prune_tool_outputs_non_tool_result_blocks_unchanged() {
        let large_text = "a".repeat(10_000);
        let mut messages = vec![
            ConversationMessage::user_text(&large_text),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: large_text.clone(),
            }]),
        ];

        super::prune_tool_outputs(&mut messages);

        if let ContentBlock::Text { text } = &messages[0].blocks[0] {
            assert_eq!(text.chars().count(), 10_000);
        }
        if let ContentBlock::Text { text } = &messages[1].blocks[0] {
            assert_eq!(text.chars().count(), 10_000);
        }
    }
}
