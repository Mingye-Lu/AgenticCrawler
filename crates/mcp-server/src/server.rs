use std::collections::BTreeSet;
use std::io::{self, BufRead, BufReader, Write};
use std::str::FromStr;
use std::sync::Mutex;

use acrawl_core::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage, MessageRole,
    RuntimeError, TokenUsage, ToolEffect, ToolSpec,
};
use agent::script_manager::ScriptManager;
use agent::{mvp_tool_specs, ToolRegistry};
use agent::{CrawlResult, CrawlerAgent};
use api::provider::{model_api_id, ProviderClient, ProviderRegistry};
use api::{
    ContentBlockDelta, ContentBlockDeltaEvent, InputContentBlock, InputMessage, MessageRequest,
    StreamEvent,
};
use api::{OutputContentBlock, ToolChoice, ToolDefinition};
use browser::{BrowserBackend, BrowserContext, PlaywrightBridge};
use runtime::{encode_mcp_frame, read_mcp_frame};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

static JOB_MUTEX: Mutex<()> = Mutex::new(());
static OUTPUT_MODE: Mutex<TransportMode> = Mutex::new(TransportMode::Framed);

const SERVER_NAME: &str = "acrawl-mcp-server";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2024-11-05";

const EXCLUDED_TOOLS: &[&str] = &[
    "fork",
    "wait_for_subagents",
    "cancel_subagent",
    "subagent_status",
];

const SCRIPT_TOOLS: &[&str] = &[
    "run_script",
    "script_status",
    "wait_for_scripts",
    "cancel_script",
    "save_script",
    "list_scripts",
    "read_script",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportMode {
    Framed,
    LineDelimited,
}

fn set_output_mode(mode: TransportMode) {
    match OUTPUT_MODE.lock() {
        Ok(mut guard) => *guard = mode,
        Err(poisoned) => *poisoned.into_inner() = mode,
    }
}

fn output_mode() -> TransportMode {
    match OUTPUT_MODE.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn read_json_line(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = String::new();
    let bytes_read = reader.read_line(&mut line)?;
    if bytes_read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "MCP stdio stream closed while reading line-delimited message",
        ));
    }
    Ok(line.trim_end_matches(['\r', '\n']).as_bytes().to_vec())
}

fn read_protocol_message(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    // Consume any leading whitespace before detecting the transport mode.
    // A slow pipe may deliver whitespace bytes in a separate read before
    // the real first byte of the message arrives.
    let first = loop {
        let buffered = reader.fill_buf()?;
        if buffered.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "MCP stdio stream closed before first message",
            ));
        }
        if let Some(pos) = buffered.iter().position(|b| !b.is_ascii_whitespace()) {
            reader.consume(pos);
            let after = reader.fill_buf()?;
            break after[0];
        }
        let len = buffered.len();
        reader.consume(len);
    };

    let mode = match first {
        b'{' | b'[' => TransportMode::LineDelimited,
        _ => TransportMode::Framed,
    };
    set_output_mode(mode);

    match mode {
        TransportMode::Framed => read_mcp_frame(reader),
        TransportMode::LineDelimited => read_json_line(reader),
    }
}

fn write_frame_to_stdout(payload: &[u8]) {
    let mut stdout = io::stdout().lock();
    match output_mode() {
        TransportMode::Framed => {
            let framed = encode_mcp_frame(payload);
            let _ = stdout.write_all(&framed);
        }
        TransportMode::LineDelimited => {
            let _ = stdout.write_all(payload);
            let _ = stdout.write_all(b"\n");
        }
    }
    let _ = stdout.flush();
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

fn send_response(response: &JsonRpcResponse) {
    let json = serde_json::to_vec(response).unwrap_or_else(|_| {
        br#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal serialization error"}}"#
            .to_vec()
    });
    write_frame_to_stdout(&json);
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

fn run_goal_tool_schema() -> Value {
    json!({
        "name": "run_goal",
        "description": "Execute a high-level crawl goal autonomously. The agent plans, navigates, and extracts data using its own LLM loop. Returns structured results when done. Use this for complex multi-page tasks; use individual tools (navigate, click, etc.) for fine-grained control.",
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
                    "description": "Restrict which built-in tools the agent can use (optional)"
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
    })
}

fn tools_list_response(id: Option<Value>) {
    let mut tools: Vec<Value> = mvp_tool_specs()
        .into_iter()
        .filter(|spec| !EXCLUDED_TOOLS.contains(&spec.name))
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": spec.input_schema,
            })
        })
        .collect();

    tools.push(run_goal_tool_schema());

    send_response(&JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(json!({ "tools": tools })),
        error: None,
    });
}

