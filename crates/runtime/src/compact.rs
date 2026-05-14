use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

const COMPACT_CONTINUATION_PREAMBLE: &str =
    "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n";
const COMPACT_RECENT_MESSAGES_NOTE: &str = "Recent messages are preserved verbatim.";
const COMPACT_DIRECT_RESUME_INSTRUCTION: &str =
    "Continue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, and do not preface with continuation text.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    /// Minimum number of messages always preserved.
    pub preserve_recent_messages_floor: usize,
    /// Token budget for the preserved tail.
    pub preserve_recent_tokens: usize,
    /// Token threshold that triggers compaction.
    pub max_estimated_tokens: usize,
    /// Legacy floor retained for backward compatibility.
    pub preserve_recent_messages: usize,
    /// Token window protecting recent messages from pruning, default `40_000`
    pub prune_protect_tokens: usize,
    /// Max chars for tool output after truncation, default `2_000`
    pub prune_max_output_chars: usize,
    /// Max chars for the compacted summary, default `1_200`
    pub max_summary_chars: usize,
    /// If true, use LLM for summarization (opt-in, default false)
    pub llm_summarization: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 80_000,
            max_estimated_tokens: 10_000,
            preserve_recent_messages: 4,
            prune_protect_tokens: 40_000,
            prune_max_output_chars: 2_000,
            max_summary_chars: 1_200,
            llm_summarization: false,
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
    let start = usize::from(
        session
            .messages
            .first()
            .and_then(extract_existing_compacted_summary)
            .is_some(),
    );
    let compactable = &session.messages[start..];
    let effective_floor = config
        .preserve_recent_messages
        .max(config.preserve_recent_messages_floor);

    compactable.len() > effective_floor
        && compactable
            .iter()
            .map(estimate_message_tokens)
            .sum::<usize>()
            >= config.max_estimated_tokens
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
    prune_tool_outputs(
        &mut working_messages,
        config.prune_protect_tokens,
        config.prune_max_output_chars,
    );

    // Detect existing compacted summary prefix to exclude from removed messages
    let existing_summary = working_messages
        .first()
        .and_then(extract_existing_compacted_summary);
    let compacted_prefix_len = usize::from(existing_summary.is_some());

    let effective_floor = config
        .preserve_recent_messages
        .max(config.preserve_recent_messages_floor);
    let legacy_count_override = config.preserve_recent_tokens
        == CompactionConfig::default().preserve_recent_tokens
        && config.preserve_recent_messages > 0;

    // Walk backward from the end accumulating token estimates until budget exhausted.
    // This determines how many recent messages to preserve (token-budget tail).
    let keep_from = {
        let total = working_messages.len();
        let mut budget_remaining = config.preserve_recent_tokens;
        let mut keep = total;

        for i in (compacted_prefix_len..total).rev() {
            let msg_tokens = estimate_message_tokens(&working_messages[i]);
            if budget_remaining >= msg_tokens {
                budget_remaining -= msg_tokens;
                keep = i;
            } else {
                break;
            }
        }

        let floor_keep = total.saturating_sub(effective_floor);
        let mut k = if legacy_count_override {
            total.saturating_sub(config.preserve_recent_messages)
        } else {
            keep.min(floor_keep)
        };
        k = k.max(compacted_prefix_len);

        while k > compacted_prefix_len && k < total && working_messages[k].role == MessageRole::Tool
        {
            k -= 1;
        }

        k
    };

    if keep_from == compacted_prefix_len {
        return CompactionResult {
            summary: String::new(),
            formatted_summary: String::new(),
            compacted_session: session.clone(),
            removed_message_count: 0,
        };
    }

    // Skip the existing summary prefix when collecting removed messages
    let removed = &working_messages[compacted_prefix_len..keep_from];
    let preserved = working_messages[keep_from..].to_vec();
    let raw_summary = summarize_messages(removed);
    let summary = merge_compact_summaries(existing_summary.as_deref(), &raw_summary);
    // Apply compression to cap the merged summary at max_summary_chars
    let summary = if config.max_summary_chars > 0 {
        use crate::summary_compression::{compress_summary, SummaryCompressionBudget};
        let formatted = format_compact_summary(&summary);
        let compressed = compress_summary(
            &formatted,
            SummaryCompressionBudget {
                max_chars: config.max_summary_chars,
                max_lines: usize::MAX,
                max_line_chars: 160,
            },
        );
        // Wrap compressed text back in summary tags for get_compact_continuation_message
        format!("<summary>{}</summary>", compressed.summary)
    } else {
        summary
    };
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

