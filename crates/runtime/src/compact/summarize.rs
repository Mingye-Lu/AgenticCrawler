use crate::session::{ContentBlock, ConversationMessage, MessageRole};

pub(super) fn summarize_messages(messages: &[ConversationMessage]) -> String {
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
            ContentBlock::ToolResult { tool_name, .. }
            | ContentBlock::ToolResultImage { tool_name, .. } => Some(tool_name.as_str()),
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

    let key_urls = collect_key_urls(messages);
    if !key_urls.is_empty() {
        lines.push(format!("- Key URLs visited: {}.", key_urls.join(", ")));
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

pub(super) fn summarize_block(block: &ContentBlock) -> String {
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
        ContentBlock::ToolResultImage {
            tool_name, caption, ..
        } => {
            format!("tool_result_image {tool_name}: {caption}")
        }
    };
    truncate_summary(&raw, 160)
}

pub(super) fn collect_recent_role_summaries(
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

pub(super) fn infer_pending_work(messages: &[ConversationMessage]) -> Vec<String> {
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

pub(super) fn collect_key_urls(messages: &[ConversationMessage]) -> Vec<String> {
    let urls = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::ToolUse { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Reasoning { .. }
            | ContentBlock::ToolResultImage { .. } => None,
        })
        .flat_map(extract_url_candidates)
        .collect::<Vec<_>>();
    let mut recent_unique = Vec::new();
    for url in urls.iter().rev() {
        if recent_unique.contains(url) {
            continue;
        }
        recent_unique.push(url.clone());
        if recent_unique.len() == 10 {
            break;
        }
    }
    recent_unique.into_iter().rev().collect()
}

pub(super) fn infer_current_work(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .find(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, 200))
}

pub(super) fn first_text_block(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
        ContentBlock::ToolUse { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Reasoning { .. }
        | ContentBlock::ToolResultImage { .. }
        | ContentBlock::Text { .. } => None,
    })
}

pub(super) fn extract_url_candidates(content: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut pos = 0;

    while pos < content.len() {
        let remaining = &content[pos..];
        let next_http = remaining.find("http://");
        let next_https = remaining.find("https://");
        let start = match (next_http, next_https) {
            (Some(http), Some(https)) => http.min(https),
            (Some(http), None) => http,
            (None, Some(https)) => https,
            (None, None) => break,
        };

        let abs_start = pos + start;
        let end = content[abs_start..]
            .find(|c: char| c.is_whitespace() || "\"'<>)]},".contains(c))
            .map_or(content.len(), |offset| abs_start + offset);

        let candidate = content[abs_start..end].trim_end_matches(|c: char| ".,;:!?".contains(c));
        if !candidate.is_empty() {
            urls.push(candidate.to_string());
        }
        pos = end;
    }

    urls
}

pub(super) fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

pub(super) fn estimate_message_tokens(message: &ConversationMessage) -> usize {
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
            ContentBlock::ToolResultImage {
                tool_name,
                caption,
                base64_data,
                ..
            } => (tool_name.len() + caption.len() + base64_data.len()) / 4 + 1,
        })
        .sum()
}