fn execute_browser_tool(
    name: &str,
    input: &Value,
    registry: &ToolRegistry,
    browser: &mut BrowserContext,
    rt: &tokio::runtime::Runtime,
    crawl_state: &mut agent::state::CrawlState,
) -> Result<String, String> {
    // MCP transport timeout: reject selector-less waits over 45s.
    // The cap is enforced here (MCP-only path), not in parse_input()
    // which is shared with TUI/prompt sessions where the 300s limit applies.
    const MAX_MCP_TIME_ONLY_MS: u64 = 45_000;
    if name == "wait" {
        let selector = input.get("selector").and_then(|v| v.as_str());
        if selector.is_none() {
            let timeout_ms = if let Some(ms) = input.get("timeout_ms").and_then(serde_json::Value::as_u64) {
                ms
            } else if let Some(sec) = input.get("seconds").and_then(serde_json::Value::as_f64) {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let ms = (sec * 1000.0) as u64;
                ms
            } else {
                5_000 // default
            };
            if timeout_ms > MAX_MCP_TIME_ONLY_MS {
                return Err(format!(
                    "wait without a selector is limited to {}s when used over MCP. \
                     For longer delays, chain multiple wait calls \
                     (e.g., wait(seconds=30) then wait(seconds=30)). \
                     Use wait with a selector for longer timeouts.",
                    MAX_MCP_TIME_ONLY_MS / 1000
                ));
            }
        }
    }

    rt.block_on(async {
        match registry
            .execute_async(name, input, browser, crawl_state)
            .await
        {
            Ok(ToolEffect::Reply(output)) => Ok(output),
            Ok(_) => Err(format!("tool `{name}` returned unsupported effect")),
            Err(e) => Err(e.to_string()),
        }
    })
}

