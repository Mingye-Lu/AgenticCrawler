use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use acrawl_core::{ScriptLimits, ScriptState, ScriptStatus};
use async_trait::async_trait;
use browser::{
    BridgeError, BrowserBackend, BrowserContext, BrowserState, PageInfo, ScreenshotOptions,
    SharedBridge,
};
use script::grammar::{Expression, ScriptDefinition, ScriptNode};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::ScriptExecutor;

#[derive(Debug)]
struct MockBridge {
    call_log: Arc<Mutex<Vec<(String, Value)>>>,
    next_page_index: AtomicUsize,
    evaluate_responses: Mutex<Vec<Value>>,
    navigate_responses: Mutex<Vec<PageInfo>>,
    click_error: Mutex<Option<String>>,
}

#[allow(dead_code)]
impl MockBridge {
    fn new() -> Self {
        Self {
            call_log: Arc::new(Mutex::new(Vec::new())),
            next_page_index: AtomicUsize::new(1),
            evaluate_responses: Mutex::new(Vec::new()),
            navigate_responses: Mutex::new(Vec::new()),
            click_error: Mutex::new(None),
        }
    }

    fn with_evaluate_responses(mut self, responses: Vec<Value>) -> Self {
        self.evaluate_responses = Mutex::new(responses);
        self
    }

    fn with_navigate_responses(mut self, responses: Vec<PageInfo>) -> Self {
        self.navigate_responses = Mutex::new(responses);
        self
    }

    fn with_click_error(mut self, error: &str) -> Self {
        self.click_error = Mutex::new(Some(error.to_string()));
        self
    }

    fn log(&self, method: &str, args: Value) {
        self.call_log
            .lock()
            .unwrap()
            .push((method.to_string(), args));
    }

    fn call_log(&self) -> Arc<Mutex<Vec<(String, Value)>>> {
        self.call_log.clone()
    }

    fn default_page_map() -> Value {
        json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "interactive": {"counts": {"buttons": 0, "inputs": 0, "selects": 0, "textareas": 0, "total": 0}, "elements": []},
            "meta": {"title": "Mock Page", "url": "https://mock.test", "description": ""},
            "truncated_links": false,
            "truncated_forms": false,
            "truncated_landmarks": false
        })
    }
}

#[async_trait]
impl BrowserBackend for MockBridge {
    async fn poll_observations(&mut self) -> Result<Vec<browser::ObservationEvent>, BridgeError> {
        Ok(Vec::new())
    }
    async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn navigate(&mut self, url: &str) -> Result<PageInfo, BridgeError> {
        self.log("navigate", json!({"url": url}));
        let mut responses = self.navigate_responses.lock().unwrap();
        if responses.is_empty() {
            Ok(PageInfo {
                title: "Mock Page".to_string(),
                html: Some(format!(
                    "<html><head><title>Mock Page</title></head><body><h1>Content for {url}</h1></body></html>"
                )),
            })
        } else {
            Ok(responses.remove(0))
        }
    }

    async fn new_page(&mut self, url: Option<&str>) -> Result<usize, BridgeError> {
        let index = self.next_page_index.fetch_add(1, Ordering::Relaxed);
        self.log("new_page", json!({"url": url, "index": index}));
        Ok(index)
    }

    async fn close_page(&mut self, page_index: usize) -> Result<(), BridgeError> {
        self.log("close_page", json!({"page_index": page_index}));
        Ok(())
    }

    async fn scroll(&mut self, direction: &str, pixels: i64) -> Result<(), BridgeError> {
        self.log("scroll", json!({"direction": direction, "pixels": pixels}));
        Ok(())
    }

    async fn page_map(
        &mut self,
        scope: Option<&str>,
        _compound_enrichment: bool,
        _depth: Option<usize>,
    ) -> Result<Value, BridgeError> {
        self.log("page_map", json!({"scope": scope}));
        Ok(Self::default_page_map())
    }

