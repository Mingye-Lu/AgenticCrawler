use crawler::{CrawlerAgent, ToolRegistry};
use runtime::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolExecutor};
use serde_json::{json, Value};

struct MockApiClient {
    call_count: usize,
}

impl ApiClient for MockApiClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.call_count += 1;
        match self.call_count {
            1 => Ok(vec![
                AssistantEvent::ToolUse {
                    id: "call-1".to_string(),
                    name: "extract_data".to_string(),
                    input:
                        r#"{"instruction":"extract title","data":{"url":"https://example.com"}}"#
                            .to_string(),
                },
                AssistantEvent::MessageStop,
            ]),
            2 => Ok(vec![
                AssistantEvent::TextDelta("Pipeline completed".to_string()),
                AssistantEvent::MessageStop,
            ]),
            _ => Err(RuntimeError::new("unexpected extra API call")),
        }
    }
}

fn mock_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(
        "extract_data",
        Box::new(|input| {
            let instruction = input
                .get("instruction")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let source_url = input
                .get("data")
                .and_then(|value| value.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default();

            Ok(json!({
                "instruction": instruction,
                "url": source_url,
                "title": "Example Domain"
            }))
        }),
    );
    registry
}

#[tokio::test]
async fn crawler_agent_execute_dispatches_registered_tool() {
    let mut agent = CrawlerAgent::new_lazy(mock_registry());

    let output = agent
        .execute(
            "extract_data",
            r#"{"instruction":"extract title","data":{"url":"https://example.com"}}"#,
        )
        .await
        .expect("extract_data should execute successfully");

    let parsed: Value = serde_json::from_str(&output).expect("tool output should be valid JSON");
    assert_eq!(parsed["title"], "Example Domain");
    assert_eq!(parsed["url"], "https://example.com");
}

#[tokio::test]
async fn crawler_agent_run_completes_full_pipeline_with_mock_llm() {
    let agent = CrawlerAgent::new_lazy(mock_registry()).with_max_steps(5);
    let api_client = MockApiClient { call_count: 0 };

    let result = agent
        .run("Extract title from example.com", api_client)
        .await
        .expect("agent run should succeed");

    assert_eq!(result.steps_executed, 2);
    assert_eq!(result.summary, "Pipeline completed");
    assert_eq!(result.extracted_data.len(), 1);
    assert_eq!(result.extracted_data[0]["title"], "Example Domain");
    assert_eq!(result.extracted_data[0]["url"], "https://example.com");
}