fn execute_script_tool(
    name: &str,
    input: &Value,
    script_manager: &mut ScriptManager,
    browser: &mut Option<BrowserContext>,
    rt: &tokio::runtime::Runtime,
) -> Result<String, String> {
    match name {
        "save_script" => match agent::tools::save_script::execute(input) {
            Ok(ToolEffect::Reply(output)) => Ok(output),
            Ok(_) => Err("save_script returned unexpected effect".to_string()),
            Err(e) => Err(e.to_string()),
        },
        "list_scripts" => match agent::tools::list_scripts::execute(input) {
            Ok(ToolEffect::Reply(output)) => Ok(output),
            Ok(_) => Err("list_scripts returned unexpected effect".to_string()),
            Err(e) => Err(e.to_string()),
        },
        "read_script" => match agent::tools::read_script::execute(input) {
            Ok(ToolEffect::Reply(output)) => Ok(output),
            Ok(_) => Err("read_script returned unexpected effect".to_string()),
            Err(e) => Err(e.to_string()),
        },
        "run_script" => {
            let task = match agent::tools::run_script::execute(input) {
                Ok(ToolEffect::RunScript(task)) => task,
                Ok(_) => return Err("run_script returned unexpected effect".to_string()),
                Err(e) => return Err(e.to_string()),
            };

            // Ensure browser is initialized for script execution
            if browser.is_none() {
                match rt.block_on(PlaywrightBridge::new()) {
                    Ok(bridge) => {
                        let shared = std::sync::Arc::new(tokio::sync::Mutex::new(
                            Box::new(bridge) as Box<dyn BrowserBackend + Send>
                        ));
                        *browser = Some(BrowserContext::new(shared));
                    }
                    Err(e) => {
                        return Err(format!("failed to launch browser for script: {e}"));
                    }
                }
            }

            let browser_ctx = browser.as_ref().unwrap().clone();
            match rt.block_on(async { script_manager.spawn_script(task, browser_ctx) }) {
                Ok(script_id) => Ok(json!({"script_id": script_id}).to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
        "script_status" => {
            let spec = match agent::tools::script_status::execute(input) {
                Ok(ToolEffect::ScriptStatus(spec)) => spec,
                Ok(_) => return Err("script_status returned unexpected effect".to_string()),
                Err(e) => return Err(e.to_string()),
            };
            match script_manager.get_status(&spec.script_id) {
                Ok(state) => serde_json::to_string(&state)
                    .map_err(|e| format!("failed to serialize script state: {e}")),
                Err(e) => Err(e.to_string()),
            }
        }
        "wait_for_scripts" => {
            let spec = match agent::tools::wait_for_scripts::execute(input) {
                Ok(ToolEffect::ScriptWait(spec)) => spec,
                Ok(_) => return Err("wait_for_scripts returned unexpected effect".to_string()),
                Err(e) => return Err(e.to_string()),
            };
            rt.block_on(async {
                match script_manager.wait_for_scripts(spec.script_ids).await {
                    Ok(results) => serde_json::to_string(&results)
                        .map_err(|e| format!("failed to serialize script results: {e}")),
                    Err(e) => Err(e.to_string()),
                }
            })
        }
        "cancel_script" => {
            let spec = match agent::tools::cancel_script::execute(input) {
                Ok(ToolEffect::ScriptCancel(spec)) => spec,
                Ok(_) => return Err("cancel_script returned unexpected effect".to_string()),
                Err(e) => return Err(e.to_string()),
            };
            match script_manager.cancel_script(&spec.script_id) {
                Ok(()) => Ok(format!("Script '{}' cancelled", spec.script_id)),
                Err(e) => Err(e.to_string()),
            }
        }
        _ => Err(format!("unknown script tool: {name}")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunGoalRequest {
    pub goal: String,
    pub model: String,
    pub allowed_tools: Vec<String>,
    pub max_steps: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunGoalExecutionError {
    Internal(String),
    Crawl(String),
}

#[derive(Debug, Clone, PartialEq)]
enum RunGoalOutcome {
    ToolResult(Value),
    JsonRpcError { code: i32, message: String },
}

pub trait GoalExecutor {
    fn execute(&self, request: &RunGoalRequest) -> Result<CrawlResult, RunGoalExecutionError>;
}

pub struct RealGoalExecutor;

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
        let normalized = normalize_tool_name(name);
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

fn normalize_tool_name(name: &str) -> String {
    name.replace('-', "_").to_lowercase()
}

fn normalize_tool_names(names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|name| normalize_tool_name(name))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn filtered_tool_specs(allowed_tools: &[String]) -> Vec<ToolSpec> {
    let allowed: BTreeSet<&str> = allowed_tools.iter().map(String::as_str).collect();
    mvp_tool_specs()
        .into_iter()
        .filter(|spec| allowed.is_empty() || allowed.contains(spec.name))
        .collect()
}

fn build_run_goal_system_prompt(allowed_tools: &[String]) -> Vec<String> {
    agent::build_system_prompt(&filtered_tool_specs(allowed_tools), None)
}

fn parse_run_goal_request(arguments: &Value) -> Result<RunGoalRequest, RunGoalOutcome> {
    let goal = arguments
        .get("goal")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    if goal.is_empty() {
        return Err(RunGoalOutcome::JsonRpcError {
            code: -32602,
            message: "missing required parameter: goal".to_string(),
        });
    }
    if goal.len() > 100_000 {
        return Err(RunGoalOutcome::JsonRpcError {
            code: -32602,
            message: "goal exceeds maximum length (100,000 characters)".to_string(),
        });
    }

    let model = resolve_model(arguments.get("model").and_then(Value::as_str)).map_err(|error| {
        RunGoalOutcome::JsonRpcError {
            code: -32602,
            message: error,
        }
    })?;

    let raw_allowed_tools: Vec<String> = arguments
        .get("allowed_tools")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    validate_tool_names(&raw_allowed_tools).map_err(|error| RunGoalOutcome::JsonRpcError {
        code: -32602,
        message: error,
    })?;
    let allowed_tools = normalize_tool_names(&raw_allowed_tools);

    let max_steps = if let Some(raw) = arguments.get("max_steps").and_then(Value::as_u64) {
        if !(1..=200).contains(&raw) {
            return Err(RunGoalOutcome::JsonRpcError {
                code: -32602,
                message: format!("max_steps must be between 1 and 200, got {raw}"),
            });
        }
        #[allow(clippy::cast_possible_truncation)]
        Some(raw as usize)
    } else {
        None
    };

    Ok(RunGoalRequest {
        goal: goal.to_string(),
        model,
        allowed_tools,
        max_steps,
    })
}

fn render_text_with_json(summary: &str, payload: &Value) -> String {
    let pretty = serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string());
    format!("{summary}\n\nStructured result:\n```json\n{pretty}\n```")
}

fn build_run_goal_success_response(request: &RunGoalRequest, result: &CrawlResult) -> Value {
    let structured = json!({
        "summary": result.summary,
        "extracted_data": result.extracted_data,
        "steps_executed": result.steps_executed,
        "model_used": request.model,
        "allowed_tools": request.allowed_tools,
        "goal": request.goal,
    });
    json!({
        "content": [{
            "type": "text",
            "text": render_text_with_json(
                &format!("Crawl completed in {} steps.\n\n{}", result.steps_executed, result.summary),
                &structured,
            )
        }],
        "structuredContent": structured,
        "isError": false,
    })
}

fn build_run_goal_failure_response(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": format!("Crawl failed: {message}") }],
        "isError": true,
    })
}

fn execute_run_goal<E: GoalExecutor>(executor: &E, arguments: &Value) -> RunGoalOutcome {
    let request = match parse_run_goal_request(arguments) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };

    match executor.execute(&request) {
        Ok(result) => {
            RunGoalOutcome::ToolResult(build_run_goal_success_response(&request, &result))
        }
        Err(RunGoalExecutionError::Internal(message)) => RunGoalOutcome::JsonRpcError {
            code: -32603,
            message,
        },
        Err(RunGoalExecutionError::Crawl(message)) => {
            RunGoalOutcome::ToolResult(build_run_goal_failure_response(&message))
        }
    }
}

struct CrawlApiClient {
    provider: ProviderClient,
    model: String,
    tool_names: Vec<String>,
    max_tokens: u32,
    reasoning_effort: Option<api::ReasoningEffort>,
}

impl CrawlApiClient {
    fn new(provider: ProviderClient, model: &str, tool_names: Vec<String>) -> Self {
        let store = api::load_credentials().unwrap_or_default();
        let registry = ProviderRegistry::from_credentials(&store);
        let max_tokens = registry.max_tokens(model);
        let reasoning_effort = reasoning_effort_for_model(model, &registry);
        Self {
            provider,
            model: model.to_string(),
            tool_names,
            max_tokens,
            reasoning_effort,
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
                            output,
                            is_error,
                            ..
                        } => InputContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: vec![api::ToolResultContentBlock::Text {
                                text: output.clone(),
                            }],
                            is_error: *is_error,
                        },
                        ContentBlock::Reasoning { data } => {
                            let parsed: Value =
                                serde_json::from_str(data).unwrap_or(json!({"raw": data}));
                            InputContentBlock::Reasoning { data: parsed }
                        }
                        ContentBlock::ToolResultImage {
                            tool_use_id,
                            media_type,
                            base64_data,
                            caption,
                            is_error,
                            ..
                        } => InputContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: vec![
                                api::ToolResultContentBlock::Image {
                                    source: api::ImageSource {
                                        source_type: "base64".to_string(),
                                        media_type: media_type.clone(),
                                        data: base64_data.clone(),
                                    },
                                },
                                api::ToolResultContentBlock::Text {
                                    text: caption.clone(),
                                },
                            ],
                            is_error: *is_error,
                        },
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

fn reasoning_effort_for_model(
    model: &str,
    registry: &ProviderRegistry,
) -> Option<api::ReasoningEffort> {
    if !registry
        .resolve_model(model)
        .is_some_and(|info| info.capabilities.reasoning)
    {
        return None;
    }

    runtime::load_settings()
        .reasoning_effort
        .as_deref()
        .and_then(|value| api::ReasoningEffort::from_str(value).ok())
        .or(Some(api::ReasoningEffort::High))
}

fn reasoning_event_payload(thinking: &str) -> Option<String> {
    if thinking.is_empty() {
        None
    } else {
        Some(json!({"reasoning_content": thinking}).to_string())
    }
}

fn push_output_block(
    block: OutputContentBlock,
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
) {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            let initial_input =
                if input.is_object() && input.as_object().is_some_and(serde_json::Map::is_empty) {
                    String::new()
                } else {
                    input.to_string()
                };
            *pending_tool = Some((id, name, initial_input));
        }
        OutputContentBlock::Reasoning => {}
    }
}

impl ApiClient for CrawlApiClient {
    #[allow(clippy::too_many_lines)]
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
            reasoning_effort: self.reasoning_effort,
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
                let mut saw_stop = false;

                loop {
                    let event = stream
                        .next_event()
                        .await
                        .map_err(|e: api::ApiError| RuntimeError::new(e.to_string()))?;
                    match event {
                        Some(StreamEvent::MessageStart(start)) => {
                            for block in start.message.content {
                                push_output_block(block, &mut events, &mut pending_tool);
                            }
                        }
                        Some(StreamEvent::ContentBlockStart(start)) => {
                            if matches!(start.content_block, OutputContentBlock::Reasoning) {
                                pending_reasoning = Some(String::new());
                            } else {
                                push_output_block(
                                    start.content_block,
                                    &mut events,
                                    &mut pending_tool,
                                );
                            }
                        }
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
                                let input = if input.is_empty() {
                                    "{}".to_string()
                                } else {
                                    input
                                };
                                events.push(AssistantEvent::ToolUse { id, name, input });
                            }
                            if let Some(data) = pending_reasoning.take() {
                                if let Some(data) = reasoning_event_payload(&data) {
                                    events.push(AssistantEvent::Reasoning { data });
                                }
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
                        Some(StreamEvent::MessageStop(_)) => {
                            saw_stop = true;
                            events.push(AssistantEvent::MessageStop);
                            break;
                        }
                        None => break,
                    }
                }

                if !saw_stop
                    && events.iter().any(|event| {
                        matches!(event, AssistantEvent::TextDelta(text) if !text.is_empty())
                            || matches!(event, AssistantEvent::ToolUse { .. })
                    })
                {
                    events.push(AssistantEvent::MessageStop);
                }

                Ok(events)
            })
        })
    }
}

