use super::*;
use api::{MessageResponse, OutputContentBlock, Usage};
use runtime::{AssistantEvent, ContentBlock, ConversationMessage, MessageRole};
use serde_json::json;
use std::fs;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn with_clean_config_env<T>(f: impl FnOnce() -> T) -> T {
    let _guard = test_env_lock();
    let saved_config_home = std::env::var_os("ACRAWL_CONFIG_HOME");
    let temp_dir = std::env::temp_dir().join(format!(
        "app-tests-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).expect("create temp config home");
    std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
    let result = f();
    match saved_config_home {
        Some(value) => std::env::set_var("ACRAWL_CONFIG_HOME", value),
        None => std::env::remove_var("ACRAWL_CONFIG_HOME"),
    }
    fs::remove_dir_all(temp_dir).expect("cleanup temp config home");
    result
}

#[test]
fn resolves_known_models_by_id() {
    let registry =
        api::provider::ProviderRegistry::from_credentials(&api::CredentialStore::default());
    assert!(registry.resolve_model("claude-opus-4-6").is_some());
    assert!(registry.resolve_model("claude-sonnet-4-6").is_some());
    assert!(registry
        .resolve_model("claude-haiku-4-5-20251213")
        .is_some());
    assert!(registry.resolve_model("not-a-real-model").is_none());
}

#[test]
fn provider_for_model_requires_provider_prefix() {
    let registry =
        api::provider::ProviderRegistry::from_credentials(&api::CredentialStore::default());
    assert!(registry.provider_for_model("claude-sonnet-4-6").is_err());
}

#[test]
fn provider_for_model_accepts_prefixed_model() {
    let registry =
        api::provider::ProviderRegistry::from_credentials(&api::CredentialStore::default());
    assert_eq!(
        registry
            .provider_for_model("anthropic/claude-sonnet-4-6")
            .unwrap(),
        "anthropic"
    );
}

#[test]
fn converts_tool_roundtrip_messages() {
    let messages = vec![
        ConversationMessage::user_text("hello"),
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            input: "{\"command\":\"pwd\"}".to_string(),
        }]),
        ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                tool_name: "bash".to_string(),
                output: "ok".to_string(),
                is_error: false,
            }],
            usage: None,
        },
    ];
    let converted = convert_messages(&messages);
    assert_eq!(converted.len(), 3);
    assert_eq!(converted[1].role, "assistant");
    assert_eq!(converted[2].role, "user");
}

