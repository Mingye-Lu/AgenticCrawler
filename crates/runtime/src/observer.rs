use crate::usage::TokenUsage;

/// Observer that receives events from `ConversationRuntime`.
/// All methods have default no-op implementations.
pub trait RuntimeObserver: Send {
    fn on_text_delta(&mut self, text: &str) {
        let _ = text;
    }

    fn on_tool_call_start(&mut self, id: &str, name: &str, input: &str) {
        let _ = (id, name, input);
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        let _ = (name, output, is_error);
    }

    fn on_system_message(&mut self, msg: &str) {
        let _ = msg;
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        let _ = result;
    }

    fn on_usage(&mut self, usage: &TokenUsage) {
        let _ = usage;
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeObserver;
    use crate::conversation::{
        ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError,
        StaticToolExecutor,
    };
    use crate::session::Session;
    use crate::usage::TokenUsage;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default, PartialEq, Eq)]
    struct ObserverState {
        text_deltas: Vec<String>,
        tool_calls: Vec<(String, String, String)>,
        tool_results: Vec<(String, String, bool)>,
        turn_finished: Vec<Result<(), String>>,
        usages: Vec<TokenUsage>,
    }

    struct RecordingObserver {
        state: Arc<Mutex<ObserverState>>,
    }

    impl RecordingObserver {
        fn new(state: Arc<Mutex<ObserverState>>) -> Self {
            Self { state }
        }
    }

    impl RuntimeObserver for RecordingObserver {
        fn on_text_delta(&mut self, text: &str) {
            self.state
                .lock()
                .expect("observer state lock")
                .text_deltas
                .push(text.to_string());
        }

        fn on_tool_call_start(&mut self, id: &str, name: &str, input: &str) {
            self.state
                .lock()
                .expect("observer state lock")
                .tool_calls
                .push((id.to_string(), name.to_string(), input.to_string()));
        }

        fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
            self.state
                .lock()
                .expect("observer state lock")
                .tool_results
                .push((name.to_string(), output.to_string(), is_error));
        }

        fn on_turn_finished(&mut self, result: &Result<(), String>) {
            self.state
                .lock()
                .expect("observer state lock")
                .turn_finished
                .push(result.clone());
        }

        fn on_usage(&mut self, usage: &TokenUsage) {
            self.state
                .lock()
                .expect("observer state lock")
                .usages
                .push(*usage);
        }
    }

    struct NoOpObserver;

    impl RuntimeObserver for NoOpObserver {}

    #[test]
    fn test_no_op_observer_compiles() {
        let mut observer = NoOpObserver;
        observer.on_text_delta("hello");
        observer.on_tool_call_start("tool-1", "add", "2,2");
        observer.on_tool_result("add", "4", false);
        observer.on_system_message("system");
        observer.on_turn_finished(&Ok(()));
        observer.on_usage(&TokenUsage::default());
    }

    struct TextDeltaApiClient;

    impl ApiClient for TextDeltaApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("hello observer".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    #[tokio::test]
    async fn test_observer_receives_text_delta() {
        let state = Arc::new(Mutex::new(ObserverState::default()));
        let observer = RecordingObserver::new(Arc::clone(&state));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            TextDeltaApiClient,
            StaticToolExecutor::new(),
            vec!["system".to_string()],
        )
        .with_observer(Box::new(observer));

        runtime.run_turn("say hi").await.expect("turn succeeds");

        let state = state.lock().expect("observer state lock");
        assert_eq!(state.text_deltas, vec!["hello observer"]);
    }

    struct ToolCallApiClient {
        calls: usize,
    }

    impl ApiClient for ToolCallApiClient {
        fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "echo".to_string(),
                        input: "payload".to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                2 => {
                    assert!(request
                        .messages
                        .iter()
                        .any(|message| message.role == crate::session::MessageRole::Tool));
                    Ok(vec![
                        AssistantEvent::TextDelta("done".to_string()),
                        AssistantEvent::MessageStop,
                    ])
                }
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    #[tokio::test]
    async fn test_observer_receives_tool_call() {
        let state = Arc::new(Mutex::new(ObserverState::default()));
        let observer = RecordingObserver::new(Arc::clone(&state));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ToolCallApiClient { calls: 0 },
            StaticToolExecutor::new().register("echo", |input| Ok(format!("echo:{input}"))),
            vec!["system".to_string()],
        )
        .with_observer(Box::new(observer));

        runtime.run_turn("use tool").await.expect("turn succeeds");

        let state = state.lock().expect("observer state lock");
        assert_eq!(
            state.tool_calls,
            vec![(
                "tool-1".to_string(),
                "echo".to_string(),
                "payload".to_string(),
            )]
        );
        assert_eq!(
            state.tool_results,
            vec![("echo".to_string(), "echo:payload".to_string(), false)]
        );
    }

    struct FinishedApiClient;

    impl ApiClient for FinishedApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("finished".to_string()),
                AssistantEvent::Usage(TokenUsage {
                    input_tokens: 3,
                    output_tokens: 2,
                    cache_creation_input_tokens: 1,
                    cache_read_input_tokens: 0,
                }),
                AssistantEvent::MessageStop,
            ])
        }
    }

    #[tokio::test]
    async fn test_observer_turn_finished_called() {
        let state = Arc::new(Mutex::new(ObserverState::default()));
        let observer = RecordingObserver::new(Arc::clone(&state));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            FinishedApiClient,
            StaticToolExecutor::new(),
            vec!["system".to_string()],
        )
        .with_observer(Box::new(observer));

        runtime.run_turn("finish").await.expect("turn succeeds");

        let state = state.lock().expect("observer state lock");
        assert_eq!(state.turn_finished, vec![Ok(())]);
        assert_eq!(
            state.usages,
            vec![TokenUsage {
                input_tokens: 3,
                output_tokens: 2,
                cache_creation_input_tokens: 1,
                cache_read_input_tokens: 0,
            }]
        );
    }

    struct ErrorApiClient;

    impl ApiClient for ErrorApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Err(RuntimeError::new("boom"))
        }
    }

    #[tokio::test]
    async fn test_observer_turn_finished_called_on_error() {
        let state = Arc::new(Mutex::new(ObserverState::default()));
        let observer = RecordingObserver::new(Arc::clone(&state));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ErrorApiClient,
            StaticToolExecutor::new(),
            vec!["system".to_string()],
        )
        .with_observer(Box::new(observer));

        let error = runtime
            .run_turn("fail")
            .await
            .expect_err("turn should fail");
        assert_eq!(error.to_string(), "boom");

        let state = state.lock().expect("observer state lock");
        assert_eq!(state.turn_finished, vec![Err("boom".to_string())]);
    }
}
