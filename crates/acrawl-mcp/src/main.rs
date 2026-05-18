use std::collections::BTreeSet;
use std::io::{self, BufRead, Write};
use std::sync::Mutex;

use api::provider::{model_api_id, ProviderClient, ProviderRegistry};
use api::{
    ContentBlockDelta, ContentBlockDeltaEvent, InputContentBlock, InputMessage, MessageRequest,
    StreamEvent,
};
use api::{ImageSource, OutputContentBlock, ToolChoice, ToolDefinition};
use crawler::{mvp_tool_specs, CrawlerAgent, ToolRegistry};
use runtime::{ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage};
use runtime::{MessageRole, RuntimeError, TokenUsage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

static JOB_MUTEX: Mutex<()> = Mutex::new(());

const SERVER_NAME: &str = "acrawl-mcp-server";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

fn send_response(response: &JsonRpcResponse) {
    let json = serde_json::to_string(response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal serialization error"}}"#
            .to_string()
    });
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{json}");
    let _ = stdout.flush();
}

fn send_error(id: Option<Value>, code: i32, message: String) {
    send_response(&JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(JsonRpcError { code, message }),
    });
}

fn initialize_response(id: Option<Value>) {
    let result = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION,
        },
    });
    send_response(&JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(result),
        error: None,
    });
}

fn tools_list_response(id: Option<Value>) {
    let tools = json!([
        {
            "name": "run_goal",
            "description": "Execute a high-level crawl goal using acrawl's browser agent and return structured results. The agent plans, navigates, and extracts data autonomously.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "goal": {
                        "type": "string",
                        "description": "Natural-language crawl goal"
                    },
                    "model": {
                        "type": "string",
                        "description": "Model to use (optional; uses default from credentials if omitted)"
                    },
                    "allowed_tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict which built-in tools the agent can use (optional; validated but runtime filtering is not yet enforced)"
                    },
                    "max_steps": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum agent steps (optional; default from settings)"
                    }
                },
                "required": ["goal"]
            }
        },
        {
            "name": "list_builtin_tools",
            "description": "List acrawl's built-in crawl tool capabilities (read-only metadata). Returns names, descriptions, and input schemas for the 19 internal tools.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }
    ]);
    send_response(&JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(json!({ "tools": tools })),
        error: None,
    });
}

fn resolve_model(model: Option<&str>) -> Result<String, String> {
    if let Some(m) = model {
        if !m.is_empty() {
            return Ok(m.to_string());
        }
    }
    let settings = runtime::load_settings();
    settings
        .model
        .filter(|m| !m.is_empty() && m.contains('/'))
        .ok_or_else(|| {
            "no model configured: set a default via `acrawl auth` or pass `model` in the request"
                .to_string()
        })
}

fn build_provider(model: &str) -> Result<ProviderClient, String> {
    let store = api::load_credentials().unwrap_or_default();
    let registry = ProviderRegistry::from_credentials(&store);
    if model.is_empty() {
        return Ok(ProviderClient::no_auth_placeholder());
    }
    registry
        .build_client(model, &store)
        .map_err(|e| format!("failed to build provider client for model `{model}`: {e}"))
}

fn validate_tool_names(names: &[String]) -> Result<(), String> {
    let valid: BTreeSet<&str> = mvp_tool_specs().iter().map(|s| s.name).collect();
    for name in names {
        let normalized = name.replace('-', "_").to_lowercase();
        if !valid.contains(normalized.as_str()) {
            let mut sorted: Vec<&str> = valid.iter().copied().collect();
            sorted.sort_unstable();
            return Err(format!(
                "unknown tool `{name}`: valid built-in tools are: {}",
                sorted.join(", ")
            ));
        }
    }
    Ok(())
}

struct CrawlApiClient {
    provider: ProviderClient,
    model: String,
    tool_names: Vec<String>,
    max_tokens: u32,
}

impl CrawlApiClient {
    fn new(provider: ProviderClient, model: &str, tool_names: Vec<String>) -> Self {
        let max_tokens =
            ProviderRegistry::from_credentials(&api::load_credentials().unwrap_or_default())
                .max_tokens(model);
        Self {
            provider,
            model: model.to_string(),
            tool_names,
            max_tokens,
        }
    }

    fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    MessageRole::User | MessageRole::System | MessageRole::Tool => "user",
                    MessageRole::Assistant => "assistant",
                };
                let content: Vec<InputContentBlock> = msg
                    .blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => {
                            InputContentBlock::Text { text: text.clone() }
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let parsed: Value =
                                serde_json::from_str(input).unwrap_or(json!({"raw": input}));
                            InputContentBlock::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input: parsed,
                            }
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            tool_name,
                            output,
                            is_error,
                        } => {
                            if tool_name == "screenshot" {
                                if let Ok(val) = serde_json::from_str::<Value>(output) {
                                    if let Some(b64) =
                                        val.get("screenshot_base64").and_then(Value::as_str)
                                    {
                                        let blocks = vec![
                                            api::ToolResultContentBlock::Image {
                                                source: ImageSource {
                                                    source_type: "base64".to_string(),
                                                    media_type: "image/png".to_string(),
                                                    data: b64.to_string(),
                                                },
                                            },
                                            api::ToolResultContentBlock::Text {
                                                text: format!(
                                                    "screenshot: {} bytes",
                                                    val.get("size_bytes")
                                                        .and_then(Value::as_u64)
                                                        .unwrap_or(0)
                                                ),
                                            },
                                        ];
                                        return InputContentBlock::ToolResult {
                                            tool_use_id: tool_use_id.clone(),
                                            content: blocks,
                                            is_error: *is_error,
                                        };
                                    }
                                }
                            }
                            InputContentBlock::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content: vec![api::ToolResultContentBlock::Text {
                                    text: output.clone(),
                                }],
                                is_error: *is_error,
                            }
                        }
                        ContentBlock::Reasoning { data } => {
                            let parsed: Value =
                                serde_json::from_str(data).unwrap_or(json!({"raw": data}));
                            InputContentBlock::Reasoning { data: parsed }
                        }
                    })
                    .collect();
                InputMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .collect()
    }

    fn build_tool_defs(&self) -> Vec<ToolDefinition> {
        mvp_tool_specs()
            .into_iter()
            .filter(|spec| {
                self.tool_names.is_empty() || self.tool_names.contains(&spec.name.to_string())
            })
            .map(|spec| ToolDefinition {
                name: spec.name.to_string(),
                description: Some(spec.description.to_string()),
                input_schema: spec.input_schema,
            })
            .collect()
    }
}