#[test]
fn tool_result_image_block_produces_image_and_text_content() {
    use api::InputContentBlock;
    let messages = vec![ConversationMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResultImage {
            tool_use_id: "tool-1".to_string(),
            tool_name: "screenshot".to_string(),
            media_type: "image/png".to_string(),
            base64_data: "iVBORw0KGgo=".to_string(),
            caption: "screenshot: 42 bytes".to_string(),
            is_error: false,
        }],
        usage: None,
    }];
    let converted = convert_messages(&messages);
    assert_eq!(converted.len(), 1);
    let content = &converted[0].content;
    assert_eq!(content.len(), 1);
    match &content[0] {
        InputContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            assert_eq!(tool_use_id, "tool-1");
            assert!(!is_error);
            assert_eq!(content.len(), 2);
            match &content[0] {
                api::ToolResultContentBlock::Image { source } => {
                    assert_eq!(source.source_type, "base64");
                    assert_eq!(source.media_type, "image/png");
                    assert_eq!(source.data, "iVBORw0KGgo=");
                }
                other => panic!("expected Image block, got {other:?}"),
            }
            match &content[1] {
                api::ToolResultContentBlock::Text { text } => {
                    assert_eq!(text, "screenshot: 42 bytes");
                }
                other => panic!("expected Text block, got {other:?}"),
            }
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn tool_result_with_arbitrary_output_produces_single_text_block() {
    use api::InputContentBlock;
    let messages = vec![ConversationMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: "tool-1".to_string(),
            tool_name: "screenshot".to_string(),
            output: r#"{"screenshot_base64":"iVBORw0KGgo=","size_bytes":42}"#.to_string(),
            is_error: false,
        }],
        usage: None,
    }];
    let converted = convert_messages(&messages);
    assert_eq!(converted.len(), 1);
    let content = &converted[0].content;
    assert_eq!(content.len(), 1);
    match &content[0] {
        InputContentBlock::ToolResult { content, .. } => {
            assert_eq!(content.len(), 1);
            assert!(matches!(
                &content[0],
                api::ToolResultContentBlock::Text { .. }
            ));
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn non_screenshot_tool_result_stays_as_text() {
    use api::InputContentBlock;
    let messages = vec![ConversationMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: "tool-1".to_string(),
            tool_name: "navigate".to_string(),
            output: r#"{"url":"https://example.com","status":200}"#.to_string(),
            is_error: false,
        }],
        usage: None,
    }];
    let converted = convert_messages(&messages);
    let content = &converted[0].content;
    assert_eq!(content.len(), 1);
    match &content[0] {
        InputContentBlock::ToolResult { content, .. } => {
            assert_eq!(content.len(), 1);
            assert!(matches!(
                &content[0],
                api::ToolResultContentBlock::Text { .. }
            ));
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn reasoning_block_converts_to_input_content_block() {
    use api::InputContentBlock;

    let messages = vec![ConversationMessage::assistant(vec![
        ContentBlock::Reasoning {
            data: r#"{"id":"rs_xyz","content":[]}"#.to_string(),
        },
        ContentBlock::Text {
            text: "done".to_string(),
        },
    ])];
    let converted = convert_messages(&messages);
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].content.len(), 2);
    match &converted[0].content[0] {
        InputContentBlock::Reasoning { data } => {
            assert_eq!(data["id"], "rs_xyz");
        }
        other => panic!("expected Reasoning, got {other:?}"),
    }
    assert!(matches!(
        &converted[0].content[1],
        InputContentBlock::Text { text } if text == "done"
    ));
}

#[test]
fn push_output_block_emits_text_delta() {
    let mut events = Vec::new();
    let mut pending_tool = None;
    push_output_block(
        OutputContentBlock::Text {
            text: "# Heading".to_string(),
        },
        &mut events,
        &mut pending_tool,
        false,
    );
    assert!(matches!(
        &events[0],
        AssistantEvent::TextDelta(text) if text == "# Heading"
    ));
}

#[test]
fn push_output_block_skips_empty_object_prefix_for_tool_streams() {
    let mut events = Vec::new();
    let mut pending_tool = None;
    push_output_block(
        OutputContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "read_file".to_string(),
            input: json!({}),
        },
        &mut events,
        &mut pending_tool,
        true,
    );
    assert!(events.is_empty());
    assert_eq!(
        pending_tool,
        Some(("tool-1".to_string(), "read_file".to_string(), String::new()))
    );
}

#[test]
fn response_to_events_preserves_empty_object_json_input_outside_streaming() {
    let events = response_to_events(MessageResponse {
        id: "msg-1".to_string(),
        kind: "message".to_string(),
        model: "claude-opus-4-6".to_string(),
        role: "assistant".to_string(),
        content: vec![OutputContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "read_file".to_string(),
            input: json!({}),
        }],
        stop_reason: Some("tool_use".to_string()),
        stop_sequence: None,
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
        request_id: None,
    });
    assert!(
        matches!(&events[0], AssistantEvent::ToolUse { name, input, .. } if name == "read_file" && input == "{}")
    );
}

