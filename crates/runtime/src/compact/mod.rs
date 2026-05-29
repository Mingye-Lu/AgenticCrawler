use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

mod summarize;
#[cfg(test)]
mod tests;
mod transform;

use summarize::{estimate_message_tokens, summarize_messages};
pub use transform::is_compact_continuation_message;
use transform::{
    collapse_blank_lines, extract_existing_compacted_summary, extract_tag_block,
    merge_compact_summaries, prune_tool_outputs, strip_tag_block,
};

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
    let summary: String = if config.max_summary_chars > 0 {
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
            child_sessions: session.child_sessions.clone(),
        },
        removed_message_count: removed.len(),
    }
}
