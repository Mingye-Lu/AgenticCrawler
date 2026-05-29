use super::{
    parse_auto_compaction_threshold, ApiClient, ApiRequest, AssistantEvent, AutoCompactionEvent,
    ConversationRuntime, RuntimeError, StaticToolExecutor, ToolOutcome,
    DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD,
};
use crate::compact::CompactionConfig;
use crate::observer::RuntimeObserver;
use crate::prompt::SystemPromptBuilder;
use crate::session::{ContentBlock, MessageRole, Session};
use crate::usage::TokenUsage;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::time::{timeout, Duration};

struct ScriptedApiClient {
    call_count: usize,
}

impl ApiClient for ScriptedApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.call_count += 1;
        match self.call_count {
            1 => {
                assert!(request
                    .messages
                    .iter()
                    .any(|message| message.role == MessageRole::User));
                Ok(vec![
                    AssistantEvent::TextDelta("Let me calculate that.".to_string()),
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "add".to_string(),
                        input: "2,2".to_string(),
                    },
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 20,
                        output_tokens: 6,
                        cache_creation_input_tokens: 1,
                        cache_read_input_tokens: 2,
                    }),
                    AssistantEvent::MessageStop,
                ])
            }
            2 => {
                let last_message = request
                    .messages
                    .last()
                    .expect("tool result should be present");
                assert_eq!(last_message.role, MessageRole::Tool);
                Ok(vec![
                    AssistantEvent::TextDelta("The answer is 4.".to_string()),
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 24,
                        output_tokens: 4,
                        cache_creation_input_tokens: 1,
                        cache_read_input_tokens: 3,
                    }),
                    AssistantEvent::MessageStop,
                ])
            }
            _ => Err(RuntimeError::new("unexpected extra API call")),
        }
    }
}

struct MockApiClientWithText(String);

impl ApiClient for MockApiClientWithText {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(vec![
            AssistantEvent::TextDelta(self.0.clone()),
            AssistantEvent::MessageStop,
        ])
    }
}

struct MockApiClientError;

impl ApiClient for MockApiClientError {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Err(RuntimeError::new("simulated API error"))
    }
}

#[tokio::test]
async fn runs_user_to_tool_to_result_loop_end_to_end_and_tracks_usage() {
    let api_client = ScriptedApiClient { call_count: 0 };
    let tool_executor = StaticToolExecutor::new().register("add", |input| {
        let total = input
            .split(',')
            .map(|part| part.parse::<i32>().expect("input must be valid integer"))
            .sum::<i32>();
        Ok(ToolOutcome::reply(total.to_string()))
    });
    let system_prompt = SystemPromptBuilder::new().append_section("# Tools").build();
    let mut runtime =
        ConversationRuntime::new(Session::new(), api_client, tool_executor, system_prompt);

    let summary = runtime
        .run_turn("what is 2 + 2?")
        .await
        .expect("conversation loop should succeed");

    assert_eq!(summary.iterations, 2);
    assert_eq!(summary.assistant_messages.len(), 2);
    assert_eq!(summary.tool_results.len(), 1);
    assert_eq!(runtime.session().messages.len(), 4);
    assert_eq!(summary.usage.output_tokens, 10);
    assert_eq!(summary.auto_compaction, None);
    assert!(matches!(
        runtime.session().messages[1].blocks[1],
        ContentBlock::ToolUse { .. }
    ));
    assert!(matches!(
        runtime.session().messages[2].blocks[0],
        ContentBlock::ToolResult {
            is_error: false,
            ..
        }
    ));
}

#[test]
fn reconstructs_usage_tracker_from_restored_session() {
    struct SimpleApi;
    impl ApiClient for SimpleApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let mut session = Session::new();
    session
        .messages
        .push(crate::session::ConversationMessage::assistant_with_usage(
            vec![ContentBlock::Text {
                text: "earlier".to_string(),
            }],
            Some(TokenUsage {
                input_tokens: 11,
                output_tokens: 7,
                cache_creation_input_tokens: 2,
                cache_read_input_tokens: 1,
            }),
        ));

    let runtime = ConversationRuntime::new(
        session,
        SimpleApi,
        StaticToolExecutor::new(),
        vec!["system".to_string()],
    );

    assert_eq!(runtime.usage().turns(), 1);
    assert_eq!(runtime.usage().cumulative_usage().total_tokens(), 21);
}