    async fn read_content(
        &mut self,
        heading: Option<&str>,
        selector: Option<&str>,
        offset: usize,
        max_chars: usize,
    ) -> Result<Value, BridgeError> {
        self.log("read_content", json!({"heading": heading, "selector": selector, "offset": offset, "max_chars": max_chars}));
        Ok(json!({"content": "mock content", "total_chars": 12}))
    }

    async fn wait_for_selector(
        &mut self,
        selector: &str,
        timeout_ms: u64,
        state: Option<&str>,
    ) -> Result<bool, BridgeError> {
        self.log(
            "wait_for_selector",
            json!({"selector": selector, "timeout_ms": timeout_ms, "state": state}),
        );
        Ok(true)
    }

    async fn select_option(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        self.log(
            "select_option",
            json!({"selector": selector, "value": value}),
        );
        Ok(())
    }

    async fn evaluate(&mut self, script: &str) -> Result<Value, BridgeError> {
        self.log("evaluate", json!({"script": script}));
        let mut responses = self.evaluate_responses.lock().unwrap();
        if responses.is_empty() {
            Ok(json!({"value": null}))
        } else {
            Ok(responses.remove(0))
        }
    }

    async fn hover(&mut self, selector: &str) -> Result<(), BridgeError> {
        self.log("hover", json!({"selector": selector}));
        Ok(())
    }

    async fn press_key(&mut self, key: &str, selector: Option<&str>) -> Result<(), BridgeError> {
        self.log("press_key", json!({"key": key, "selector": selector}));
        Ok(())
    }

    async fn switch_tab(&mut self, index: i64) -> Result<Value, BridgeError> {
        self.log("switch_tab", json!({"index": index}));
        Ok(json!({"url": "https://mock.test", "title": "Mock Page"}))
    }

    async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
        Err(BridgeError::Protocol("not supported in mock".into()))
    }

    async fn import_cookies(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("not supported in mock".into()))
    }

    async fn import_cookies_only(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("not supported in mock".into()))
    }

    async fn import_local_storage(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("not supported in mock".into()))
    }

    async fn list_resources(&mut self) -> Result<Value, BridgeError> {
        self.log("list_resources", json!({}));
        Ok(json!({"links": [], "images": [], "forms": []}))
    }

    async fn save_file(
        &mut self,
        url: &str,
        path: &str,
        _headers: Option<&BTreeMap<String, String>>,
    ) -> Result<String, BridgeError> {
        self.log("save_file", json!({"url": url, "path": path}));
        Ok(path.to_string())
    }

    async fn click(&mut self, selector: &str) -> Result<(), BridgeError> {
        self.log("click", json!({"selector": selector}));
        let error = self.click_error.lock().unwrap();
        if let Some(msg) = error.as_ref() {
            Err(BridgeError::Protocol(msg.clone()))
        } else {
            Ok(())
        }
    }

    async fn click_at(&mut self, x: f64, y: f64) -> Result<(), BridgeError> {
        self.log("click_at", json!({"x": x, "y": y}));
        Ok(())
    }

    async fn fill(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        self.log("fill", json!({"selector": selector, "value": value}));
        Ok(())
    }

    async fn screenshot(
        &mut self,
        _options: &ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError> {
        self.log("screenshot", json!({}));
        Ok(("base64data".to_string(), 10))
    }

    async fn go_back(&mut self) -> Result<String, BridgeError> {
        self.log("go_back", json!({}));
        Ok("https://mock.test/previous".to_string())
    }

    async fn set_device(
        &mut self,
        options: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        self.log("set_device", json!({"options": options}));
        Ok(json!({"success": true}))
    }
}

fn default_limits() -> ScriptLimits {
    ScriptLimits {
        max_steps: 100,
        max_timeout_secs: 300,
        max_output_bytes: 1_048_576,
        max_script_size_bytes: 1_048_576,
        max_parallel_branches: 4,
        max_nesting_depth: 10,
        per_step_timeout_secs: 30,
    }
}

fn low_step_limits(max_steps: usize) -> ScriptLimits {
    ScriptLimits {
        max_steps,
        max_timeout_secs: 300,
        max_output_bytes: 1_048_576,
        max_script_size_bytes: 1_048_576,
        max_parallel_branches: 4,
        max_nesting_depth: 10,
        per_step_timeout_secs: 30,
    }
}

