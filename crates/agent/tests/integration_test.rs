use acrawl_core::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolEffect, ToolExecutor};
use agent::{CrawlerAgent, ToolRegistry};
use browser::FetchedPage;
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

#[cfg(test)]
mod set_device_integration {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::Mutex;

    use agent::tools::set_device::resolve_device;
    use agent::{
        BridgeError, BrowserBackend, BrowserContext, BrowserState, CrawlState, PageInfo,
        ScreenshotOptions, SharedBridge, ToolEffect,
    };

    /// Minimal no-op backend — tests return before calling the bridge.
    #[derive(Debug)]
    struct NopBackend;

    #[async_trait]
    impl BrowserBackend for NopBackend {
        async fn navigate(&mut self, _url: &str) -> Result<PageInfo, BridgeError> {
            unreachable!()
        }
        async fn new_page(&mut self, _url: Option<&str>) -> Result<usize, BridgeError> {
            unreachable!()
        }
        async fn close_page(&mut self, _page_index: usize) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn scroll(&mut self, _direction: &str, _pixels: i64) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn page_map(
            &mut self,
            _scope: Option<&str>,
            _compound_enrichment: bool,
        ) -> Result<serde_json::Value, BridgeError> {
            unreachable!()
        }
        async fn read_content(
            &mut self,
            _heading: Option<&str>,
            _selector: Option<&str>,
            _offset: usize,
            _max_chars: usize,
        ) -> Result<serde_json::Value, BridgeError> {
            unreachable!()
        }
        async fn wait_for_selector(
            &mut self,
            _selector: &str,
            _timeout_ms: u64,
            _state: Option<&str>,
        ) -> Result<bool, BridgeError> {
            unreachable!()
        }
        async fn select_option(
            &mut self,
            _selector: &str,
            _value: &str,
        ) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn evaluate(&mut self, _script: &str) -> Result<serde_json::Value, BridgeError> {
            unreachable!()
        }
        async fn hover(&mut self, _selector: &str) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn press_key(
            &mut self,
            _key: &str,
            _selector: Option<&str>,
        ) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn switch_tab(&mut self, _index: i64) -> Result<serde_json::Value, BridgeError> {
            unreachable!()
        }
        async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
            unreachable!()
        }
        async fn import_cookies(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn import_cookies_only(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn import_local_storage(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn list_resources(&mut self) -> Result<serde_json::Value, BridgeError> {
            unreachable!()
        }
        async fn save_file(&mut self, _url: &str, _path: &str) -> Result<String, BridgeError> {
            unreachable!()
        }
        async fn click(&mut self, _selector: &str) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn click_at(&mut self, _x: f64, _y: f64) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn fill(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn screenshot(
            &mut self,
            _options: &ScreenshotOptions<'_>,
        ) -> Result<(String, usize), BridgeError> {
            unreachable!()
        }
        async fn go_back(&mut self) -> Result<String, BridgeError> {
            unreachable!()
        }
        async fn set_device(
            &mut self,
            _options: &serde_json::Value,
        ) -> Result<serde_json::Value, BridgeError> {
            unreachable!()
        }
    }

    fn make_browser() -> BrowserContext {
        let backend: Box<dyn BrowserBackend + Send> = Box::new(NopBackend);
        let bridge: SharedBridge = Arc::new(Mutex::new(backend));
        BrowserContext::new(bridge)
    }

    #[test]
    fn resolve_iphone_15_returns_correct_viewport() {
        let preset = resolve_device("iphone_15").unwrap();
        let options = preset.to_json();
        assert_eq!(options["viewport"]["width"], 393);
        assert_eq!(options["viewport"]["height"], 659);
        assert_eq!(options["isMobile"], true);
        assert_eq!(options["hasTouch"], true);
    }

    #[test]
    fn resolve_desktop_returns_desktop_defaults() {
        let preset = resolve_device("desktop").unwrap();
        let options = preset.to_json();
        assert_eq!(options["viewport"]["width"], 1920);
        assert_eq!(options["viewport"]["height"], 1080);
        assert_eq!(options["isMobile"], false);
        assert_eq!(options["hasTouch"], false);
    }

    #[test]
    fn unknown_device_preset_resolve_fails() {
        assert!(resolve_device("nonexistent").is_none());
    }

    #[test]
    fn crawl_state_current_device_initializes_to_none() {
        let state = CrawlState::default();
        assert!(state.current_device.is_none());
    }

    #[test]
    fn crawl_state_has_active_subagents_initializes_to_false() {
        let state = CrawlState::default();
        assert!(!state.has_active_subagents);
    }

    #[tokio::test]
    async fn set_device_handler_blocks_when_subagents_active() {
        use agent::tools::set_device::execute;

        let mut state = CrawlState {
            has_active_subagents: true,
            ..CrawlState::default()
        };

        let mut browser = make_browser();

        let result = execute(&json!({"device": "iphone_15"}), &mut browser, &mut state).await;

        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("sub-agent") || err_str.contains("sub-agents"),
            "Expected sub-agent error, got: {err_str}"
        );
    }

    #[tokio::test]
    async fn set_device_no_op_when_already_in_mode() {
        use agent::tools::set_device::execute;

        let mut state = CrawlState {
            current_device: Some("iphone_15".to_string()),
            has_active_subagents: false,
            ..CrawlState::default()
        };

        let mut browser = make_browser();

        let result = execute(&json!({"device": "iphone_15"}), &mut browser, &mut state).await;

        assert!(result.is_ok(), "Expected Ok, got: {result:?}");
        match result.unwrap() {
            ToolEffect::Reply(text) => {
                assert!(
                    text.contains("Already") || text.contains("already"),
                    "Expected 'already in mode' message, got: {text}"
                );
            }
            other => panic!("Expected ToolEffect::Reply, got: {other:?}"),
        }
    }
}