#[test]
fn response_to_events_preserves_non_empty_json_input_outside_streaming() {
    let events = response_to_events(MessageResponse {
        id: "msg-2".to_string(),
        kind: "message".to_string(),
        model: "claude-opus-4-6".to_string(),
        role: "assistant".to_string(),
        content: vec![OutputContentBlock::ToolUse {
            id: "tool-2".to_string(),
            name: "read_file".to_string(),
            input: json!({ "path": "rust/Cargo.toml" }),
        }],
        stop_reason: Some("tool_use".to_string()),
        stop_sequence: None,
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
        request_id: None,
    });
    assert!(
        matches!(&events[0], AssistantEvent::ToolUse { name, input, .. } if name == "read_file" && input == "{\"path\":\"rust/Cargo.toml\"}")
    );
}

#[test]
fn cancellable_callback_stops_on_cancel() {
    let (cancel_tx, cancel_rx) = mpsc::channel();
    cancel_tx.send(()).expect("send cancel signal");

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let handle =
        std::thread::spawn(move || wait_for_oauth_callback_cancellable(listener, cancel_rx));

    let result = handle.join().expect("thread should not panic");
    let err = result.expect_err("should return error on cancel");
    let msg = err.to_string();
    assert!(
        msg.contains("cancelled") || msg.contains("Interrupted"),
        "expected cancellation error, got: {msg}"
    );
}

#[test]
fn cancellable_callback_returns_on_cancel_while_listening() {
    let (cancel_tx, cancel_rx) = mpsc::channel();

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let handle =
        std::thread::spawn(move || wait_for_oauth_callback_cancellable(listener, cancel_rx));

    std::thread::sleep(std::time::Duration::from_millis(250));
    cancel_tx.send(()).expect("send cancel signal");

    let result = handle.join().expect("thread should not panic");
    let err = result.expect_err("should return error on cancel");
    let msg = err.to_string();
    assert!(
        msg.contains("cancelled") || msg.contains("Interrupted"),
        "expected cancellation error, got: {msg}"
    );
}

#[test]
fn model_supports_reasoning_for_catalog_models() {
    assert!(model_supports_reasoning("o3"));
    assert!(model_supports_reasoning("o4-mini"));
    assert!(model_supports_reasoning("codex-mini-latest"));
}

#[test]
fn model_supports_reasoning_false_for_non_reasoning_models() {
    assert!(!model_supports_reasoning("gpt-4o"));
    assert!(!model_supports_reasoning("claude-sonnet-4-6"));
}

#[test]
fn model_supports_reasoning_catalog_known_models_are_deterministic() {
    assert!(model_supports_reasoning("o3"));
    assert!(model_supports_reasoning("o4-mini"));
    assert!(!model_supports_reasoning("gpt-4o"));
    assert!(!model_supports_reasoning("claude-sonnet-4-6"));
}

#[test]
fn model_supports_reasoning_matches_catalog_capabilities() {
    with_clean_config_env(|| {
        assert!(model_supports_reasoning("o3"));
        assert!(!model_supports_reasoning("claude-sonnet-4-6"));
    });
}

#[test]
fn model_reasoning_efforts_returns_expected_efforts_for_reasoning_models() {
    with_clean_config_env(|| {
        assert_eq!(
            model_reasoning_efforts("o4-mini"),
            api::ReasoningEffort::OPENAI.to_vec()
        );
        assert!(model_reasoning_efforts("gpt-4o").is_empty());
    });
}

#[test]
fn filter_tool_specs_without_allowlist_returns_all_specs() {
    let filtered = filter_tool_specs(None);
    let all = mvp_tool_specs();

    assert_eq!(filtered.len(), all.len());
    assert_eq!(
        filtered.iter().map(|spec| spec.name).collect::<Vec<_>>(),
        all.iter().map(|spec| spec.name).collect::<Vec<_>>()
    );
}

#[test]
fn filter_tool_specs_with_specific_names_returns_only_matching_specs() {
    let allowed = ["navigate", "read_content"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let filtered = filter_tool_specs(Some(&allowed));

    assert_eq!(
        filtered.iter().map(|spec| spec.name).collect::<Vec<_>>(),
        vec!["navigate", "read_content"]
    );
}