fn make_shared_bridge(mock: MockBridge) -> SharedBridge {
    Arc::new(tokio::sync::Mutex::new(
        Box::new(mock) as Box<dyn BrowserBackend + Send>
    ))
}

fn make_executor(bridge: SharedBridge, limits: ScriptLimits) -> ScriptExecutor {
    let shared_state = Arc::new(RwLock::new(ScriptState {
        script_id: "test_script".to_string(),
        status: ScriptStatus::Pending,
        step: 0,
        total_steps: None,
        current_url: None,
        items_collected: 0,
        elapsed_secs: 0.0,
        errors_caught: 0,
        yielded_data: Vec::new(),
    }));
    let browser = BrowserContext::new(bridge);
    let cancel_token = CancellationToken::new();

    ScriptExecutor::new(
        "test_script".to_string(),
        browser,
        limits,
        shared_state,
        cancel_token,
    )
}

fn make_executor_with_cancel(
    bridge: SharedBridge,
    limits: ScriptLimits,
) -> (ScriptExecutor, CancellationToken) {
    let shared_state = Arc::new(RwLock::new(ScriptState {
        script_id: "test_script".to_string(),
        status: ScriptStatus::Pending,
        step: 0,
        total_steps: None,
        current_url: None,
        items_collected: 0,
        elapsed_secs: 0.0,
        errors_caught: 0,
        yielded_data: Vec::new(),
    }));
    let browser = BrowserContext::new(bridge);
    let cancel_token = CancellationToken::new();
    let token_clone = cancel_token.clone();

    let executor = ScriptExecutor::new(
        "test_script".to_string(),
        browser,
        limits,
        shared_state,
        cancel_token,
    );
    (executor, token_clone)
}

fn script(steps: Vec<ScriptNode>) -> ScriptDefinition {
    ScriptDefinition {
        schema_version: 1,
        name: Some("test".to_string()),
        steps,
    }
}

fn literal(value: Value) -> Expression {
    Expression::Literal(value)
}

fn variable(name: &str) -> Expression {
    Expression::Variable(name.to_string())
}

fn js_eval(code: &str) -> Expression {
    Expression::JsEval(code.to_string())
}

fn tool_call(tool: &str, input: Value, output: Option<&str>) -> ScriptNode {
    ScriptNode::ToolCall {
        tool: tool.to_string(),
        input,
        output: output.map(String::from),
    }
}

#[tokio::test]
async fn sequential_three_tool_calls() {
    let mock = MockBridge::new();
    let log = mock.call_log();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            tool_call("scroll", json!({"direction": "down", "pixels": 300}), None),
            tool_call("scroll", json!({"direction": "down", "pixels": 500}), None),
            tool_call("scroll", json!({"direction": "up", "pixels": 200}), None),
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.steps_executed, 3);
    assert!(result.error.is_none());

    let calls = log.lock().unwrap();
    let scroll_calls: Vec<_> = calls.iter().filter(|(m, _)| m == "scroll").collect();
    assert_eq!(scroll_calls.len(), 3);
}

#[tokio::test]
async fn variable_assignment_and_usage() {
    let mock = MockBridge::new().with_evaluate_responses(vec![json!({"value": "hello world"})]);
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "greeting".to_string(),
                value: literal(json!("hello")),
            },
            tool_call("execute_js", json!({"script": "$greeting"}), None),
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn tool_call_output_captured_as_variable() {
    let mock = MockBridge::new().with_evaluate_responses(vec![json!({"value": "page title"})]);
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            tool_call(
                "execute_js",
                json!({"script": "document.title"}),
                Some("title"),
            ),
            ScriptNode::Collect {
                value: variable("title"),
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 1);
    let collected = &result.extracted_data[0];
    assert!(collected.get("success").is_some() || collected.get("result").is_some());
}