impl ApiClient for CrawlApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let tools = self.build_tool_defs();
        let message_request = MessageRequest {
            model: model_api_id(&self.model).to_string(),
            max_tokens: self.max_tokens,
            messages: Self::convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: Some(tools),
            tool_choice: Some(ToolChoice::Auto),
            stream: true,
            reasoning_effort: None,
        };

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut stream = self
                    .provider
                    .stream_message(&message_request)
                    .await
                    .map_err(|e: api::ApiError| RuntimeError::new(e.to_string()))?;

                let mut events: Vec<AssistantEvent> = Vec::new();
                let mut pending_tool: Option<(String, String, String)> = None;
                let mut pending_reasoning: Option<String> = None;

                loop {
                    let event = stream
                        .next_event()
                        .await
                        .map_err(|e: api::ApiError| RuntimeError::new(e.to_string()))?;
                    match event {
                        Some(StreamEvent::MessageStart(_)) => {}
                        Some(StreamEvent::ContentBlockStart(start)) => match start.content_block {
                            OutputContentBlock::Text { text } => {
                                if !text.is_empty() {
                                    events.push(AssistantEvent::TextDelta(text));
                                }
                            }
                            OutputContentBlock::ToolUse { id, name, input } => {
                                let input_str = if input.is_object()
                                    && input.as_object().is_some_and(serde_json::Map::is_empty)
                                {
                                    String::new()
                                } else {
                                    serde_json::to_string(&input).unwrap_or_default()
                                };
                                pending_tool = Some((id, name, input_str));
                            }
                            OutputContentBlock::Reasoning => {
                                pending_reasoning = Some(String::new());
                            }
                        },
                        Some(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                            delta,
                            ..
                        })) => match delta {
                            ContentBlockDelta::TextDelta { text } => {
                                events.push(AssistantEvent::TextDelta(text));
                            }
                            ContentBlockDelta::InputJsonDelta { partial_json } => {
                                if let Some((_, _, ref mut input)) = pending_tool {
                                    input.push_str(&partial_json);
                                }
                            }
                            ContentBlockDelta::ThinkingDelta { thinking } => {
                                if let Some(ref mut reasoning) = pending_reasoning {
                                    reasoning.push_str(&thinking);
                                }
                            }
                        },
                        Some(StreamEvent::ContentBlockStop(_)) => {
                            if let Some((id, name, input)) = pending_tool.take() {
                                events.push(AssistantEvent::ToolUse { id, name, input });
                            }
                            if let Some(data) = pending_reasoning.take() {
                                events.push(AssistantEvent::Reasoning { data });
                            }
                        }
                        Some(StreamEvent::MessageDelta(delta)) => {
                            events.push(AssistantEvent::Usage(TokenUsage {
                                input_tokens: delta.usage.input_tokens,
                                output_tokens: delta.usage.output_tokens,
                                cache_creation_input_tokens: delta
                                    .usage
                                    .cache_creation_input_tokens,
                                cache_read_input_tokens: delta.usage.cache_read_input_tokens,
                            }));
                        }
                        Some(StreamEvent::MessageStop(_)) | None => {
                            events.push(AssistantEvent::MessageStop);
                            break;
                        }
                    }
                }

                Ok(events)
            })
        })
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation
)]
fn handle_run_goal(id: Option<Value>, arguments: Value) {
    let Some(goal) = arguments.get("goal").and_then(Value::as_str) else {
        send_error(id, -32602, "missing required parameter: goal".to_string());
        return;
    };

    let model = match resolve_model(arguments.get("model").and_then(Value::as_str)) {
        Ok(m) => m,
        Err(e) => {
            send_error(id, -32602, e);
            return;
        }
    };

    let allowed_tools: Vec<String> = arguments
        .get("allowed_tools")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();

    if !allowed_tools.is_empty() {
        if let Err(e) = validate_tool_names(&allowed_tools) {
            send_error(id, -32602, e);
            return;
        }
    }

    let max_steps = arguments
        .get("max_steps")
        .and_then(Value::as_u64)
        .map(|n| n as usize);

    if let Some(ms) = max_steps {
        if !(1..=200).contains(&ms) {
            send_error(
                id,
                -32602,
                format!("max_steps must be between 1 and 200, got {ms}"),
            );
            return;
        }
    }

    let provider = match build_provider(&model) {
        Ok(p) => p,
        Err(e) => {
            send_error(id, -32603, e);
            return;
        }
    };

    let api_client = CrawlApiClient::new(provider, &model, allowed_tools.clone());

    let registry = ToolRegistry::new_with_options(false);
    let mut agent = CrawlerAgent::new_lazy(registry);
    if let Some(ms) = max_steps {
        agent = agent.with_max_steps(ms);
    }

    let _guard = match JOB_MUTEX.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            send_error(id, -32603, format!("failed to create tokio runtime: {e}"));
            return;
        }
    };

    match rt.block_on(agent.run(goal, api_client)) {
        Ok(result) => {
            let response = json!({
                "content": [
                    {
                        "type": "text",
                        "text": format!(
                            "Crawl completed in {} steps.\n\n{}",
                            result.steps_executed,
                            result.summary
                        )
                    }
                ],
                "structuredContent": {
                    "summary": result.summary,
                    "extracted_data": result.extracted_data,
                    "steps_executed": result.steps_executed,
                    "model_used": model,
                    "allowed_tools": allowed_tools,
                    "goal": goal,
                },
                "isError": false,
            });
            send_response(&JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(response),
                error: None,
            });
        }
        Err(e) => {
            let response = json!({
                "content": [
                    {
                        "type": "text",
                        "text": format!("Crawl failed: {e}")
                    }
                ],
                "isError": true,
            });
            send_response(&JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(response),
                error: None,
            });
        }
    }
}

