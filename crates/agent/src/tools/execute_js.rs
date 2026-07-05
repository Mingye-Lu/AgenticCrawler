use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub struct ExecuteJsInput {
    pub script: String,
    pub hover_selector: Option<String>,
}

pub fn parse_input(input: &Value) -> Result<ExecuteJsInput, CrawlError> {
    let script = input
        .get("script")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("execute_js requires 'script' field"))?;

    let hover_selector = input
        .get("hover_selector")
        .and_then(|v| v.as_str())
        .map(String::from);

    if let Some(ref s) = hover_selector {
        if s.is_empty() {
            return Err(CrawlError::new("hover_selector must not be empty"));
        }
    }

    Ok(ExecuteJsInput {
        script,
        hover_selector,
    })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_input(input)?;

    if let Some(hover_selector) = &params.hover_selector {
        let resolved = super::ref_resolve::resolve_selector(hover_selector, browser.ref_map())
            .map_err(ToolExecutionError::new)?;

        browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .hover(&resolved)
            .await
            .map_err(|e| {
                ToolExecutionError::new(format!(
                    "execute_js: failed to hover over '{hover_selector}' before evaluating script: {e}"
                ))
            })?;
    }

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&params.script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let value = result.get("value").cloned().unwrap_or(Value::Null);

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": true,
        "result": value
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_script() {
        let input = json!({"script": "document.title"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.script, "document.title");
        assert!(parsed.hover_selector.is_none());
    }

    #[test]
    fn parses_hover_selector() {
        let input = json!({"script": "getComputedStyle(document.querySelector('.btn')).color", "hover_selector": ".btn"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(
            parsed.script,
            "getComputedStyle(document.querySelector('.btn')).color"
        );
        assert_eq!(parsed.hover_selector.as_deref(), Some(".btn"));
    }

    #[test]
    fn fails_without_script() {
        let input = json!({});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn fails_with_non_string_script() {
        let input = json!({"script": 42});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn fails_with_empty_hover_selector() {
        let input = json!({"script": "1", "hover_selector": ""});
        assert!(parse_input(&input).is_err());
    }

    mod execute_tests {
        use std::sync::{Arc, Mutex};

        use async_trait::async_trait;
        use browser::{
            BridgeError, BrowserBackend, BrowserState, ObservationEvent, PageInfo,
            ScreenshotOptions, SharedBridge, StorageEntry, StorageType,
        };
        use tokio::sync::Mutex as AsyncMutex;

        use super::*;

        #[derive(Debug, Default)]
        struct MockState {
            calls: Vec<String>,
            hover_should_fail: bool,
        }

        #[derive(Debug, Clone)]
        struct MockBackend {
            state: Arc<Mutex<MockState>>,
        }

        #[async_trait]
        impl BrowserBackend for MockBackend {
            async fn navigate(&mut self, _url: &str) -> Result<PageInfo, BridgeError> {
                Ok(PageInfo {
                    title: "Test".to_string(),
                    html: String::new(),
                })
            }
            async fn new_page(&mut self, _url: Option<&str>) -> Result<usize, BridgeError> {
                Ok(0)
            }
            async fn close_page(&mut self, _page_index: usize) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn scroll(&mut self, _direction: &str, _pixels: i64) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn page_map(
                &mut self,
                _scope: Option<&str>,
                _compound_enrichment: bool,
                _depth: Option<usize>,
            ) -> Result<Value, BridgeError> {
                Ok(json!({}))
            }
            async fn read_content(
                &mut self,
                _heading: Option<&str>,
                _selector: Option<&str>,
                _offset: usize,
                _max_chars: usize,
            ) -> Result<Value, BridgeError> {
                Ok(json!({}))
            }
            async fn wait_for_selector(
                &mut self,
                _selector: &str,
                _timeout_ms: u64,
                _state: Option<&str>,
            ) -> Result<bool, BridgeError> {
                Ok(true)
            }
            async fn select_option(
                &mut self,
                _selector: &str,
                _value: &str,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn evaluate(&mut self, script: &str) -> Result<Value, BridgeError> {
                self.state
                    .lock()
                    .unwrap()
                    .calls
                    .push(format!("evaluate:{script}"));
                Ok(json!({"value": "ok"}))
            }
            async fn hover(&mut self, selector: &str) -> Result<(), BridgeError> {
                let mut state = self.state.lock().unwrap();
                state.calls.push(format!("hover:{selector}"));
                if state.hover_should_fail {
                    return Err(BridgeError::Protocol("hover failed".to_string()));
                }
                Ok(())
            }
            async fn press_key(
                &mut self,
                _key: &str,
                _selector: Option<&str>,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn switch_tab(&mut self, _index: i64) -> Result<Value, BridgeError> {
                Ok(json!({"ok": true}))
            }
            async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
                Ok(BrowserState {
                    cookies: Value::Array(Vec::new()),
                    local_storage: Value::Object(serde_json::Map::new()),
                    url: String::new(),
                })
            }
            async fn import_cookies(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn import_cookies_only(
                &mut self,
                _state: &BrowserState,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn import_local_storage(
                &mut self,
                _state: &BrowserState,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn list_resources(&mut self) -> Result<Value, BridgeError> {
                Ok(json!([]))
            }
            async fn save_file(
                &mut self,
                _url: &str,
                _path: &str,
                _headers: Option<&std::collections::BTreeMap<String, String>>,
            ) -> Result<String, BridgeError> {
                Ok(String::new())
            }
            async fn click(&mut self, _selector: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn click_at(&mut self, _x: f64, _y: f64) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn fill(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn screenshot(
                &mut self,
                _options: &ScreenshotOptions<'_>,
            ) -> Result<(String, usize), BridgeError> {
                Ok((String::new(), 0))
            }
            async fn go_back(&mut self) -> Result<String, BridgeError> {
                Ok(String::new())
            }
            async fn set_device(&mut self, _options: &Value) -> Result<Value, BridgeError> {
                Ok(json!({}))
            }
            async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
                Ok(Vec::new())
            }
            async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
                Ok(())
            }
            async fn get_storage(
                &mut self,
                _storage_type: StorageType,
            ) -> Result<(Vec<StorageEntry>, Vec<StorageEntry>), BridgeError> {
                Ok((Vec::new(), Vec::new()))
            }
        }

        fn make_browser(state: Arc<Mutex<MockState>>) -> BrowserContext {
            let bridge: SharedBridge = Arc::new(AsyncMutex::new(
                Box::new(MockBackend { state }) as Box<dyn BrowserBackend + Send>
            ));
            BrowserContext::new(bridge)
        }

        #[tokio::test]
        async fn hovers_before_evaluating() {
            let state = Arc::new(Mutex::new(MockState::default()));
            let mut browser = make_browser(state.clone());
            let input = json!({"script": "1", "hover_selector": ".btn"});

            let result = execute(&input, &mut browser, &CrawlState::default()).await;

            assert!(result.is_ok());
            assert_eq!(
                state.lock().unwrap().calls,
                vec!["hover:.btn".to_string(), "evaluate:1".to_string()]
            );
        }

        #[tokio::test]
        async fn hover_failure_short_circuits_before_evaluate() {
            let state = Arc::new(Mutex::new(MockState {
                hover_should_fail: true,
                ..Default::default()
            }));
            let mut browser = make_browser(state.clone());
            let input = json!({"script": "1", "hover_selector": ".btn"});

            let result = execute(&input, &mut browser, &CrawlState::default()).await;

            assert!(result.is_err());
            assert_eq!(state.lock().unwrap().calls, vec!["hover:.btn".to_string()]);
        }

        #[tokio::test]
        async fn no_hover_call_without_hover_selector() {
            let state = Arc::new(Mutex::new(MockState::default()));
            let mut browser = make_browser(state.clone());
            let input = json!({"script": "1"});

            let result = execute(&input, &mut browser, &CrawlState::default()).await;

            assert!(result.is_ok());
            assert_eq!(state.lock().unwrap().calls, vec!["evaluate:1".to_string()]);
        }
    }
}
