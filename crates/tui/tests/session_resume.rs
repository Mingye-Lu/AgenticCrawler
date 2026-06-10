use std::collections::HashMap;

use acrawl_core::message::{ContentBlock, ConversationMessage};
use acrawl_tui::child_tabs::hydrate_from_child_sessions;
use acrawl_tui::repl_render::build_wrapped_list;
use acrawl_tui::tool_pairing::{build_tool_result_index, ToolResultInfo};
use runtime::ChildSession;

fn render_messages(
    messages: &[ConversationMessage],
    tool_results: &HashMap<String, ToolResultInfo>,
) -> (Vec<ratatui::widgets::ListItem<'static>>, Vec<String>) {
    build_wrapped_list(messages, tool_results, &[], 80, None, '⠋', false)
}

#[test]
fn empty_session_renders_nothing_but_padding() {
    let messages: Vec<ConversationMessage> = vec![];
    let tool_results = HashMap::new();

    let (items, text_lines) = render_messages(&messages, &tool_results);

    assert!(!items.is_empty(), "expected at least 1 padding item");
    assert!(
        items.len() <= 2,
        "expected at most 2 items for empty session, got {}",
        items.len()
    );
    assert!(
        text_lines.len() <= 2,
        "expected at most 2 text lines for empty session, got {}",
        text_lines.len()
    );
}

#[test]
fn user_and_assistant_both_visible() {
    let messages = vec![
        ConversationMessage::user_text("Hello"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "World".to_string(),
        }]),
    ];
    let tool_results = build_tool_result_index(&messages);

    let (_items, text_lines) = render_messages(&messages, &tool_results);

    let joined = text_lines.join("\n");
    assert!(
        joined.contains("You "),
        "expected user prefix 'You ' in output, got: {joined}"
    );
    assert!(
        joined.contains("World"),
        "expected assistant text 'World' in output, got: {joined}"
    );
}

#[test]
fn tool_use_with_result_shows_success() {
    let messages = vec![
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "navigate".to_string(),
            input: r#"{"url": "https://example.com"}"#.to_string(),
        }]),
        ConversationMessage::tool_result("t1", "navigate", "Page loaded successfully", false),
    ];
    let tool_results = build_tool_result_index(&messages);

    let (_items, text_lines) = render_messages(&messages, &tool_results);

    let joined = text_lines.join("\n");
    assert!(
        joined.contains("navigate"),
        "expected 'navigate' tool name in output, got: {joined}"
    );
    assert!(
        !joined.contains("Interrupted"),
        "should not show 'Interrupted' when result exists, got: {joined}"
    );
}

#[test]
fn tool_use_without_result_shows_interrupted() {
    let messages = vec![ConversationMessage::assistant(vec![
        ContentBlock::ToolUse {
            id: "t2".to_string(),
            name: "click".to_string(),
            input: r#"{"selector": ".btn"}"#.to_string(),
        },
    ])];
    let tool_results: HashMap<String, ToolResultInfo> = HashMap::new();

    let (_items, text_lines) = render_messages(&messages, &tool_results);

    let joined = text_lines.join("\n");
    let has_tool_name = joined.contains("click");
    let has_interrupted = joined.contains("Interrupted");
    assert!(
        has_tool_name || has_interrupted,
        "expected 'click' or 'Interrupted' in output, got: {joined}"
    );
}

#[test]
fn twenty_plus_messages_render_without_panic() {
    let mut messages = Vec::new();
    for i in 0..22 {
        if i % 2 == 0 {
            messages.push(ConversationMessage::user_text(format!("Message {i}")));
        } else {
            messages.push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("Reply {i}"),
            }]));
        }
    }
    let tool_results = build_tool_result_index(&messages);

    let (items, _text_lines) = render_messages(&messages, &tool_results);

    assert!(
        items.len() > 20,
        "expected more than 20 items for 22 messages, got {}",
        items.len()
    );
}

#[test]
fn session_with_child_sessions_populates_tabs() {
    let sessions = vec![
        ChildSession {
            id: "c1".to_string(),
            goal: "scrape prices".to_string(),
            messages: vec![
                ConversationMessage::user_text("scrape prices from page"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Found 5 prices".to_string(),
                }]),
            ],
        },
        ChildSession {
            id: "c2".to_string(),
            goal: "fetch reviews".to_string(),
            messages: vec![ConversationMessage::user_text("fetch reviews")],
        },
    ];

    let panel = hydrate_from_child_sessions(&sessions);

    assert_eq!(
        panel.tabs.len(),
        2,
        "expected 2 child tabs, got {}",
        panel.tabs.len()
    );
}

#[test]
fn compacted_session_renders_system_summary() {
    use acrawl_core::message::{ConversationMessage, MessageRole};
    let summary_text =
        "This session is being continued. Summary: Previously we scraped 50 pages of book data.";
    let messages = vec![
        ConversationMessage {
            role: MessageRole::System,
            blocks: vec![ContentBlock::Text {
                text: summary_text.to_string(),
            }],
            usage: None,
        },
        ConversationMessage::user_text("continue"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Continuing from where we left off.".to_string(),
        }]),
    ];
    let tool_results = build_tool_result_index(&messages);

    let (_items, text_lines) = render_messages(&messages, &tool_results);

    let joined = text_lines.join("\n");
    assert!(
        joined.contains("Summary"),
        "compacted summary (System message) must be visible in resumed transcript, got: {joined}"
    );
    assert!(
        joined.contains("Continuing from where"),
        "assistant reply must also be visible, got: {joined}"
    );
}