fn handle_list_builtin_tools(id: Option<Value>) {
    let tools: Vec<Value> = mvp_tool_specs()
        .into_iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "input_schema": spec.input_schema,
                "instructions": spec.instructions,
            })
        })
        .collect();

    let result = json!({
        "content": [
            {
                "type": "text",
                "text": format!(
                    "acrawl provides {} built-in crawl tools (informational only - not registered as callable MCP tools).",
                    tools.len()
                )
            }
        ],
        "structuredContent": {
            "tool_count": tools.len(),
            "tools": tools,
        },
        "isError": false,
    });
    send_response(&JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(result),
        error: None,
    });
}

fn handle_tools_call(id: Option<Value>, params: Option<Value>) {
    let Some(params) = params else {
        send_error(id, -32602, "missing params".to_string());
        return;
    };

    let Some(name) = params.get("name").and_then(Value::as_str) else {
        send_error(id, -32602, "missing tool name".to_string());
        return;
    };

    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match name {
        "run_goal" => handle_run_goal(id, arguments),
        "list_builtin_tools" => handle_list_builtin_tools(id),
        other => {
            send_error(
                id,
                -32601,
                format!("unknown tool: {other} (available: run_goal, list_builtin_tools)"),
            );
        }
    }
}

fn main() {
    let stdin = io::stdin().lock();
    let reader = io::BufReader::new(stdin);

    for line in reader.lines() {
        let Ok(line) = line else { break };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                send_error(None, -32700, format!("parse error: {e}"));
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            send_error(
                request.id,
                -32600,
                "invalid request: jsonrpc must be 2.0".to_string(),
            );
            continue;
        }

        match request.method.as_str() {
            "initialize" => initialize_response(request.id),
            "notifications/initialized" => {}
            "tools/list" => tools_list_response(request.id),
            "tools/call" => handle_tools_call(request.id, request.params),
            method => {
                send_error(request.id, -32601, format!("method not found: {method}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_tool_names_accepts_valid_names() {
        let names = vec!["navigate".to_string(), "click".to_string()];
        assert!(validate_tool_names(&names).is_ok());
    }

    #[test]
    fn validate_tool_names_rejects_unknown_tool() {
        let names = vec!["navigate".to_string(), "nonexistent_tool".to_string()];
        let err = validate_tool_names(&names).unwrap_err();
        assert!(err.contains("unknown tool"));
        assert!(err.contains("nonexistent_tool"));
    }

    #[test]
    fn validate_tool_names_empty_list_is_ok() {
        assert!(validate_tool_names(&[]).is_ok());
    }

    #[test]
    fn mvp_tool_specs_has_expected_count() {
        let specs = mvp_tool_specs();
        assert_eq!(specs.len(), 19);
    }

    #[test]
    fn mvp_tool_specs_names_are_unique() {
        let specs = mvp_tool_specs();
        let names: BTreeSet<&str> = specs.iter().map(|s| s.name).collect();
        assert_eq!(names.len(), 19);
    }

    #[test]
    fn mvp_tool_specs_each_has_schema() {
        for spec in &mvp_tool_specs() {
            assert!(!spec.name.is_empty());
            assert!(!spec.description.is_empty());
            assert!(spec.input_schema.is_object());
        }
    }

    #[test]
    fn jsonrpc_parse_valid_request() {
        let req: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "tools/list");
    }

    #[test]
    fn validate_tool_names_lists_all_valid_tools_on_error() {
        let names = vec!["bogus_tool".to_string()];
        let err = validate_tool_names(&names).unwrap_err();
        for expected in &[
            "navigate",
            "click",
            "fill_form",
            "screenshot",
            "go_back",
            "scroll",
            "wait",
            "select_option",
            "execute_js",
            "hover",
            "press_key",
            "switch_tab",
            "list_resources",
            "save_file",
            "page_map",
            "read_content",
            "fork",
            "wait_for_subagents",
            "wait_for_human",
        ] {
            assert!(
                err.contains(expected),
                "error should list `{expected}` in valid tools: {err}"
            );
        }
    }
}