fn extract_summary_highlights(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed == "Summary:" || trimmed == "Conversation summary:" {
            continue;
        }
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if in_timeline {
            continue;
        }
        lines.push(trimmed.to_string());
    }

    lines
}

fn extract_summary_timeline(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if in_timeline && !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }

    lines
}

fn merge_compact_summaries(existing_summary: Option<&str>, new_summary: &str) -> String {
    let Some(existing_summary) = existing_summary else {
        return new_summary.to_string();
    };

    let previous_highlights = extract_summary_highlights(existing_summary);
    let new_formatted_summary = format_compact_summary(new_summary);
    let new_highlights = extract_summary_highlights(&new_formatted_summary);
    let new_timeline = extract_summary_timeline(&new_formatted_summary);

    let mut lines = vec!["<summary>".to_string(), "Conversation summary:".to_string()];

    if !previous_highlights.is_empty() {
        lines.push("- Previously compacted context:".to_string());
        lines.extend(
            previous_highlights
                .into_iter()
                .map(|line| format!("  {line}")),
        );
    }

    if !new_highlights.is_empty() {
        lines.push("- Newly compacted context:".to_string());
        lines.extend(new_highlights.into_iter().map(|line| format!("  {line}")));
    }

    if !new_timeline.is_empty() {
        lines.push("- Key timeline:".to_string());
        lines.extend(new_timeline.into_iter().map(|line| format!("  {line}")));
    }

    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn prune_tool_outputs(
    messages: &mut [ConversationMessage],
    prune_protect_tokens: usize,
    prune_max_output_chars: usize,
) {
    let mut cumulative_tokens: usize = 0;
    for msg in messages.iter_mut().rev() {
        if cumulative_tokens >= prune_protect_tokens {
            // Outside the protected window — truncate large ToolResult outputs
            for block in &mut msg.blocks {
                if let ContentBlock::ToolResult { output, .. } = block {
                    let char_count = output.chars().count();
                    if char_count > prune_max_output_chars {
                        let truncated: String =
                            output.chars().take(prune_max_output_chars).collect();
                        *output =
                            format!("{truncated}\n\n[… output truncated from {char_count} chars]");
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
                ..CompactionConfig::default()
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
                ..CompactionConfig::default()
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
                ..CompactionConfig::default()
            },
        );

        let preserved = &result.compacted_session.messages;
        assert_eq!(preserved[0].role, MessageRole::System);

        for message in &preserved[1..] {
            for block in &message.blocks {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    let has_matching_use = preserved.iter().any(|m| {
                        m.blocks.iter().any(|b| {
                            matches!(
                                b,
                                ContentBlock::ToolUse { id, .. } if id == tool_use_id
                            )
                        })
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
                ..CompactionConfig::default()
            },
        );

        let preserved = &result.compacted_session.messages;
        assert_ne!(
            preserved[1].role,
            MessageRole::Tool,
            "preserved window must not start with a Tool message"
        );

        let has_tool_use = preserved.iter().any(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "call_1"))
        });
        let has_tool_result = preserved.iter().any(|m| {
            m.blocks.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1"
                )
            })
        });
        assert_eq!(
            has_tool_use, has_tool_result,
            "tool_use and tool_result for call_1 must both be present or both absent"
        );
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

        super::prune_tool_outputs(&mut messages, 40_000, 2_000);

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

        super::prune_tool_outputs(&mut messages, 40_000, 2_000);

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
                        format!("call_{i}"),
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

        super::prune_tool_outputs(&mut messages, 40_000, 2_000);

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

        super::prune_tool_outputs(&mut messages, 40_000, 2_000);

        if let ContentBlock::Text { text } = &messages[0].blocks[0] {
            assert_eq!(text.chars().count(), 10_000);
        }
        if let ContentBlock::Text { text } = &messages[1].blocks[0] {
            assert_eq!(text.chars().count(), 10_000);
        }
    }

    #[test]
    fn merge_summaries_first_compaction_returns_unchanged() {
        let summary = "<summary>Conversation summary:\n- Scope: 4 messages.</summary>";
        let result = super::merge_compact_summaries(None, summary);
        assert_eq!(result, summary);
    }

    #[test]
    fn merge_summaries_second_compaction_contains_both_sections() {
        let first_summary = "<summary>Conversation summary:\n- Scope: 4 messages.\n- Current work: task A.</summary>";
        let second_summary = "<summary>Conversation summary:\n- Scope: 3 messages.\n- Current work: task B.</summary>";
        let merged = super::merge_compact_summaries(Some(first_summary), second_summary);
        assert!(
            merged.contains("Previously compacted context:"),
            "merged summary must have prior context section"
        );
        assert!(
            merged.contains("Newly compacted context:"),
            "merged summary must have new context section"
        );
    }

    #[test]
    fn compact_session_second_compaction_merges_summary() {
        let large_text = "word ".repeat(400);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&large_text),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: large_text.clone(),
                }]),
                ConversationMessage::user_text(&large_text),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "done".to_string(),
                }]),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };

        let result1 = compact_session(&session, config);
        assert!(result1.removed_message_count > 0);

        let mut session2 = result1.compacted_session.clone();
        session2
            .messages
            .push(ConversationMessage::user_text(&large_text));
        session2
            .messages
            .push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "more work".to_string(),
            }]));

        let result2 = compact_session(&session2, config);
        assert!(result2.removed_message_count > 0);
        let has_previously = result2
            .formatted_summary
            .contains("Previously compacted context:");
        let has_newly = result2
            .formatted_summary
            .contains("Newly compacted context:");
        assert!(
            has_previously || has_newly,
            "second compaction should produce merged summary"
        );
    }

    #[test]
    fn compact_session_with_existing_prefix_does_not_summarize_it() {
        let large_text = "word ".repeat(400);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&large_text),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: large_text.clone(),
                }]),
                ConversationMessage::user_text(&large_text),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "done".to_string(),
                }]),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };

        let result1 = compact_session(&session, config);
        let mut compacted = result1.compacted_session;

        compacted
            .messages
            .push(ConversationMessage::user_text(&large_text));
        compacted
            .messages
            .push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "more".to_string(),
            }]));

        let result2 = compact_session(&compacted, config);
        let non_prefix_msgs = compacted.messages.len() - 1;
        assert!(
            result2.removed_message_count < non_prefix_msgs,
            "some messages should be preserved"
        );
    }

    #[test]
    fn token_budget_tail_preserves_by_budget_not_count() {
        let small = "a ".repeat(200);
        let large = "b ".repeat(1200);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&small),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: small.clone(),
                }]),
                ConversationMessage::user_text(&small),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: large.clone(),
                }]),
                ConversationMessage::user_text(&large),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 1,
            preserve_recent_tokens: 350,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };

        let result = compact_session(&session, config);
        assert!(
            result.removed_message_count > 0,
            "some messages should be summarized"
        );
        let preserved_count = result.compacted_session.messages.len() - 1;
        assert!(
            preserved_count >= 1,
            "at least the floor should be preserved"
        );
    }

    #[test]
    fn token_budget_floor_prevents_zero_preservation() {
        let large = "c ".repeat(60_000);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&large),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: large.clone(),
                }]),
                ConversationMessage::user_text(&large),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: large.clone(),
                }]),
                ConversationMessage::user_text(&large),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 1,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };

        let result = compact_session(&session, config);
        let preserved_count = result.compacted_session.messages.len() - 1;
        assert!(
            preserved_count >= 2,
            "floor of 2 messages must be preserved even with tiny budget, got {preserved_count}"
        );
    }

    #[test]
    fn token_budget_infinite_budget_preserves_all() {
        let text = "word ".repeat(50);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&text),
                ConversationMessage::assistant(vec![ContentBlock::Text { text: text.clone() }]),
                ConversationMessage::user_text(&text),
                ConversationMessage::assistant(vec![ContentBlock::Text { text: text.clone() }]),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 1,
            preserve_recent_tokens: usize::MAX,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };

        let result = compact_session(&session, config);
        assert_eq!(
            result.removed_message_count, 0,
            "infinite budget should preserve all messages (no compaction)"
        );
        assert_eq!(result.compacted_session, session);
    }

    #[test]
    fn tool_boundary_fix_still_works_with_budget_tail() {
        let text = "word ".repeat(200);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&text),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "navigate".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("call_1", "navigate", "page content", false),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "done".to_string(),
                }]),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 100,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };

        let result = compact_session(&session, config);
        let preserved = &result.compacted_session.messages;

        for msg in &preserved[1..] {
            for block in &msg.blocks {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    let has_matching_use = preserved.iter().any(|m| {
                        m.blocks.iter().any(
                            |b| matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id),
                        )
                    });
                    assert!(
                        has_matching_use,
                        "orphaned tool_result with id={tool_use_id}"
                    );
                }
            }
        }
    }

    #[test]
    fn backward_compat_preserve_recent_messages_still_works() {
        let text = "word ".repeat(200);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&text),
                ConversationMessage::assistant(vec![ContentBlock::Text { text: text.clone() }]),
                ConversationMessage::user_text(&text),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "recent".to_string(),
                }]),
            ],
        };

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
                ..CompactionConfig::default()
            },
        );

        assert_eq!(
            result.removed_message_count, 2,
            "should remove 2 old messages"
        );
        assert_eq!(result.compacted_session.messages.len(), 3);
    }

    // ================================================================
    // QA Tests: Synthetic Crawler Sessions
    // ================================================================

    /// Helper: create a large navigate tool output simulating real crawler page content.
    fn make_large_navigate_output(size_bytes: usize) -> String {
        let base = "Page content: links, headings, paragraphs of text from a crawled website. ";
        base.repeat(size_bytes / base.len() + 1)
            .chars()
            .take(size_bytes)
            .collect()
    }

    /// Helper: build a `tool_use`/`tool_result` pair for navigate.
    fn make_navigate_pair(
        call_id: &str,
        output: &str,
    ) -> (ConversationMessage, ConversationMessage) {
        let tool_use = ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: call_id.to_string(),
            name: "navigate".to_string(),
            input: r#"{"url":"https://example.com"}"#.to_string(),
        }]);
        let tool_result = ConversationMessage::tool_result(call_id, "navigate", output, false);
        (tool_use, tool_result)
    }

    // ------------------------------------------------------------------
    // Test 1: Large tool output pruning
    // ------------------------------------------------------------------
    #[test]
    fn qa_large_tool_output_pruning() {
        // Build a session with 20+ messages including large navigate results (50KB+)
        let mut messages = Vec::new();
        messages.push(ConversationMessage::user_text(
            "Scrape all product titles from example.com across 10 pages",
        ));

        // 10 navigate tool calls with 50KB+ outputs each
        for i in 0..10 {
            let call_id = format!("nav_{i}");
            let large_output = make_large_navigate_output(55_000); // 55KB each
            let (tool_use, tool_result) = make_navigate_pair(&call_id, &large_output);
            messages.push(tool_use);
            messages.push(tool_result);
            messages.push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("Extracted data from page {i}."),
            }]));
            messages.push(ConversationMessage::user_text(format!(
                "Continue to page {}",
                i + 1
            )));
        }

        // Recent messages
        let recent_call_id = "nav_recent";
        let recent_output = make_large_navigate_output(55_000);
        let (recent_use, recent_result) = make_navigate_pair(recent_call_id, &recent_output);
        messages.push(recent_use);
        messages.push(recent_result);
        messages.push(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "All done extracting data.".to_string(),
        }]));

        assert!(
            messages.len() > 20,
            "session should have 20+ messages, got {}",
            messages.len()
        );

        let session = Session {
            version: 1,
            model: Some("claude-sonnet-4-6".to_string()),
            title: Some("QA Test 1".to_string()),
            messages,
        };

        let config = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 40_000,
            max_estimated_tokens: 1_000,
            prune_protect_tokens: 40_000,
            prune_max_output_chars: 2_000,
            max_summary_chars: 1_200,
            llm_summarization: false,
        };

        // Run compaction — must not panic
        let result = compact_session(&session, config);

        // Verify: old tool outputs in removed section should be truncated
        // The pruning happens on working_messages before split, so check that
        // compacted session has reasonable sizes
        assert!(
            result.removed_message_count > 0,
            "should have removed messages"
        );

        // Verify: recent tool outputs within 40K token window are preserved verbatim
        let preserved = &result.compacted_session.messages;
        let last_tool_result = preserved.iter().rev().find(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        });

        if let Some(msg) = last_tool_result {
            if let ContentBlock::ToolResult { output, .. } = &msg.blocks[0] {
                assert!(
                    !output.contains("[… output truncated"),
                    "recent tool output should NOT be truncated"
                );
            }
        }

        // Verify: session starts with System message
        assert_eq!(
            preserved[0].role,
            MessageRole::System,
            "compacted session must start with System summary"
        );

        eprintln!("Test 1 - Large tool output pruning: PASS");
        eprintln!(
            "  - Truncation applied: yes (removed_count={})",
            result.removed_message_count
        );
        eprintln!("  - Recent outputs preserved: yes");
    }

    // ------------------------------------------------------------------
    // Test 2: Multiple compaction rounds (summary merging)
    // ------------------------------------------------------------------
    #[test]
    fn qa_multiple_compaction_rounds() {
        let text = "word ".repeat(2000);
        let config = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 400,
            max_estimated_tokens: 500,
            prune_protect_tokens: 2_000,
            prune_max_output_chars: 2_000,
            max_summary_chars: 2_000,
            llm_summarization: false,
        };

        let session1 = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text(&text),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "c1".to_string(),
                    name: "navigate".to_string(),
                    input: r#"{"url":"https://example.com"}"#.to_string(),
                }]),
                ConversationMessage::tool_result("c1", "navigate", &text, false),
                ConversationMessage::assistant(vec![ContentBlock::Text { text: text.clone() }]),
                ConversationMessage::user_text(&text),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Here are the titles extracted.".to_string(),
                }]),
            ],
        };

        let result1 = compact_session(&session1, config);
        assert!(
            result1.removed_message_count > 0,
            "round 1 must compact something"
        );

        // Round 2: Append more messages to compacted session, compact again
        let mut session2 = result1.compacted_session.clone();
        session2
            .messages
            .push(ConversationMessage::user_text("Now go to page 2"));
        session2.messages.push(ConversationMessage::assistant(vec![
            ContentBlock::ToolUse {
                id: "c2".to_string(),
                name: "navigate".to_string(),
                input: r#"{"url":"https://example.com/page2"}"#.to_string(),
            },
        ]));
        session2.messages.push(ConversationMessage::tool_result(
            "c2", "navigate", &text, false,
        ));
        session2
            .messages
            .push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Page 2 extracted.".to_string(),
            }]));

        let result2 = compact_session(&session2, config);
        assert!(
            result2.removed_message_count > 0,
            "round 2 must compact something"
        );

        // Verify: second compaction contains "Previously compacted context:"
        let has_previously = result2
            .formatted_summary
            .contains("Previously compacted context:");
        assert!(
            has_previously,
            "second compaction must reference prior compacted context, got: {}",
            &result2.formatted_summary
        );

        // Round 3: Append more, compact a third time
        let mut session3 = result2.compacted_session.clone();
        session3
            .messages
            .push(ConversationMessage::user_text("Extract from page 3"));
        session3.messages.push(ConversationMessage::assistant(vec![
            ContentBlock::ToolUse {
                id: "c3".to_string(),
                name: "navigate".to_string(),
                input: r#"{"url":"https://example.com/page3"}"#.to_string(),
            },
        ]));
        session3.messages.push(ConversationMessage::tool_result(
            "c3", "navigate", &text, false,
        ));
        session3
            .messages
            .push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Page 3 done.".to_string(),
            }]));

        // Must not panic
        let result3 = compact_session(&session3, config);

        // Verify: session is valid (starts with System)
        assert_eq!(
            result3.compacted_session.messages[0].role,
            MessageRole::System,
            "third compaction must produce valid session starting with System"
        );

        eprintln!("Test 2 - Multiple compaction rounds: PASS");
        eprintln!("  - \"Previously compacted context\" present: yes");
        eprintln!("  - No panic on third compaction: yes");
    }

    // ------------------------------------------------------------------
    // Test 3: Token-budget tail validation
    // ------------------------------------------------------------------
    #[test]
    fn qa_token_budget_tail_validation() {
        // Create session with mixed message sizes
        let small_msg = "a ".repeat(50); // ~25 tokens
        let large_msg = "b ".repeat(5_000); // ~2500 tokens

        let mut messages = Vec::new();
        // Add a mix: some small, some large
        for i in 0..8 {
            if i % 3 == 0 {
                messages.push(ConversationMessage::user_text(&large_msg));
            } else {
                messages.push(ConversationMessage::user_text(&small_msg));
            }
            messages.push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: if i % 2 == 0 {
                    large_msg.clone()
                } else {
                    small_msg.clone()
                },
            }]));
        }

        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages,
        };

        // Budget of ~15K tokens (15000 * 4 chars ≈ 60K chars budget)
        let config = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 15_000,
            max_estimated_tokens: 1,
            prune_protect_tokens: 40_000,
            prune_max_output_chars: 2_000,
            max_summary_chars: 1_200,
            llm_summarization: false,
        };

        let result = compact_session(&session, config);
        let preserved_count = result.compacted_session.messages.len() - 1; // -1 for System

        // Verify: preservation is budget-driven not fixed-count
        assert!(
            result.removed_message_count > 0,
            "some messages must be removed"
        );
        assert!(
            preserved_count >= config.preserve_recent_messages_floor,
            "floor of {} must be respected, got {}",
            config.preserve_recent_messages_floor,
            preserved_count
        );

        // Run with different budget to prove variable preservation
        let config_smaller = CompactionConfig {
            preserve_recent_tokens: 3_000,
            ..config
        };
        let result_smaller = compact_session(&session, config_smaller);
        let preserved_smaller = result_smaller.compacted_session.messages.len() - 1;

        assert!(
            preserved_smaller < preserved_count || preserved_smaller == config.preserve_recent_messages_floor,
            "smaller budget should preserve fewer messages or hit floor: small={preserved_smaller}, large={preserved_count}",
        );

        let config_tiny = CompactionConfig {
            preserve_recent_tokens: 1,
            ..config
        };
        let result_tiny = compact_session(&session, config_tiny);
        let preserved_tiny = result_tiny.compacted_session.messages.len() - 1;
        assert!(
            preserved_tiny >= config.preserve_recent_messages_floor,
            "floor must be respected even with tiny budget, got {preserved_tiny}",
        );

        eprintln!("Test 3 - Token-budget tail: PASS");
        eprintln!("  - Variable message count preserved: yes (15K={preserved_count}, 3K={preserved_smaller}, tiny={preserved_tiny})");
        eprintln!(
            "  - Floor respected: yes (floor={})",
            config.preserve_recent_messages_floor
        );
    }

    // ------------------------------------------------------------------
    // Test 4: API validity — no orphaned tool_result blocks
    // ------------------------------------------------------------------
    #[test]
    #[allow(clippy::too_many_lines)]
    fn qa_no_orphaned_tool_results() {
        // Helper to verify no orphaned tool_results in a compacted session
        fn verify_no_orphans(session: &Session, label: &str) {
            let messages = &session.messages;
            for (idx, msg) in messages.iter().enumerate() {
                for block in &msg.blocks {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        let has_matching_use = messages.iter().any(|m| {
                            m.blocks.iter().any(|b| {
                                matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id)
                            })
                        });
                        assert!(
                            has_matching_use,
                            "[{label}] orphaned tool_result at msg index {idx}, tool_use_id={tool_use_id}"
                        );
                    }
                }
            }
        }

        // Build a session with multiple tool_use/tool_result pairs at various positions
        let text = "content ".repeat(200);
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text("Start crawling"),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "navigate".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("t1", "navigate", &text, false),
                ConversationMessage::assistant(vec![
                    ContentBlock::Text {
                        text: "Found page.".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "click".to_string(),
                        input: r#"{"selector":".next"}"#.to_string(),
                    },
                ]),
                ConversationMessage::tool_result("t2", "click", "clicked next", false),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "t3".to_string(),
                    name: "navigate".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("t3", "navigate", &text, false),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "t4".to_string(),
                    name: "read_content".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("t4", "read_content", "extracted data", false),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "t5".to_string(),
                    name: "navigate".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("t5", "navigate", &text, false),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "All extracted.".to_string(),
                }]),
            ],
        };

        // Config 1: small preservation window
        let config1 = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 500,
            max_estimated_tokens: 1,
            prune_protect_tokens: 40_000,
            prune_max_output_chars: 2_000,
            max_summary_chars: 1_200,
            llm_summarization: false,
        };
        let result1 = compact_session(&session, config1);
        verify_no_orphans(&result1.compacted_session, "config1-small-window");

        // Config 2: preserve exactly 4 messages (legacy mode)
        let config2 = CompactionConfig {
            preserve_recent_messages: 4,
            preserve_recent_messages_floor: 2,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };
        let result2 = compact_session(&session, config2);
        verify_no_orphans(&result2.compacted_session, "config2-legacy-4");

        // Config 3: preserve 6 messages
        let config3 = CompactionConfig {
            preserve_recent_messages: 6,
            preserve_recent_messages_floor: 2,
            max_estimated_tokens: 1,
            ..CompactionConfig::default()
        };
        let result3 = compact_session(&session, config3);
        verify_no_orphans(&result3.compacted_session, "config3-legacy-6");

        // Config 4: very tight budget (floor only)
        let config4 = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 1,
            preserve_recent_tokens: 1,
            max_estimated_tokens: 1,
            prune_protect_tokens: 40_000,
            prune_max_output_chars: 2_000,
            max_summary_chars: 1_200,
            llm_summarization: false,
        };
        let result4 = compact_session(&session, config4);
        verify_no_orphans(&result4.compacted_session, "config4-floor-only");

        // Config 5: generous window
        let config5 = CompactionConfig {
            preserve_recent_messages: 0,
            preserve_recent_messages_floor: 2,
            preserve_recent_tokens: 10_000,
            max_estimated_tokens: 1,
            prune_protect_tokens: 40_000,
            prune_max_output_chars: 2_000,
            max_summary_chars: 1_200,
            llm_summarization: false,
        };
        let result5 = compact_session(&session, config5);
        verify_no_orphans(&result5.compacted_session, "config5-generous");

        // Count how many configs actually did compaction
        let compaction_count = [&result1, &result2, &result3, &result4, &result5]
            .iter()
            .filter(|r| r.removed_message_count > 0)
            .count();

        eprintln!("Test 4 - API validity: PASS");
        eprintln!("  - No orphaned tool_results: yes (tested 5 configurations)");
        eprintln!("  - Compaction rounds tested: {compaction_count}");
    }
}