#[tokio::test]
async fn compacts_session_after_turns() {
    struct SimpleApi;
    impl ApiClient for SimpleApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SimpleApi,
        StaticToolExecutor::new(),
        vec!["system".to_string()],
    );
    runtime.run_turn("a").await.expect("turn a");
    runtime.run_turn("b").await.expect("turn b");
    runtime.run_turn("c").await.expect("turn c");

    let result = runtime.compact(CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
        ..CompactionConfig::default()
    });
    assert!(result.summary.contains("Conversation summary"));
    assert_eq!(
        result.compacted_session.messages[0].role,
        MessageRole::System
    );
}

#[test]
fn try_llm_summarize_returns_some_on_success() {
    let removed = vec![
        crate::session::ConversationMessage::user_text("scrape books.toscrape.com"),
        crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Navigating to books.toscrape.com".to_string(),
        }]),
    ];
    let mut client = MockApiClientWithText(
        "## Goal\nScrape books\n## Progress\nNavigated to site\n## Key Decisions\nNone\n## Next Steps\nExtract data\n## Relevant Files\nNone"
            .to_string(),
    );

    let result = super::try_llm_summarize(&removed, &mut client, Some("test-model"));

    assert!(result.is_some(), "should return Some on success");
    let summary = result.expect("summary should exist");
    assert!(
        summary.contains("<summary>"),
        "should be wrapped in summary tags"
    );
    assert!(
        summary.contains("Goal") || summary.contains("goal"),
        "should contain structured output"
    );
}

#[test]
fn try_llm_summarize_compresses_oversized_response() {
    let removed = vec![crate::session::ConversationMessage::user_text("hello")];
    let oversized = "## Goal\n".to_string() + &"word ".repeat(1_000);
    let mut client = MockApiClientWithText(oversized);

    let result = super::try_llm_summarize(&removed, &mut client, None);

    let summary = result.expect("oversized response must be compressed, not dropped");
    assert!(summary.starts_with("<summary>"));
    assert!(summary.ends_with("</summary>"));
    let inner = summary
        .trim_start_matches("<summary>")
        .trim_end_matches("</summary>");
    assert!(
        inner.chars().count() <= 1_200,
        "compressed inner length must respect default budget; got {}",
        inner.chars().count()
    );
}

#[test]
fn try_llm_summarize_returns_none_on_api_error() {
    let removed = vec![crate::session::ConversationMessage::user_text("hello")];
    let mut client = MockApiClientError;

    let result = super::try_llm_summarize(&removed, &mut client, None);

    assert!(result.is_none(), "should return None on API error");
}

#[test]
fn try_llm_summarize_returns_none_on_empty_response() {
    let removed = vec![crate::session::ConversationMessage::user_text("hello")];
    let mut client = MockApiClientWithText(String::new());

    let result = super::try_llm_summarize(&removed, &mut client, None);

    assert!(result.is_none(), "should return None on empty response");
}

#[test]
fn try_llm_summarize_returns_none_for_empty_messages() {
    let mut client = MockApiClientWithText("some summary".to_string());

    let result = super::try_llm_summarize(&[], &mut client, None);

    assert!(
        result.is_none(),
        "should return None when no messages to summarize"
    );
}

#[test]
fn llm_summarization_disabled_uses_mechanical_path() {
    use crate::compact::compact_session;

    let text = "word ".repeat(200);
    let session = Session {
        version: 1,
        model: None,
        title: None,
        messages: vec![
            crate::session::ConversationMessage::user_text(&text),
            crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                text: text.clone(),
            }]),
            crate::session::ConversationMessage::user_text(&text),
            crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "done".to_string(),
            }]),
        ],
        child_sessions: Vec::new(),
    };
    let config = CompactionConfig {
        llm_summarization: false,
        max_estimated_tokens: 1,
        preserve_recent_messages: 2,
        ..CompactionConfig::default()
    };

    let result = compact_session(&session, config);

    assert!(result.removed_message_count > 0);
    assert!(
        result.formatted_summary.contains("Scope:"),
        "mechanical summary should contain 'Scope:'"
    );
}

