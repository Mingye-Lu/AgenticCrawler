use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

use super::{
    format_compact_summary, summarize::estimate_message_tokens, summarize::first_text_block,
    COMPACT_CONTINUATION_PREAMBLE, COMPACT_DIRECT_RESUME_INSTRUCTION, COMPACT_RECENT_MESSAGES_NOTE,
};

pub(super) fn extract_tag_block(content: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let start_index = content.find(&start)? + start.len();
    let end_index = content[start_index..].find(&end)? + start_index;
    Some(content[start_index..end_index].to_string())
}

pub(super) fn strip_tag_block(content: &str, tag: &str) -> String {
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

pub(super) fn collapse_blank_lines(content: &str) -> String {
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

/// Returns `true` when `message` is the synthetic system message produced by
/// a prior compaction (i.e. starts with [`COMPACT_CONTINUATION_PREAMBLE`]).
///
/// Callers that need to skip the prior compaction prefix when slicing the
/// removed range should go through this helper instead of substring-matching
/// the preamble themselves — that keeps the wording in one place.
#[must_use]
pub fn is_compact_continuation_message(message: &ConversationMessage) -> bool {
    extract_existing_compacted_summary(message).is_some()
}

pub(super) fn extract_existing_compacted_summary(message: &ConversationMessage) -> Option<String> {
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
pub(super) fn compacted_summary_prefix_len(session: &Session) -> usize {
    usize::from(
        session
            .messages
            .first()
            .and_then(extract_existing_compacted_summary)
            .is_some(),
    )
}

pub(super) fn extract_summary_highlights(summary: &str) -> Vec<String> {
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

pub(super) fn extract_summary_timeline(summary: &str) -> Vec<String> {
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

pub(super) fn merge_compact_summaries(existing_summary: Option<&str>, new_summary: &str) -> String {
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

pub(super) fn prune_tool_outputs(
    messages: &mut [ConversationMessage],
    prune_protect_tokens: usize,
    prune_max_output_chars: usize,
) {
    let mut cumulative_tokens: usize = 0;
    for msg in messages.iter_mut().rev() {
        if cumulative_tokens >= prune_protect_tokens {
            // Outside the protected window — truncate large ToolResult outputs
            // and strip image payloads from ToolResultImage blocks entirely
            // (keeping only the caption), since images dominate token estimates
            // and would otherwise silently bloat the preserved context.
            for block in &mut msg.blocks {
                match block {
                    ContentBlock::ToolResult { output, .. } => {
                        let char_count = output.chars().count();
                        if char_count > prune_max_output_chars {
                            let truncated: String =
                                output.chars().take(prune_max_output_chars).collect();
                            *output = format!(
                                "{truncated}\n\n[… output truncated from {char_count} chars]"
                            );
                        }
                    }
                    ContentBlock::ToolResultImage {
                        tool_use_id,
                        tool_name,
                        caption,
                        is_error,
                        ..
                    } => {
                        let replacement = ContentBlock::ToolResult {
                            tool_use_id: std::mem::take(tool_use_id),
                            tool_name: std::mem::take(tool_name),
                            output: format!("{caption} [image removed by compaction]"),
                            is_error: *is_error,
                        };
                        *block = replacement;
                    }
                    ContentBlock::Text { .. }
                    | ContentBlock::ToolUse { .. }
                    | ContentBlock::Reasoning { .. } => {}
                }
            }
        }
        cumulative_tokens = cumulative_tokens.saturating_add(estimate_message_tokens(msg));
    }
}