impl GoalExecutor for RealGoalExecutor {
    fn execute(&self, request: &RunGoalRequest) -> Result<CrawlResult, RunGoalExecutionError> {
        let provider = build_provider(&request.model).map_err(RunGoalExecutionError::Internal)?;
        let api_client =
            CrawlApiClient::new(provider, &request.model, request.allowed_tools.clone());
        let system_prompt = build_run_goal_system_prompt(&request.allowed_tools);

        let registry = ToolRegistry::new_with_core_tools();
        let mut agent = CrawlerAgent::new_lazy(registry).with_model_supports_vision(
            api::provider::catalog::model_supports_vision(&request.model),
        );
        if !request.allowed_tools.is_empty() {
            agent = agent.with_allowed_tools(request.allowed_tools.iter().cloned().collect());
        }
        if let Some(max_steps) = request.max_steps {
            agent = agent.with_max_steps(max_steps);
        }

        let _guard = match JOB_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                RunGoalExecutionError::Internal(format!("failed to create tokio runtime: {error}"))
            })?;

        runtime
            .block_on(agent.run_with_system_prompt(&request.goal, api_client, system_prompt))
            .map_err(|error| RunGoalExecutionError::Crawl(error.to_string()))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn handle_run_goal(id: Option<Value>, arguments: Value) {
    match execute_run_goal(&RealGoalExecutor, &arguments) {
        RunGoalOutcome::ToolResult(response) => send_response(&JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(response),
            error: None,
        }),
        RunGoalOutcome::JsonRpcError { code, message } => send_error(id, code, message),
    }
}

