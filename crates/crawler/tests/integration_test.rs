use crawler::{CrawlerAgent, ToolEffect, ToolRegistry};
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
        "page_map",
        Box::new(|_input| {
            Ok(ToolEffect::reply_json(&json!({
                "sections": [
                    {
                        "level": 1,
                        "text": "Example Domain",
                        "id": null,
                        "selector": "h1:nth-of-type(1)",
                        "char_count": 200,
                        "preview": "This domain is for use in..."
                    }
                ]
            })))
        }),
    );
    registry.register(
        "read_content",
        Box::new(|_input| {
            Ok(ToolEffect::reply_json(&json!({
                "content": "This domain is for use in illustrative examples.",
                "found": true,
                "total_chars": 200,
                "offset": 0,
                "has_more": false,
                "truncated": false,
                "matches_count": 1
            })))
        }),
    );
    registry
}

#[tokio::test]
async fn crawler_agent_execute_dispatches_registered_tool() {
    let mut agent = CrawlerAgent::new_lazy(mock_registry());

    let output = agent
        .execute("page_map", r"{}")
        .await
        .expect("page_map should execute successfully");

    let parsed: Value = serde_json::from_str(&output).expect("tool output should be valid JSON");
    assert!(
        parsed["sections"].is_array(),
        "page_map should return sections array"
    );
    assert_eq!(parsed["sections"][0]["text"], "Example Domain");
}

#[tokio::test]
async fn crawler_agent_run_completes_full_pipeline_with_mock_llm() {
    let agent = CrawlerAgent::new_lazy(mock_registry()).with_max_steps(5);
    let api_client = MockApiClient { call_count: 0 };

    let result = agent
        .run("Extract title from example.com", api_client)
        .await
        .expect("agent run should succeed");

    assert_eq!(result.steps_executed, 1);
    assert_eq!(result.summary, "Pipeline completed");
}
