use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

const MAX_SETTLE_MS: u64 = 5_000;

#[derive(Debug)]
pub struct ExecuteJsInput {
    pub script: String,
    pub hover_selector: Option<String>,
    pub settle_ms: u64,
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

    let settle_ms = match input.get("settle_ms") {
        None => 0,
        Some(v) => {
            let settle_ms = v.as_u64().ok_or_else(|| {
                CrawlError::new("execute_js settle_ms must be a non-negative integer")
            })?;
            if settle_ms > MAX_SETTLE_MS {
                return Err(CrawlError::new(format!(
                    "execute_js settle_ms must be <= {MAX_SETTLE_MS}"
                )));
            }
            settle_ms
        }
    };

    Ok(ExecuteJsInput {
        script,
        hover_selector,
        settle_ms,
    })
}

/// Ensures the script's final expression is returned so an async IIFE
/// wrapper produces a completion value instead of `undefined`.
fn preserve_completion_value(script: &str) -> String {
    if script.trim_start().starts_with("return ") || script.trim_start().starts_with("return\t") {
        return script.to_string();
    }

    let Some(last_semicolon) = script.rfind(';') else {
        return format!("return {};", script.trim());
    };

    let tail = &script[last_semicolon + 1..];
    let trimmed_tail = tail.trim_start();
    if trimmed_tail.is_empty() || trimmed_tail.starts_with("return") {
        return script.to_string();
    }

    let tail_start = last_semicolon + 1 + (tail.len() - trimmed_tail.len());
    format!(
        "{}return {};",
        &script[..tail_start],
        script[tail_start..].trim_end()
    )
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

    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    // The bridge evaluates the script as an expression (Playwright's
    // page.evaluate), so a bare statement list with a top-level `return`
    // would throw `SyntaxError: Illegal return statement`. Wrapping in an
    // async IIFE turns the script into a function body: multi-statement
    // scripts keep their completion-value (last-expression) semantics, and
    // scripts with an explicit `return` now work too. The settle delay stays
    // a separate round-trip rather than being spliced into the script text.
    let transformed = preserve_completion_value(&params.script);
    let wrapped = format!("(async () => {{\n{transformed}\n}})()");
    let result = bridge
        .evaluate(&wrapped)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    if params.settle_ms > 0 {
        bridge
            .evaluate(&format!(
                "await new Promise(r => setTimeout(r, {}))",
                params.settle_ms
            ))
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;
    }

    drop(bridge);

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
    fn preserve_completion_value_single_expression_gets_return() {
        assert_eq!(
            preserve_completion_value("document.title"),
            "return document.title;"
        );
    }

    #[test]
    fn preserve_completion_value_leaves_existing_return_unchanged() {
        assert_eq!(
            preserve_completion_value("return document.title;"),
            "return document.title;"
        );
        assert_eq!(
            preserve_completion_value("  return document.title;  "),
            "  return document.title;  "
        );
    }

    #[test]
    fn preserve_completion_value_multi_statement_adds_return_to_last() {
        assert_eq!(
            preserve_completion_value(
                "document.querySelector('.toggle').click(); document.querySelector('.toggle').getAttribute('aria-checked')"
            ),
            "document.querySelector('.toggle').click(); return document.querySelector('.toggle').getAttribute('aria-checked');"
        );
    }

    #[test]
    fn preserve_completion_value_multi_statement_with_existing_return() {
        assert_eq!(
            preserve_completion_value("a(); return b;"),
            "a(); return b;"
        );
    }

    #[test]
    fn preserve_completion_value_trailing_semicolon_only_unchanged() {
        assert_eq!(preserve_completion_value("a(); b();"), "a(); b();");
        assert_eq!(preserve_completion_value("a();   "), "a();   ");
    }

    #[test]
    fn parses_script() {
        let input = json!({"script": "document.title"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.script, "document.title");
        assert!(parsed.hover_selector.is_none());
        assert_eq!(parsed.settle_ms, 0);
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
    fn parses_script_with_settle_ms() {
        let input = json!({"script": "document.title", "settle_ms": 50});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.script, "document.title");
        assert_eq!(parsed.settle_ms, 50);
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

    #[test]
    fn rejects_negative_settle_ms() {
        let input = json!({"script": "document.title", "settle_ms": -50});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("non-negative"));
    }

    #[test]
    fn rejects_settle_ms_above_max() {
        let input = json!({"script": "document.title", "settle_ms": MAX_SETTLE_MS + 1});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains(&MAX_SETTLE_MS.to_string()));
    }

    #[test]
    fn allows_settle_ms_at_max() {
        let input = json!({"script": "document.title", "settle_ms": MAX_SETTLE_MS});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.settle_ms, MAX_SETTLE_MS);
    }

    #[tokio::test]
    async fn evaluates_multi_statement_script_unmodified_with_settle() {
        use crate::tools::test_support::{
            browser_with_evaluate_recorder, take_recorded_evaluate_scripts,
        };

        let (mut browser, sink) = browser_with_evaluate_recorder();
        let crawl_state = CrawlState::default();

        let script = "document.querySelector('.toggle').click(); document.querySelector('.toggle').getAttribute('aria-checked')";
        let input = json!({"script": script, "settle_ms": 50});

        execute(&input, &mut browser, &crawl_state)
            .await
            .expect("execute should succeed");

        let calls = take_recorded_evaluate_scripts(&sink).await;
        assert!(
            calls[0].starts_with("(async () => {"),
            "the caller's script must be wrapped in an async IIFE"
        );
        assert!(
            calls[0].contains("document.querySelector('.toggle').click();"),
            "the wrapped call must contain the caller's original first statement"
        );
        assert!(
            calls[0].contains(
                "return document.querySelector('.toggle').getAttribute('aria-checked');"
            ),
            "the wrapped call must add a return to the final expression so it becomes the completion value"
        );
        assert_eq!(
            calls.len(),
            2,
            "settle delay must be a separate evaluate call"
        );
        assert!(calls[1].contains("setTimeout"));
        assert!(calls[1].contains("50"));
    }

    #[tokio::test]
    async fn skips_settle_call_when_settle_ms_is_zero() {
        use crate::tools::test_support::{
            browser_with_evaluate_recorder, take_recorded_evaluate_scripts,
        };

        let (mut browser, sink) = browser_with_evaluate_recorder();
        let crawl_state = CrawlState::default();

        let input = json!({"script": "document.title"});
        execute(&input, &mut browser, &crawl_state)
            .await
            .expect("execute should succeed");

        let calls = take_recorded_evaluate_scripts(&sink).await;
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("(async () => {"));
        assert!(calls[0].contains("document.title"));
    }

    #[test]
    fn parses_script_with_top_level_return() {
        let input = json!({"script": "return document.title;"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.script, "return document.title;");
    }

    #[tokio::test]
    async fn wraps_script_with_return_in_async_iife() {
        use crate::tools::test_support::{
            browser_with_evaluate_recorder, take_recorded_evaluate_scripts,
        };

        let (mut browser, sink) = browser_with_evaluate_recorder();
        let crawl_state = CrawlState::default();

        let script = "return document.title;";
        let input = json!({"script": script});

        execute(&input, &mut browser, &crawl_state)
            .await
            .expect("execute should succeed");

        let calls = take_recorded_evaluate_scripts(&sink).await;
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].starts_with("(async () => {"),
            "a script with a top-level return must be wrapped in an async IIFE"
        );
        assert!(calls[0].contains(script));
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
                    html: Some(String::new()),
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
            let calls = state.lock().unwrap().calls.clone();
            assert_eq!(calls[0], "hover:.btn");
            assert!(calls[1].starts_with("evaluate:(async () => {"));
            assert!(calls[1].contains('1'));
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
            let calls = state.lock().unwrap().calls.clone();
            assert_eq!(calls.len(), 1);
            assert!(calls[0].starts_with("evaluate:(async () => {"));
        }

        #[tokio::test]
        async fn hover_then_settle_uses_separate_evaluate_calls() {
            let state = Arc::new(Mutex::new(MockState::default()));
            let mut browser = make_browser(state.clone());
            let input = json!({"script": "1", "hover_selector": ".btn", "settle_ms": 25});

            let result = execute(&input, &mut browser, &CrawlState::default()).await;

            assert!(result.is_ok());
            let calls = state.lock().unwrap().calls.clone();
            assert_eq!(calls[0], "hover:.btn");
            assert!(calls[1].starts_with("evaluate:(async () => {"));
            assert!(calls[1].contains('1'));
            assert!(calls[2].contains("evaluate:") && calls[2].contains("setTimeout"));
        }
    }
}