#[allow(clippy::too_many_lines)]
fn handle_tools_call(
    id: Option<Value>,
    params: Option<Value>,
    registry: &ToolRegistry,
    browser: &mut Option<BrowserContext>,
    script_manager: &mut ScriptManager,
    rt: &tokio::runtime::Runtime,
    crawl_state: &mut agent::state::CrawlState,
) {
    let Some(params) = params else {
        send_error(id, -32602, "missing params".to_string());
        return;
    };

    let Some(name) = params.get("name").and_then(Value::as_str) else {
        send_error(id, -32602, "missing tool name".to_string());
        return;
    };

    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    if name == "run_goal" {
        handle_run_goal(id, arguments);
        return;
    }

    if EXCLUDED_TOOLS.contains(&name) {
        send_error(
            id,
            -32601,
            format!("tool `{name}` is not available in MCP mode (agent-control only)"),
        );
        return;
    }

    let valid_browser_tools: BTreeSet<&str> = mvp_tool_specs()
        .iter()
        .filter(|spec| !EXCLUDED_TOOLS.contains(&spec.name))
        .map(|s| s.name)
        .collect();
    if !valid_browser_tools.contains(name) {
        send_error(id, -32601, format!("unknown tool: {name}"));
        return;
    }

    if SCRIPT_TOOLS.contains(&name) {
        match execute_script_tool(name, &arguments, script_manager, browser, rt) {
            Ok(output) => {
                let result = json!({
                    "content": [{ "type": "text", "text": output }],
                    "isError": false,
                });
                send_response(&JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(result),
                    error: None,
                });
            }
            Err(message) => {
                let result = json!({
                    "content": [{ "type": "text", "text": format!("Error: {message}") }],
                    "isError": true,
                });
                send_response(&JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(result),
                    error: None,
                });
            }
        }
        return;
    }

    if browser.is_none() {
        match rt.block_on(PlaywrightBridge::new()) {
            Ok(bridge) => {
                let shared = std::sync::Arc::new(tokio::sync::Mutex::new(
                    Box::new(bridge) as Box<dyn BrowserBackend + Send>
                ));
                *browser = Some(BrowserContext::new(shared));
            }
            Err(e) => {
                let result = json!({
                    "content": [{ "type": "text", "text": format!("Error: failed to launch browser — {e}") }],
                    "isError": true,
                });
                send_response(&JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(result),
                    error: None,
                });
                return;
            }
        }
    }

    match execute_browser_tool(
        name,
        &arguments,
        registry,
        browser.as_mut().unwrap(),
        rt,
        crawl_state,
    ) {
        Ok(output) => {
            let result = json!({
                "content": [{ "type": "text", "text": output }],
                "isError": false,
            });
            send_response(&JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(result),
                error: None,
            });
        }
        Err(message) => {
            let result = json!({
                "content": [{ "type": "text", "text": format!("Error: {message}") }],
                "isError": true,
            });
            send_response(&JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(result),
                error: None,
            });
        }
    }
}