#[tokio::test]
async fn collect_accumulates_extracted_data() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Collect {
                value: literal(json!({"item": "first"})),
            },
            ScriptNode::Collect {
                value: literal(json!({"item": "second"})),
            },
            ScriptNode::Collect {
                value: literal(json!({"item": "third"})),
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 3);
    assert_eq!(result.extracted_data[0], json!({"item": "first"}));
    assert_eq!(result.extracted_data[1], json!({"item": "second"}));
    assert_eq!(result.extracted_data[2], json!({"item": "third"}));
}

#[tokio::test]
async fn yield_appends_to_yielded_data() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());
    let yielded = executor.yielded_data.clone();

    let result = executor
        .execute(script(vec![
            ScriptNode::Yield {
                value: literal(json!("row_1")),
            },
            ScriptNode::Yield {
                value: literal(json!("row_2")),
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.yielded_data.len(), 2);
    assert_eq!(result.yielded_data[0], json!("row_1"));
    assert_eq!(result.yielded_data[1], json!("row_2"));

    let live = yielded.read().unwrap();
    assert_eq!(live.len(), 2);
}

#[tokio::test]
async fn for_loop_correct_iterations() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::ForLoop {
            variable: "i".to_string(),
            from: literal(json!(0)),
            to: literal(json!(5)),
            steps: vec![ScriptNode::Collect {
                value: variable("i"),
            }],
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 5);
    for (idx, item) in result.extracted_data.iter().enumerate() {
        assert_eq!(*item, json!(idx));
    }
}

#[tokio::test]
async fn for_each_iterates_array() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "items".to_string(),
                value: literal(json!(["apple", "banana", "cherry"])),
            },
            ScriptNode::ForEach {
                variable: "fruit".to_string(),
                iterable: variable("items"),
                steps: vec![ScriptNode::Collect {
                    value: variable("fruit"),
                }],
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 3);
    assert_eq!(result.extracted_data[0], json!("apple"));
    assert_eq!(result.extracted_data[1], json!("banana"));
    assert_eq!(result.extracted_data[2], json!("cherry"));
}

#[tokio::test]
async fn for_each_iterates_object() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "data".to_string(),
                value: literal(json!({"a": 1, "b": 2})),
            },
            ScriptNode::ForEach {
                variable: "entry".to_string(),
                iterable: variable("data"),
                steps: vec![ScriptNode::Collect {
                    value: variable("entry"),
                }],
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 2);
    for item in &result.extracted_data {
        assert!(item.get("key").is_some());
        assert!(item.get("value").is_some());
    }
}

#[tokio::test]
async fn while_loop_exits_on_false_condition() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    // Given counter=3 (truthy), loop body collects then sets counter=0 (falsy) → one iteration
    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "counter".to_string(),
                value: literal(json!(3)),
            },
            ScriptNode::WhileLoop {
                condition: variable("counter"),
                steps: vec![
                    ScriptNode::Collect {
                        value: variable("counter"),
                    },
                    ScriptNode::Assign {
                        variable: "counter".to_string(),
                        value: literal(json!(0)),
                    },
                ],
            },
        ]))
        .await;

    assert_eq!(result.extracted_data.len(), 1);
    assert_eq!(result.extracted_data[0], json!(3));
}

#[tokio::test]
async fn while_loop_step_limit_terminates() {
    let mock = MockBridge::new().with_evaluate_responses(vec![
        json!({"value": true}),
        json!({"value": true}),
        json!({"value": true}),
        json!({"value": true}),
        json!({"value": true}),
    ]);
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, low_step_limits(3));

    let result = executor
        .execute(script(vec![ScriptNode::WhileLoop {
            condition: literal(json!(true)),
            steps: vec![tool_call("execute_js", json!({"script": "1+1"}), None)],
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(result.error.as_ref().unwrap().contains("step limit"));
}

#[tokio::test]
async fn if_else_then_branch() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::IfElse {
            condition: literal(json!(true)),
            then_steps: vec![ScriptNode::Collect {
                value: literal(json!("then_executed")),
            }],
            else_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("else_executed")),
            }]),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 1);
    assert_eq!(result.extracted_data[0], json!("then_executed"));
}

