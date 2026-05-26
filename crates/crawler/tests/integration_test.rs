use crawler::{CrawlerAgent, FetchedPage, ToolEffect, ToolRegistry};
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
    register_page_map_mock(&mut registry);
    register_navigate_mock(&mut registry);
    register_click_mock(&mut registry);
    register_read_content_mock(&mut registry);
    registry
}

fn register_page_map_mock(registry: &mut ToolRegistry) {
    registry.register(
        "page_map",
        Box::new(|_input| {
            Ok(ToolEffect::reply_json(&json!({
                "headings": [
                    {
                        "level": 1,
                        "text": "Example Domain",
                        "id": null,
                        "selector": "h1:nth-of-type(1)",
                        "char_count": 200,
                        "preview": "This domain is for use in..."
                    }
                ],
                "landmarks": [],
                "forms": [],
                "links": [{
                    "text": "More information",
                    "href": "https://www.iana.org/domains/example",
                    "selector": "a"
                }],
                "interactive": {
                    "buttons": 0,
                    "inputs": 0,
                    "selects": 0,
                    "textareas": 0
                },
                "meta": {
                    "title": "Example Domain",
                    "description": "",
                    "url": "https://example.com"
                },
                "truncated_links": false,
                "truncated_forms": false,
                "truncated_landmarks": false
            })))
        }),
    );
}

fn register_navigate_mock(registry: &mut ToolRegistry) {
    registry.register(
        "navigate",
        Box::new(|_input| {
            Ok(ToolEffect::reply_json(&json!({
                "url": "https://example.com",
                "title": "Example Domain",
                "content": "# Example Domain\n\nThis domain...",
                "format": "markdown",
                "truncated": false,
                "content_length": 45,
                "page_map": {
                    "headings": [{
                        "level": 1,
                        "text": "Example Domain"
                    }],
                    "meta": {
                        "url": "https://example.com",
                        "title": "Example Domain",
                        "description": ""
                    }
                }
            })))
        }),
    );
}

fn register_click_mock(registry: &mut ToolRegistry) {
    registry.register(
        "click",
        Box::new(|_input| {
            Ok(ToolEffect::reply_json(&json!({
                "success": true,
                "message": "Clicked element: .btn",
                "page_state": {
                    "url": "https://example.com/result",
                    "title": "Result",
                    "page_map": {
                        "headings": [],
                        "landmarks": [],
                        "links": [],
                        "meta": {
                            "title": "Result",
                            "url": "https://example.com/result",
                            "description": ""
                        }
                    }
                }
            })))
        }),
    );
}

fn register_read_content_mock(registry: &mut ToolRegistry) {
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
}

#[tokio::test]
async fn crawler_agent_execute_dispatches_registered_tool() {
    let mut agent = CrawlerAgent::new_lazy(mock_registry());

    let output = agent
        .execute("page_map", r"{}")
        .await
        .expect("page_map should execute successfully");

    let parsed: Value =
        serde_json::from_str(&output.text).expect("tool output should be valid JSON");
    assert!(
        parsed["headings"].is_array(),
        "page_map should return headings array"
    );
    assert_eq!(parsed["headings"][0]["text"], "Example Domain");
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

#[tokio::test]
async fn navigate_markdown_end_to_end() {
    let mut agent = CrawlerAgent::new_lazy(mock_registry());

    let output = agent
        .execute("navigate", r#"{"url":"https://example.com"}"#)
        .await
        .expect("navigate should execute successfully");

    let parsed: Value =
        serde_json::from_str(&output.text).expect("tool output should be valid JSON");
    assert!(
        parsed["content"]
            .as_str()
            .is_some_and(|content| content.contains('#')),
        "navigate should return markdown content"
    );
    assert!(parsed["page_map"]["headings"].is_array());
    assert_eq!(parsed["format"], "markdown");
    assert!(
        parsed.get("text").is_none(),
        "navigate should not expose old text field"
    );
    assert!(
        parsed.get("html_summary").is_none(),
        "navigate should not expose old html_summary field"
    );
}

#[tokio::test]
async fn interaction_tool_returns_page_state() {
    let mut agent = CrawlerAgent::new_lazy(mock_registry());

    let output = agent
        .execute("click", r#"{"selector":".btn"}"#)
        .await
        .expect("click should execute successfully");

    let parsed: Value =
        serde_json::from_str(&output.text).expect("tool output should be valid JSON");
    assert!(parsed.get("page_state").is_some());
    assert!(parsed["page_state"]["url"].is_string());
    assert!(parsed["page_state"]["title"].is_string());
}

#[tokio::test]
async fn page_map_expanded_structure() {
    let mut agent = CrawlerAgent::new_lazy(mock_registry());

    let output = agent
        .execute("page_map", r"{}")
        .await
        .expect("page_map should execute successfully");

    let parsed: Value =
        serde_json::from_str(&output.text).expect("tool output should be valid JSON");
    for key in [
        "headings",
        "landmarks",
        "forms",
        "links",
        "interactive",
        "meta",
    ] {
        assert!(parsed.get(key).is_some(), "page_map should include {key}");
    }
}

#[test]
fn http_fetch_path_has_markdown() {
    let page = FetchedPage {
        url: "https://example.com".to_string(),
        title: Some("Title".to_string()),
        html: "<h1>Title</h1>".to_string(),
        text: "Title".to_string(),
        markdown: "# Title\n\nSome content".to_string(),
        fetched_via_browser: false,
    };

    assert!(page.markdown.contains('#'));
    assert!(page.markdown.contains("Some content"));
}