pub fn run_mcp_server() {
    eprintln!("{SERVER_NAME} v{SERVER_VERSION} ready (stdio transport, waiting for JSON-RPC)");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    let mut browser: Option<BrowserContext> = None;
    let mut crawl_state = agent::state::CrawlState::default();
    let registry = ToolRegistry::new_with_core_tools();
    let settings = runtime::load_settings();
    let script_settings = settings.script.unwrap_or_default();
    let mut script_manager = ScriptManager::new(script_settings);

    let stdin = io::stdin().lock();
    let mut reader = BufReader::new(stdin);

    loop {
        let payload = match read_protocol_message(&mut reader) {
            Ok(p) => p,
            Err(e) => {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    break;
                }
                eprintln!("frame read error: {e}");
                break;
            }
        };

        let request: JsonRpcRequest = match serde_json::from_slice(&payload) {
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
            "tools/call" => {
                handle_tools_call(
                    request.id,
                    request.params,
                    &registry,
                    &mut browser,
                    &mut script_manager,
                    &rt,
                    &mut crawl_state,
                );
            }
            method => {
                send_error(request.id, -32601, format!("method not found: {method}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn with_transport_mode_lock<T>(f: impl FnOnce() -> T) -> T {
        let _guard = JOB_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_output_mode(TransportMode::Framed);
        f()
    }

    fn assert_jsonrpc_error(
        outcome: Result<RunGoalRequest, RunGoalOutcome>,
        expected_code: i32,
        expected_message: &str,
    ) {
        match outcome {
            Err(RunGoalOutcome::JsonRpcError { code, message }) => {
                assert_eq!(code, expected_code);
                assert!(
                    message.contains(expected_message),
                    "expected `{message}` to contain `{expected_message}`"
                );
            }
            other => panic!("expected JsonRpcError, got {other:?}"),
        }
    }

    #[test]
    fn parse_standard_content_length_frame() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let framed = encode_mcp_frame(body.as_bytes());
        let mut cursor = Cursor::new(&framed[..]);
        let parsed = read_mcp_frame(&mut cursor).expect("valid frame");
        assert_eq!(parsed, body.as_bytes());
    }

    #[test]
    fn read_protocol_message_accepts_json_line_mode() {
        with_transport_mode_lock(|| {
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
            let data = format!("{body}\n").into_bytes();
            let mut cursor = Cursor::new(data);
            let parsed =
                read_protocol_message(&mut cursor).expect("line-delimited request should parse");
            assert_eq!(parsed, body.as_bytes());
            assert_eq!(output_mode(), TransportMode::LineDelimited);
        });
    }

    #[test]
    fn read_protocol_message_skips_leading_whitespace_before_json() {
        with_transport_mode_lock(|| {
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
            let data = format!("\r\n  {body}\n").into_bytes();
            let mut cursor = Cursor::new(data);
            let parsed =
                read_protocol_message(&mut cursor).expect("leading whitespace should be drained");
            assert_eq!(parsed, body.as_bytes());
            assert_eq!(output_mode(), TransportMode::LineDelimited);
        });
    }

    #[test]
    fn read_protocol_message_accepts_framed_mode() {
        with_transport_mode_lock(|| {
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
            let framed = encode_mcp_frame(body.as_bytes());
            let mut cursor = Cursor::new(framed);
            let parsed = read_protocol_message(&mut cursor).expect("framed request should parse");
            assert_eq!(parsed, body.as_bytes());
            assert_eq!(output_mode(), TransportMode::Framed);
        });
    }

    #[test]
    fn encode_decode_round_trip() {
        let payloads: Vec<&[u8]> = vec![br#"{"hello":"world"}"#, b"", &[0u8; 100]];
        for payload in payloads {
            let framed = encode_mcp_frame(payload);
            let mut cursor = Cursor::new(&framed[..]);
            let decoded = read_mcp_frame(&mut cursor).expect("decode success");
            assert_eq!(decoded, payload, "round-trip failed");
        }
    }

    #[test]
    fn tools_list_has_38_nonexcluded_tools_plus_run_goal() {
        let browser_specs: Vec<_> = mvp_tool_specs()
            .into_iter()
            .filter(|spec| !EXCLUDED_TOOLS.contains(&spec.name))
            .collect();
        assert_eq!(browser_specs.len(), 38);
        let names: BTreeSet<&str> = browser_specs.iter().map(|s| s.name).collect();
        assert!(names.contains("navigate"));
        assert!(names.contains("click"));
        assert!(names.contains("screenshot"));
        assert!(names.contains("run_script"));
        assert!(names.contains("script_status"));
        assert!(names.contains("wait_for_scripts"));
        assert!(names.contains("cancel_script"));
        assert!(names.contains("save_script"));
        assert!(names.contains("list_scripts"));
        assert!(names.contains("read_script"));
        assert!(names.contains("set_device"));
        assert!(!names.contains("fork"));
        assert!(!names.contains("wait_for_subagents"));
    }

    #[test]
    fn excluded_tools_are_all_valid_tool_names() {
        let all_names: BTreeSet<&str> = mvp_tool_specs().iter().map(|s| s.name).collect();
        for &excluded in EXCLUDED_TOOLS {
            assert!(
                all_names.contains(excluded),
                "EXCLUDED_TOOLS contains `{excluded}` which is not a valid tool name"
            );
        }
    }

    #[test]
    fn validate_tool_names_accepts_valid_names() {
        let names = vec!["navigate".to_string(), "click".to_string()];
        assert!(validate_tool_names(&names).is_ok());
    }

    #[test]
    fn validate_tool_names_accepts_empty_list() {
        assert!(validate_tool_names(&[]).is_ok());
    }

    #[test]
    fn validate_tool_names_rejects_unknown_tool() {
        let names = vec!["nonexistent-tool".to_string()];
        let err = validate_tool_names(&names).unwrap_err();
        assert!(err.contains("unknown tool"));
        assert!(err.contains("nonexistent-tool"));
    }

    #[test]
    fn normalize_tool_name_replaces_dashes_and_lowercases() {
        assert_eq!(normalize_tool_name("Read-Content"), "read_content");
        assert_eq!(normalize_tool_name("SAVE_FILE"), "save_file");
    }

    #[test]
    fn normalize_tool_names_deduplicates_names() {
        let names = vec![
            "read-content".to_string(),
            "read_content".to_string(),
            "Read-Content".to_string(),
            "navigate".to_string(),
        ];

        assert_eq!(
            normalize_tool_names(&names),
            vec!["navigate", "read_content"]
        );
    }

    #[test]
    fn filtered_tool_specs_with_empty_allowed_list_returns_all_tools() {
        let filtered = filtered_tool_specs(&[]);

        assert_eq!(filtered.len(), mvp_tool_specs().len());
        assert_eq!(
            filtered.iter().map(|spec| spec.name).collect::<Vec<_>>(),
            mvp_tool_specs()
                .iter()
                .map(|spec| spec.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn filtered_tool_specs_with_specific_allowed_list_returns_subset() {
        let filtered = filtered_tool_specs(&["navigate".to_string(), "click".to_string()]);

        assert_eq!(
            filtered.iter().map(|spec| spec.name).collect::<Vec<_>>(),
            vec!["navigate", "click"]
        );
    }

    #[test]
    fn parse_run_goal_request_accepts_valid_input() {
        let request = parse_run_goal_request(&json!({
            "goal": "Collect product titles",
            "model": "anthropic/claude-sonnet-4-6",
            "allowed_tools": ["read-content", "navigate", "read_content"],
            "max_steps": 200
        }))
        .expect("request should parse");

        assert_eq!(request.goal, "Collect product titles");
        assert_eq!(request.model, "anthropic/claude-sonnet-4-6");
        assert_eq!(request.allowed_tools, vec!["navigate", "read_content"]);
        assert_eq!(request.max_steps, Some(200));
    }

    #[test]
    fn parse_run_goal_request_rejects_missing_goal() {
        assert_jsonrpc_error(
            parse_run_goal_request(&json!({})),
            -32602,
            "missing required parameter: goal",
        );
    }

    #[test]
    fn parse_run_goal_request_rejects_empty_goal() {
        assert_jsonrpc_error(
            parse_run_goal_request(&json!({"goal": "", "model": "x/y"})),
            -32602,
            "missing required parameter: goal",
        );
    }

    #[test]
    fn parse_run_goal_request_rejects_whitespace_only_goal() {
        assert_jsonrpc_error(
            parse_run_goal_request(&json!({"goal": "  ", "model": "x/y"})),
            -32602,
            "missing required parameter: goal",
        );
    }

    #[test]
    fn parse_run_goal_request_rejects_oversized_goal() {
        assert_jsonrpc_error(
            parse_run_goal_request(&json!({
                "goal": "x".repeat(100_001),
                "model": "anthropic/claude-sonnet-4-6"
            })),
            -32602,
            "goal exceeds maximum length",
        );
    }

    #[test]
    fn parse_run_goal_request_rejects_invalid_allowed_tools() {
        assert_jsonrpc_error(
            parse_run_goal_request(&json!({
                "goal": "Collect product titles",
                "model": "anthropic/claude-sonnet-4-6",
                "allowed_tools": ["totally-fake-tool"]
            })),
            -32602,
            "unknown tool `totally-fake-tool`",
        );
    }

    #[test]
    fn parse_run_goal_request_rejects_max_steps_below_range() {
        assert_jsonrpc_error(
            parse_run_goal_request(&json!({
                "goal": "Collect product titles",
                "model": "anthropic/claude-sonnet-4-6",
                "max_steps": 0
            })),
            -32602,
            "max_steps must be between 1 and 200, got 0",
        );
    }

    #[test]
    fn parse_run_goal_request_rejects_max_steps_above_range() {
        assert_jsonrpc_error(
            parse_run_goal_request(&json!({
                "goal": "Collect product titles",
                "model": "anthropic/claude-sonnet-4-6",
                "max_steps": 201
            })),
            -32602,
            "max_steps must be between 1 and 200, got 201",
        );
    }

    #[derive(Debug, Clone)]
    struct FakeGoalExecutor {
        result: Result<CrawlResult, RunGoalExecutionError>,
    }

    impl GoalExecutor for FakeGoalExecutor {
        fn execute(&self, _request: &RunGoalRequest) -> Result<CrawlResult, RunGoalExecutionError> {
            self.result.clone()
        }
    }

    #[test]
    fn execute_run_goal_success_returns_structured_content() {
        let executor = FakeGoalExecutor {
            result: Ok(CrawlResult {
                summary: "Finished crawl".to_string(),
                extracted_data: vec![json!({"title": "Example"})],
                steps_executed: 3,
                messages: Vec::new(),
                model: Some("anthropic/claude-sonnet-4-6".to_string()),
            }),
        };

        let outcome = execute_run_goal(
            &executor,
            &json!({"goal": "Collect titles", "model": "anthropic/claude-sonnet-4-6"}),
        );

        let RunGoalOutcome::ToolResult(result) = outcome else {
            panic!("expected tool result");
        };
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["steps_executed"], 3);
        assert_eq!(result["structuredContent"]["goal"], "Collect titles");
    }

    #[test]
    fn execute_run_goal_crawl_error_returns_tool_error_result() {
        let executor = FakeGoalExecutor {
            result: Err(RunGoalExecutionError::Crawl("blocked".to_string())),
        };

        let outcome = execute_run_goal(
            &executor,
            &json!({"goal": "Collect titles", "model": "anthropic/claude-sonnet-4-6"}),
        );

        let RunGoalOutcome::ToolResult(result) = outcome else {
            panic!("expected tool error result");
        };
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "Crawl failed: blocked");
    }

    #[test]
    fn execute_run_goal_internal_error_returns_jsonrpc_error() {
        let executor = FakeGoalExecutor {
            result: Err(RunGoalExecutionError::Internal(
                "provider exploded".to_string(),
            )),
        };

        let outcome = execute_run_goal(
            &executor,
            &json!({"goal": "Collect titles", "model": "anthropic/claude-sonnet-4-6"}),
        );

        assert_eq!(
            outcome,
            RunGoalOutcome::JsonRpcError {
                code: -32603,
                message: "provider exploded".to_string(),
            }
        );
    }

    #[test]
    fn build_run_goal_success_response_returns_expected_json_structure() {
        let request = RunGoalRequest {
            goal: "Collect titles".to_string(),
            model: "anthropic/claude-sonnet-4-6".to_string(),
            allowed_tools: vec!["navigate".to_string(), "read_content".to_string()],
            max_steps: Some(5),
        };
        let result = CrawlResult {
            summary: "Finished crawl".to_string(),
            extracted_data: vec![json!({"title": "Example"})],
            steps_executed: 3,
            messages: Vec::new(),
            model: Some("anthropic/claude-sonnet-4-6".to_string()),
        };

        let response = build_run_goal_success_response(&request, &result);

        assert_eq!(response["isError"], false);
        assert_eq!(response["content"][0]["type"], "text");
        assert!(response["content"][0]["text"]
            .as_str()
            .expect("text content")
            .contains("Crawl completed in 3 steps."));
        assert_eq!(response["structuredContent"]["summary"], "Finished crawl");
        assert_eq!(
            response["structuredContent"]["extracted_data"][0]["title"],
            "Example"
        );
        assert_eq!(response["structuredContent"]["steps_executed"], 3);
        assert_eq!(
            response["structuredContent"]["model_used"],
            "anthropic/claude-sonnet-4-6"
        );
        assert_eq!(
            response["structuredContent"]["allowed_tools"],
            json!(["navigate", "read_content"])
        );
        assert_eq!(response["structuredContent"]["goal"], "Collect titles");
    }

    #[test]
    fn build_run_goal_failure_response_returns_expected_error_structure() {
        let response = build_run_goal_failure_response("blocked");

        assert_eq!(
            response,
            json!({
                "content": [{"type": "text", "text": "Crawl failed: blocked"}],
                "isError": true,
            })
        );
    }

    #[test]
    fn jsonrpc_parse_valid_request() {
        let req: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "tools/list");
    }
}