#[tokio::test]
async fn if_else_else_branch() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::IfElse {
            condition: literal(json!(false)),
            then_steps: vec![ScriptNode::Collect {
                value: literal(json!("then_executed")),
            }],
            else_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("else_executed")),
            }]),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 1);
    assert_eq!(result.extracted_data[0], json!("else_executed"));
}

#[tokio::test]
async fn try_catch_catches_tool_error() {
    let mock = MockBridge::new().with_click_error("element not found");
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::TryCatch {
            try_steps: vec![tool_call(
                "click",
                json!({"selector": "#nonexistent"}),
                None,
            )],
            catch_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("error_caught")),
            }]),
            finally_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("finally_ran")),
            }]),
            error_var: Some("err".to_string()),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 2);
    assert_eq!(result.extracted_data[0], json!("error_caught"));
    assert_eq!(result.extracted_data[1], json!("finally_ran"));
}

#[tokio::test]
async fn try_catch_finally_always_runs() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::TryCatch {
            try_steps: vec![ScriptNode::Collect {
                value: literal(json!("try_ok")),
            }],
            catch_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("catch_ran")),
            }]),
            finally_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("finally_ran")),
            }]),
            error_var: None,
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 2);
    assert_eq!(result.extracted_data[0], json!("try_ok"));
    assert_eq!(result.extracted_data[1], json!("finally_ran"));
}

#[tokio::test]
async fn try_catch_step_limit_not_catchable() {
    let mock = MockBridge::new().with_evaluate_responses(vec![
        json!({"value": true}),
        json!({"value": true}),
        json!({"value": true}),
    ]);
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, low_step_limits(2));

    let result = executor
        .execute(script(vec![ScriptNode::TryCatch {
            try_steps: vec![
                tool_call("execute_js", json!({"script": "1"}), None),
                tool_call("execute_js", json!({"script": "2"}), None),
                tool_call("execute_js", json!({"script": "3"}), None),
            ],
            catch_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("should_not_catch")),
            }]),
            finally_steps: None,
            error_var: None,
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(result.error.as_ref().unwrap().contains("step limit"));
    assert!(result.extracted_data.is_empty());
}

#[tokio::test]
async fn parallel_two_branches() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::Parallel {
            branches: vec![
                vec![ScriptNode::Collect {
                    value: literal(json!("branch_a")),
                }],
                vec![ScriptNode::Collect {
                    value: literal(json!("branch_b")),
                }],
            ],
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 2);
    assert_eq!(result.extracted_data[0], json!("branch_a"));
    assert_eq!(result.extracted_data[1], json!("branch_b"));
}

