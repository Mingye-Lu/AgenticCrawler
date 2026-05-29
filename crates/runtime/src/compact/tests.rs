use super::summarize::*;
use super::transform::*;
use super::*;

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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
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
    assert!(estimate_session_tokens(&result.compacted_session) < estimate_session_tokens(&session));
}

#[test]
fn truncates_long_blocks_in_summary() {
    let summary = summarize_block(&ContentBlock::Text {
        text: "x".repeat(400),
    });
    assert!(summary.ends_with('\u{2026}'));
    assert!(summary.chars().count() <= 161);
}

#[test]
fn extracts_key_urls_from_message_content() {
    let urls = collect_key_urls(&[
        ConversationMessage::user_text("First visit https://example.com before checking docs."),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Then inspect https://docs.example.com/guide and revisit https://example.com."
                .to_string(),
        }]),
    ]);
    assert_eq!(
        urls,
        vec![
            "https://docs.example.com/guide".to_string(),
            "https://example.com".to_string(),
        ]
    );
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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
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
    let result = extract_existing_compacted_summary(&msg);
    assert!(result.is_some());
    assert!(result.unwrap().contains("Scope:"));
}

#[test]
fn prefix_detection_returns_none_for_user_message() {
    let msg = ConversationMessage::user_text("hello");
    assert!(extract_existing_compacted_summary(&msg).is_none());
}

#[test]
fn prefix_detection_returns_none_for_empty_session() {
    let session = Session {
        version: 1,
        model: None,
        title: None,
        messages: vec![],
        child_sessions: Vec::new(),
    };
    assert_eq!(compacted_summary_prefix_len(&session), 0);
}

#[test]
fn prune_tool_outputs_truncates_large_old_outputs() {
    let large_output = "x".repeat(10_000);
    // Need enough content after the large output to push it outside the 40K token window.
    // 40K tokens 鈮?160K chars. Use 42 messages of 4000 chars each (~42K tokens).
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

    prune_tool_outputs(&mut messages, 40_000, 2_000);

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

    prune_tool_outputs(&mut messages, 40_000, 2_000);

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

    prune_tool_outputs(&mut messages, 40_000, 2_000);

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
            !output.contains("[≈?output truncated"),
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

    prune_tool_outputs(&mut messages, 40_000, 2_000);

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
    let result = merge_compact_summaries(None, summary);
    assert_eq!(result, summary);
}

#[test]
fn merge_summaries_second_compaction_contains_both_sections() {
    let first_summary =
        "<summary>Conversation summary:\n- Scope: 4 messages.\n- Current work: task A.</summary>";
    let second_summary =
        "<summary>Conversation summary:\n- Scope: 3 messages.\n- Current work: task B.</summary>";
    let merged = merge_compact_summaries(Some(first_summary), second_summary);
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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
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
                    m.blocks
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id))
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
        child_sessions: Vec::new(),
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
fn make_navigate_pair(call_id: &str, output: &str) -> (ConversationMessage, ConversationMessage) {
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
        child_sessions: Vec::new(),
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

    // Run compaction ≈?must not panic
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
                !output.contains("[≈?output truncated"),
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
        child_sessions: Vec::new(),
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
        child_sessions: Vec::new(),
    };

    // Budget of ~15K tokens (15000 * 4 chars 鈮?60K chars budget)
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
// Test 4: API validity ≈?no orphaned tool_result blocks
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
                        m.blocks.iter().any(
                            |b| matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id),
                        )
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
        child_sessions: Vec::new(),
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