#[tokio::test]
async fn auto_compacts_when_cumulative_input_threshold_is_crossed() {
    struct SimpleApi;
    impl ApiClient for SimpleApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::Usage(TokenUsage {
                    input_tokens: 120_000,
                    output_tokens: 4,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let session = Session {
        version: 1,
        model: None,
        title: None,
        messages: vec![
            crate::session::ConversationMessage::user_text("one"),
            crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "two".to_string(),
            }]),
            crate::session::ConversationMessage::user_text("three"),
            crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "four".to_string(),
            }]),
        ],
        child_sessions: Vec::new(),
    };

    let mut runtime = ConversationRuntime::new(
        session,
        SimpleApi,
        StaticToolExecutor::new(),
        vec!["system".to_string()],
    )
    .with_auto_compaction_input_tokens_threshold(100_000);

    let summary = runtime
        .run_turn("trigger")
        .await
        .expect("turn should succeed");

    assert_eq!(
        summary.auto_compaction,
        Some(AutoCompactionEvent {
            removed_message_count: 2,
        })
    );
    assert_eq!(runtime.session().messages[0].role, MessageRole::System);
}

#[tokio::test]
async fn skips_auto_compaction_below_threshold() {
    struct SimpleApi;
    impl ApiClient for SimpleApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::Usage(TokenUsage {
                    input_tokens: 99_999,
                    output_tokens: 4,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SimpleApi,
        StaticToolExecutor::new(),
        vec!["system".to_string()],
    )
    .with_auto_compaction_input_tokens_threshold(100_000);

    let summary = runtime
        .run_turn("trigger")
        .await
        .expect("turn should succeed");
    assert_eq!(summary.auto_compaction, None);
    assert_eq!(runtime.session().messages.len(), 2);
}

#[test]
fn auto_compaction_threshold_defaults_and_parses_values() {
    assert_eq!(
        parse_auto_compaction_threshold(None),
        DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD
    );
    assert_eq!(parse_auto_compaction_threshold(Some("4321")), 4321);
    assert_eq!(
        parse_auto_compaction_threshold(Some("not-a-number")),
        DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD
    );
}

#[test]
fn reasoning_event_stored_in_message() {
    use super::build_assistant_message;

    let events = vec![
        AssistantEvent::Reasoning {
            data: r#"{"id":"rs_123","content":[]}"#.to_string(),
        },
        AssistantEvent::TextDelta("answer".to_string()),
        AssistantEvent::MessageStop,
    ];
    let (message, _usage) = build_assistant_message(events).expect("build should succeed");

    assert_eq!(message.blocks.len(), 2);
    assert!(matches!(
        &message.blocks[0],
        ContentBlock::Reasoning { data } if data == r#"{"id":"rs_123","content":[]}"#
    ));
    assert!(matches!(&message.blocks[1], ContentBlock::Text { text } if text == "answer"));
}

struct SingleReplyApiClient {
    call_count: Arc<AtomicUsize>,
}

impl ApiClient for SingleReplyApiClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![
            AssistantEvent::TextDelta("done".to_string()),
            AssistantEvent::MessageStop,
        ])
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PauseObserverState {
    events: Vec<String>,
}

struct PauseRecordingObserver {
    state: Arc<Mutex<PauseObserverState>>,
}

impl PauseRecordingObserver {
    fn new(state: Arc<Mutex<PauseObserverState>>) -> Self {
        Self { state }
    }
}

impl RuntimeObserver for PauseRecordingObserver {
    fn on_pause_started(&mut self, reason: &str) {
        self.state
            .lock()
            .expect("pause observer state lock")
            .events
            .push(format!("started:{reason}"));
    }

    fn on_pause_ended(&mut self) {
        self.state
            .lock()
            .expect("pause observer state lock")
            .events
            .push("ended".to_string());
    }
}

#[tokio::test]
async fn pause_and_resume() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SingleReplyApiClient {
            call_count: Arc::clone(&call_count),
        },
        StaticToolExecutor::new(),
        vec!["system".to_string()],
    );
    let control = runtime.control_state();
    control.request_pause();

    let run_turn = runtime.run_turn("hello");
    tokio::pin!(run_turn);

    assert!(timeout(Duration::from_millis(50), &mut run_turn)
        .await
        .is_err());
    assert_eq!(call_count.load(Ordering::SeqCst), 0);

    control.resume();

    let summary = timeout(Duration::from_secs(1), &mut run_turn)
        .await
        .expect("run_turn should complete after resume")
        .expect("turn should succeed");
    assert_eq!(summary.iterations, 1);
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancel_during_pause() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SingleReplyApiClient {
            call_count: Arc::clone(&call_count),
        },
        StaticToolExecutor::new(),
        vec!["system".to_string()],
    );
    let control = runtime.control_state();
    control.request_pause();

    let run_turn = runtime.run_turn("hello");
    tokio::pin!(run_turn);

    assert!(timeout(Duration::from_millis(50), &mut run_turn)
        .await
        .is_err());
    control.request_cancel();

    let error = timeout(Duration::from_secs(1), &mut run_turn)
        .await
        .expect("run_turn should exit after cancel")
        .expect_err("turn should be interrupted");
    assert_eq!(error.to_string(), "interrupted by user");
    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert!(!control.is_paused());
    assert!(!control.is_cancelled());
}