#[tokio::test]
async fn parallel_branches_share_output_byte_budget() {
    // Each branch's own collected item (10 bytes: `"branch_a"`) fits under a
    // per-branch view of the limit, but the two branches together (20 bytes)
    // must not both succeed once the budget is properly shared.
    let tiny_limits = ScriptLimits {
        max_output_bytes: 15,
        ..default_limits()
    };
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, tiny_limits);

    let result = executor
        .execute(script(vec![ScriptNode::Parallel {
            branches: vec![
                vec![ScriptNode::Collect {
                    value: literal(json!("branch_a")),
                }],
                vec![ScriptNode::Collect {
                    value: literal(json!("branch_b")),
                }],
            ],
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("output size limit exceeded"),
        "expected 'output size limit exceeded' in: {:?}",
        result.error
    );
}

#[tokio::test]
async fn parallel_max_branches_enforced() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let limits = ScriptLimits {
        max_parallel_branches: 2,
        ..default_limits()
    };
    let executor = make_executor(bridge, limits);

    let result = executor
        .execute(script(vec![ScriptNode::Parallel {
            branches: vec![
                vec![ScriptNode::Collect {
                    value: literal(json!("a")),
                }],
                vec![ScriptNode::Collect {
                    value: literal(json!("b")),
                }],
                vec![ScriptNode::Collect {
                    value: literal(json!("c")),
                }],
            ],
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(result.error.as_ref().unwrap().contains("exceeds limit"));
}

#[tokio::test]
async fn step_limit_stops_execution() {
    let mock = MockBridge::new().with_evaluate_responses(vec![
        json!({"value": 1}),
        json!({"value": 2}),
        json!({"value": 3}),
        json!({"value": 4}),
        json!({"value": 5}),
    ]);
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, low_step_limits(2));

    let result = executor
        .execute(script(vec![
            tool_call("execute_js", json!({"script": "1"}), None),
            tool_call("execute_js", json!({"script": "2"}), None),
            tool_call("execute_js", json!({"script": "3"}), None),
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(result.error.as_ref().unwrap().contains("step limit"));
    assert!(result.steps_executed <= 3);
}

#[tokio::test]
async fn variable_substitution_in_tool_input() {
    let mock = MockBridge::new().with_evaluate_responses(vec![json!({"value": "executed"})]);
    let log = mock.call_log();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "my_script".to_string(),
                value: literal(json!("document.title")),
            },
            tool_call("execute_js", json!({"script": "$my_script"}), None),
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);

    let calls = log.lock().unwrap();
    let eval_calls: Vec<_> = calls.iter().filter(|(m, _)| m == "evaluate").collect();
    assert_eq!(eval_calls.len(), 1);
    assert_eq!(eval_calls[0].1["script"], "document.title");
}

#[tokio::test]
async fn nested_for_loop_inside_for_each() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "pages".to_string(),
                value: literal(json!(["page1", "page2"])),
            },
            ScriptNode::ForEach {
                variable: "page".to_string(),
                iterable: variable("pages"),
                steps: vec![ScriptNode::ForLoop {
                    variable: "i".to_string(),
                    from: literal(json!(0)),
                    to: literal(json!(2)),
                    steps: vec![ScriptNode::Collect {
                        value: variable("page"),
                    }],
                }],
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 4);
    assert_eq!(result.extracted_data[0], json!("page1"));
    assert_eq!(result.extracted_data[1], json!("page1"));
    assert_eq!(result.extracted_data[2], json!("page2"));
    assert_eq!(result.extracted_data[3], json!("page2"));
}

#[tokio::test]
async fn disallowed_tool_returns_error() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![tool_call(
            "fork",
            json!({"goal": "hack the planet"}),
            None,
        )]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(result.error.as_ref().unwrap().contains("not allowed"));
}

#[tokio::test]
async fn cancellation_stops_execution() {
    let mock =
        MockBridge::new().with_evaluate_responses(vec![json!({"value": 1}), json!({"value": 2})]);
    let bridge = make_shared_bridge(mock);
    let (executor, cancel_token) = make_executor_with_cancel(bridge, default_limits());

    cancel_token.cancel();

    let result = executor
        .execute(script(vec![
            tool_call("execute_js", json!({"script": "1"}), None),
            tool_call("execute_js", json!({"script": "2"}), None),
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Cancelled);
}

#[tokio::test]
async fn variable_not_found_error() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::Collect {
            value: variable("undefined_var"),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("variable not found"));
}

#[tokio::test]
async fn js_eval_expression() {
    let mock = MockBridge::new().with_evaluate_responses(vec![json!({"value": 42, "result": 42})]);
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::Collect {
            value: js_eval("2 + 2"),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 1);
    assert_eq!(result.extracted_data[0], json!(42));
}

#[tokio::test]
async fn field_access_expression() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "obj".to_string(),
                value: literal(json!({"name": "acrawl", "version": "1.0"})),
            },
            ScriptNode::Collect {
                value: Expression::FieldAccess {
                    object: Box::new(variable("obj")),
                    field: "name".to_string(),
                },
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data[0], json!("acrawl"));
}

#[tokio::test]
async fn if_else_null_is_falsy() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![
            ScriptNode::Assign {
                variable: "val".to_string(),
                value: literal(Value::Null),
            },
            ScriptNode::IfElse {
                condition: variable("val"),
                then_steps: vec![ScriptNode::Collect {
                    value: literal(json!("truthy")),
                }],
                else_steps: Some(vec![ScriptNode::Collect {
                    value: literal(json!("falsy")),
                }]),
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data[0], json!("falsy"));
}

#[tokio::test]
async fn if_else_empty_string_is_falsy() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::IfElse {
            condition: literal(json!("")),
            then_steps: vec![ScriptNode::Collect {
                value: literal(json!("truthy")),
            }],
            else_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("falsy")),
            }]),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data[0], json!("falsy"));
}