#[tokio::test]
async fn pause_observer_events() {
    let state = Arc::new(Mutex::new(PauseObserverState::default()));
    let observer = PauseRecordingObserver::new(Arc::clone(&state));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SingleReplyApiClient {
            call_count: Arc::new(AtomicUsize::new(0)),
        },
        StaticToolExecutor::new(),
        vec!["system".to_string()],
    )
    .with_observer(Box::new(observer));
    let control = runtime.control_state();
    control.request_pause();

    let run_turn = runtime.run_turn("hello");
    tokio::pin!(run_turn);

    assert!(timeout(Duration::from_millis(50), &mut run_turn)
        .await
        .is_err());
    assert_eq!(
        state.lock().expect("pause observer state lock").events,
        vec!["started:Paused by user".to_string()]
    );

    control.resume();

    timeout(Duration::from_secs(1), &mut run_turn)
        .await
        .expect("run_turn should complete after resume")
        .expect("turn should succeed");

    assert_eq!(
        state.lock().expect("pause observer state lock").events,
        vec!["started:Paused by user".to_string(), "ended".to_string()]
    );
}

#[tokio::test]
async fn llm_triggered_pause_via_tool_executor() {
    use crate::control::ControlState;

    struct PauseToolApiClient {
        call_count: Arc<AtomicUsize>,
    }

    impl ApiClient for PauseToolApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-pause".to_string(),
                        name: "wait_for_human".to_string(),
                        input: r#"{"reason":"captcha"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                1 => Ok(vec![
                    AssistantEvent::TextDelta("Resumed.".to_string()),
                    AssistantEvent::MessageStop,
                ]),
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    let shared_control = Arc::new(ControlState::default());
    let control_for_executor = Arc::clone(&shared_control);
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = Arc::clone(&call_count);

    let tool_executor = {
        let control = Arc::clone(&control_for_executor);
        StaticToolExecutor::new().register("wait_for_human", move |_input| {
            control.request_pause();
            Ok(ToolOutcome::reply(
                "Human intervention requested: captcha".to_string(),
            ))
        })
    };

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        PauseToolApiClient {
            call_count: call_count_clone,
        },
        tool_executor,
        vec!["system".to_string()],
    )
    .with_control_state(Arc::clone(&shared_control));

    let control_for_resume = Arc::clone(&shared_control);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        control_for_resume.resume();
    });

    let summary = timeout(Duration::from_secs(2), runtime.run_turn("solve captcha"))
        .await
        .expect("should not deadlock")
        .expect("turn should succeed after resume");

    assert_eq!(summary.iterations, 2);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn hotkey_pause_mid_turn_between_iterations() {
    use crate::control::ControlState;

    struct MultiStepApiClient {
        call_count: Arc<AtomicUsize>,
    }

    impl ApiClient for MultiStepApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "noop".to_string(),
                        input: "{}".to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                1 => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    let shared_control = Arc::new(ControlState::default());
    let call_count = Arc::new(AtomicUsize::new(0));

    let control_for_pause = Arc::clone(&shared_control);
    let tool_executor = StaticToolExecutor::new().register("noop", move |_| {
        control_for_pause.request_pause();
        Ok(ToolOutcome::reply("ok".to_string()))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        MultiStepApiClient {
            call_count: Arc::clone(&call_count),
        },
        tool_executor,
        vec!["system".to_string()],
    )
    .with_control_state(Arc::clone(&shared_control));

    let control_for_resume = Arc::clone(&shared_control);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        control_for_resume.resume();
    });

    let run_turn = runtime.run_turn("do it");
    tokio::pin!(run_turn);

    assert!(
        timeout(Duration::from_millis(100), &mut run_turn)
            .await
            .is_err(),
        "should be blocked during pause"
    );
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    let summary = timeout(Duration::from_secs(2), &mut run_turn)
        .await
        .expect("should complete after resume")
        .expect("turn should succeed");

    assert_eq!(summary.iterations, 2);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn multiple_pause_resume_cycles_complete() {
    use crate::control::ControlState;

    struct ThreeStepApiClient {
        call_count: Arc<AtomicUsize>,
    }

    impl ApiClient for ThreeStepApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            match n {
                0 | 1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: format!("tool-{n}"),
                        name: "pause_tool".to_string(),
                        input: "{}".to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                2 => Ok(vec![
                    AssistantEvent::TextDelta("all done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    let shared_control = Arc::new(ControlState::default());
    let cycle_count = Arc::new(AtomicUsize::new(0));

    let cycle_count_clone = Arc::clone(&cycle_count);
    let control_for_tool = Arc::clone(&shared_control);
    let tool_executor = StaticToolExecutor::new().register("pause_tool", move |_| {
        cycle_count_clone.fetch_add(1, Ordering::SeqCst);
        control_for_tool.request_pause();
        Ok(ToolOutcome::reply("paused".to_string()))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ThreeStepApiClient {
            call_count: Arc::new(AtomicUsize::new(0)),
        },
        tool_executor,
        vec!["system".to_string()],
    )
    .with_control_state(Arc::clone(&shared_control));

    let control_for_resume = Arc::clone(&shared_control);
    tokio::spawn(async move {
        for _ in 0..2 {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if control_for_resume.is_paused() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    control_for_resume.resume();
                    break;
                }
            }
        }
    });

    let summary = timeout(Duration::from_secs(3), runtime.run_turn("multi-pause"))
        .await
        .expect("should not deadlock after multiple cycles")
        .expect("turn should succeed");

    assert_eq!(summary.iterations, 3);
    assert_eq!(cycle_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancel_during_tool_triggered_pause() {
    use crate::control::ControlState;

    struct PauseToolApiClient {
        call_count: Arc<AtomicUsize>,
    }

    impl ApiClient for PauseToolApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-pause".to_string(),
                        name: "wait_for_human".to_string(),
                        input: r#"{"reason":"captcha"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                _ => Err(RuntimeError::new("should not reach second API call")),
            }
        }
    }

    let shared_control = Arc::new(ControlState::default());
    let call_count = Arc::new(AtomicUsize::new(0));

    let control_for_tool = Arc::clone(&shared_control);
    let tool_executor = StaticToolExecutor::new().register("wait_for_human", move |_| {
        control_for_tool.request_pause();
        Ok(ToolOutcome::reply("paused".to_string()))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        PauseToolApiClient {
            call_count: Arc::clone(&call_count),
        },
        tool_executor,
        vec!["system".to_string()],
    )
    .with_control_state(Arc::clone(&shared_control));

    let control_for_cancel = Arc::clone(&shared_control);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if control_for_cancel.is_paused() {
                control_for_cancel.request_cancel();
                break;
            }
        }
    });

    let error = timeout(Duration::from_secs(2), runtime.run_turn("solve captcha"))
        .await
        .expect("should not deadlock")
        .expect_err("turn should be interrupted");

    assert_eq!(error.to_string(), "interrupted by user");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fast_resume_no_lost_wakeup() {
    use crate::control::ControlState;

    struct TwoStepApiClient {
        call_count: Arc<AtomicUsize>,
    }

    impl ApiClient for TwoStepApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "pause_tool".to_string(),
                        input: "{}".to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                1 => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    let shared_control = Arc::new(ControlState::default());
    let call_count = Arc::new(AtomicUsize::new(0));

    let control_for_tool = Arc::clone(&shared_control);
    let tool_executor = StaticToolExecutor::new().register("pause_tool", move |_| {
        control_for_tool.request_pause();
        control_for_tool.resume();
        Ok(ToolOutcome::reply("instant-resumed".to_string()))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        TwoStepApiClient {
            call_count: Arc::clone(&call_count),
        },
        tool_executor,
        vec!["system".to_string()],
    )
    .with_control_state(Arc::clone(&shared_control));

    let summary = timeout(Duration::from_secs(2), runtime.run_turn("fast resume"))
        .await
        .expect("must not hang due to lost wakeup")
        .expect("turn should succeed");

    assert_eq!(summary.iterations, 2);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}