#[tokio::test]
async fn if_else_nonempty_array_is_truthy() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::IfElse {
            condition: literal(json!([1, 2, 3])),
            then_steps: vec![ScriptNode::Collect {
                value: literal(json!("truthy")),
            }],
            else_steps: Some(vec![ScriptNode::Collect {
                value: literal(json!("falsy")),
            }]),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data[0], json!("truthy"));
}

#[tokio::test]
async fn completed_script_reports_elapsed_time() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::Collect {
            value: literal(json!("done")),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert!(result.elapsed_secs >= 0.0);
    assert_eq!(result.script_id, "test_script");
}

#[tokio::test]
async fn try_catch_binds_error_var() {
    let mock = MockBridge::new().with_click_error("click failed: timeout");
    let bridge = make_shared_bridge(mock);
    let executor = make_executor(bridge, default_limits());

    let result = executor
        .execute(script(vec![ScriptNode::TryCatch {
            try_steps: vec![tool_call("click", json!({"selector": "#btn"}), None)],
            catch_steps: Some(vec![ScriptNode::Collect {
                value: variable("err_msg"),
            }]),
            finally_steps: None,
            error_var: Some("err_msg".to_string()),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);
    assert_eq!(result.extracted_data.len(), 1);
    let err_str = result.extracted_data[0].as_str().unwrap();
    assert!(err_str.contains("click failed"), "got: {err_str}");
}

#[tokio::test]
async fn shared_state_reflects_progress() {
    let mock = MockBridge::new();
    let bridge = make_shared_bridge(mock);
    let shared_state = Arc::new(RwLock::new(ScriptState {
        script_id: "progress_test".to_string(),
        status: ScriptStatus::Pending,
        step: 0,
        total_steps: None,
        current_url: None,
        items_collected: 0,
        elapsed_secs: 0.0,
        errors_caught: 0,
        yielded_data: Vec::new(),
    }));
    let browser = BrowserContext::new(bridge);
    let cancel_token = CancellationToken::new();

    let executor = ScriptExecutor::new(
        "progress_test".to_string(),
        browser,
        default_limits(),
        shared_state.clone(),
        cancel_token,
    );

    let result = executor
        .execute(script(vec![
            ScriptNode::Collect {
                value: literal(json!("a")),
            },
            ScriptNode::Collect {
                value: literal(json!("b")),
            },
        ]))
        .await;

    assert_eq!(result.status, ScriptStatus::Completed);

    let state = shared_state.read().unwrap();
    assert_eq!(state.status, ScriptStatus::Completed);
    assert_eq!(state.items_collected, 2);
}

#[tokio::test]
async fn collect_over_output_byte_limit_fails() {
    let tiny_limits = ScriptLimits {
        max_output_bytes: 10,
        ..default_limits()
    };
    let bridge = make_shared_bridge(MockBridge::new());
    let executor = make_executor(bridge, tiny_limits);

    let result = executor
        .execute(script(vec![ScriptNode::Collect {
            value: literal(json!("hello world")),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("output size limit exceeded"),
        "expected 'output size limit exceeded' in: {:?}",
        result.error
    );
    assert_eq!(result.extracted_data.len(), 0);
}

#[tokio::test]
async fn yield_over_output_byte_limit_fails() {
    let tiny_limits = ScriptLimits {
        max_output_bytes: 5,
        ..default_limits()
    };
    let bridge = make_shared_bridge(MockBridge::new());
    let executor = make_executor(bridge, tiny_limits);

    let result = executor
        .execute(script(vec![ScriptNode::Yield {
            value: literal(json!("too long")),
        }]))
        .await;

    assert_eq!(result.status, ScriptStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("output size limit exceeded"),
        "expected 'output size limit exceeded' in: {:?}",
        result.error
    );
}
