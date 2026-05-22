use std::collections::BTreeSet;
use std::io::{self, BufRead, BufReader, Write};
use std::str::FromStr;
use std::sync::Mutex;

use api::provider::{model_api_id, ProviderClient, ProviderRegistry};
use api::{
    ContentBlockDelta, ContentBlockDeltaEvent, InputContentBlock, InputMessage, MessageRequest,
    StreamEvent,
};
use api::{ImageSource, OutputContentBlock, ToolChoice, ToolDefinition};
use crawler::{mvp_tool_specs, CrawlerAgent, ToolRegistry};
use runtime::{encode_mcp_frame, read_mcp_frame};
use runtime::{ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage};
use runtime::{MessageRole, RuntimeError, TokenUsage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

static JOB_MUTEX: Mutex<()> = Mutex::new(());
static OUTPUT_MODE: Mutex<TransportMode> = Mutex::new(TransportMode::Framed);

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunGoalRequest {
    goal: String,
    model: String,
    allowed_tools: Vec<String>,
    max_steps: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RunGoalExecutionError {
    Internal(String),
    Crawl(String),
}

#[derive(Debug, Clone, PartialEq)]
enum RunGoalOutcome {
    ToolResult(Value),
    JsonRpcError { code: i32, message: String },
}

trait GoalExecutor {
    fn execute(
        &self,
        request: &RunGoalRequest,
    ) -> Result<crawler::CrawlResult, RunGoalExecutionError>;
}

struct RealGoalExecutor;

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
    let buffered = reader.fill_buf()?;
    if buffered.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "MCP stdio stream closed before first message",
        ));
    }

    let first = buffered
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty MCP message"))?;

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
                        "description": "Restrict which built-in tools the agent can use (optional; validated and enforced at prompt, model, and runtime layers)"
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
            "description": "List acrawl's built-in crawl tool capabilities (read-only metadata). Returns names, descriptions, and input schemas for the 21 internal tools.",
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

fn filtered_tool_specs(allowed_tools: &[String]) -> Vec<crawler::ToolSpec> {
    let allowed: BTreeSet<&str> = allowed_tools.iter().map(String::as_str).collect();
    mvp_tool_specs()
        .into_iter()
        .filter(|spec| allowed.is_empty() || allowed.contains(spec.name))
        .collect()
}

fn build_run_goal_system_prompt(allowed_tools: &[String]) -> Vec<String> {
    crawler::build_system_prompt(&filtered_tool_specs(allowed_tools))
}

fn parse_run_goal_request(arguments: &Value) -> Result<RunGoalRequest, RunGoalOutcome> {
    let Some(goal) = arguments.get("goal").and_then(Value::as_str) else {
        return Err(RunGoalOutcome::JsonRpcError {
            code: -32602,
            message: "missing required parameter: goal".to_string(),
        });
    };

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

    let max_steps = arguments
        .get("max_steps")
        .and_then(Value::as_u64)
        .map(|value| value.min(200) as usize);
    if let Some(max_steps) = max_steps {
        if !(1..=200).contains(&max_steps) {
            return Err(RunGoalOutcome::JsonRpcError {
                code: -32602,
                message: format!("max_steps must be between 1 and 200, got {max_steps}"),
            });
        }
    }

    Ok(RunGoalRequest {
        goal: goal.to_string(),
        model,
        allowed_tools,
        max_steps,
    })
}

fn build_run_goal_success_response(
    request: &RunGoalRequest,
    result: &crawler::CrawlResult,
) -> Value {
    let structured = json!({
        "summary": result.summary,
        "extracted_data": result.extracted_data,
        "steps_executed": result.steps_executed,
        "model_used": request.model,
        "allowed_tools": request.allowed_tools,
        "goal": request.goal,
    });
    json!({
        "content": [
            {
                "type": "text",
                "text": render_text_with_json(
                    &format!(
                        "Crawl completed in {} steps.\n\n{}",
                        result.steps_executed,
                        result.summary
                    ),
                    &structured,
                )
            }
        ],
        "structuredContent": structured,
        "isError": false,
    })
}

fn build_run_goal_failure_response(message: &str) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": format!("Crawl failed: {message}")
            }
        ],
        "isError": true,
    })
}

fn render_text_with_json(summary: &str, payload: &Value) -> String {
    let pretty = serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string());
    format!("{summary}\n\nStructured result:\n```json\n{pretty}\n```")
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
        Some(
            serde_json::json!({
                "reasoning_content": thinking,
            })
            .to_string(),
        )
    }
}

fn push_output_block(
    block: OutputContentBlock,
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
    streaming_tool_input: bool,
) {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
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
                                push_output_block(block, &mut events, &mut pending_tool, true);
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
                                    true,
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
    fn execute(
        &self,
        request: &RunGoalRequest,
    ) -> Result<crawler::CrawlResult, RunGoalExecutionError> {
        let provider = build_provider(&request.model).map_err(RunGoalExecutionError::Internal)?;
        let api_client =
            CrawlApiClient::new(provider, &request.model, request.allowed_tools.clone());
        let system_prompt = build_run_goal_system_prompt(&request.allowed_tools);

        let registry = ToolRegistry::new_with_options(false);
        let mut agent = CrawlerAgent::new_lazy(registry);
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

    let structured = json!({
        "tool_count": tools.len(),
        "tools": tools,
    });

    let result = json!({
        "content": [
            {
                "type": "text",
                "text": render_text_with_json(
                    &format!(
                        "acrawl provides {} built-in crawl tools (informational only - not registered as callable MCP tools).",
                        structured["tool_count"].as_u64().unwrap_or(0)
                    ),
                    &structured,
                )
            }
        ],
        "structuredContent": structured,
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

/// Entry point for the `acrawl mcp` subcommand.
///
/// Runs the MCP server over stdio, reading JSON-RPC requests from stdin and
/// writing responses to stdout. Blocks until stdin is closed.
pub fn run() {
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
    use std::io::Cursor;

    #[derive(Debug, Clone)]
    struct FakeGoalExecutor {
        result: Result<crawler::CrawlResult, RunGoalExecutionError>,
    }

    impl GoalExecutor for FakeGoalExecutor {
        fn execute(
            &self,
            _request: &RunGoalRequest,
        ) -> Result<crawler::CrawlResult, RunGoalExecutionError> {
            self.result.clone()
        }
    }

    fn framed_request(body: &str) -> Vec<u8> {
        encode_mcp_frame(body.as_bytes())
    }

    fn json_line_request(body: &str) -> Vec<u8> {
        format!("{body}\n").into_bytes()
    }

    fn read_framed_response(data: &[u8]) -> Vec<u8> {
        let mut cursor = Cursor::new(data);
        read_mcp_frame(&mut cursor).expect("valid frame")
    }

    // --- framing parse tests ---

    #[test]
    fn parse_standard_content_length_frame() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let framed = framed_request(body);
        let parsed = read_framed_response(&framed);
        assert_eq!(parsed, body.as_bytes());
    }

    #[test]
    fn parse_empty_body_frame() {
        let framed = framed_request("");
        let parsed = read_framed_response(&framed);
        assert_eq!(parsed, b"");
    }

    #[test]
    fn read_protocol_message_accepts_json_line_mode() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let mut cursor = Cursor::new(json_line_request(body));
        let parsed =
            read_protocol_message(&mut cursor).expect("line-delimited request should parse");
        assert_eq!(parsed, body.as_bytes());
        assert_eq!(output_mode(), TransportMode::LineDelimited);
    }

    #[test]
    fn parse_missing_content_length_header() {
        let data = b"no-header-here\r\n\r\nbody";
        let mut cursor = Cursor::new(&data[..]);
        let err = read_mcp_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("missing Content-Length"));
    }

    #[test]
    fn parse_oversized_frame_rejected() {
        let header = b"Content-Length: 99999999\r\n\r\n";
        let mut data = header.to_vec();
        data.extend(vec![0u8; 100]);
        let mut cursor = Cursor::new(&data[..]);
        let err = read_mcp_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn parse_eof_during_headers_errors() {
        let data = b"Content-Length: 10";
        let mut cursor = Cursor::new(&data[..]);
        let err = read_mcp_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn parse_eof_during_body_errors() {
        let header = b"Content-Length: 10\r\n\r\n";
        let mut data = header.to_vec();
        data.extend(b"short");
        let mut cursor = Cursor::new(&data[..]);
        let err = read_mcp_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn two_consecutive_framed_requests_parse_independently() {
        let body1 = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let body2 = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_builtin_tools"}}"#;
        let mut combined = framed_request(body1);
        combined.extend(framed_request(body2));

        let mut cursor = Cursor::new(&combined[..]);
        let parsed1 = read_mcp_frame(&mut cursor).expect("first frame");
        let parsed2 = read_mcp_frame(&mut cursor).expect("second frame");
        assert_eq!(parsed1, body1.as_bytes());
        assert_eq!(parsed2, body2.as_bytes());
    }

    // --- framing output tests ---

    #[test]
    fn encode_frame_produces_content_length_header() {
        let payload = br#"{"key":"value"}"#;
        let framed = encode_mcp_frame(payload);
        let framed_str = String::from_utf8(framed).expect("valid utf8");
        assert!(framed_str.starts_with(&format!("Content-Length: {}\r\n\r\n", payload.len())));
        assert!(framed_str.ends_with(r#"{"key":"value"}"#));
    }

    #[test]
    fn encode_decode_round_trip() {
        let payloads: Vec<&[u8]> = vec![
            br#"{"hello":"world"}"#,
            b"",
            b"simple text without json",
            &[0u8; 100],
        ];
        for payload in payloads {
            let framed = encode_mcp_frame(payload);
            let mut cursor = Cursor::new(&framed[..]);
            let decoded = read_mcp_frame(&mut cursor).expect("decode success");
            assert_eq!(decoded, payload, "round-trip failed");
        }
    }

    // --- behavior tests (no external deps) ---

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
        assert_eq!(specs.len(), 21);
    }

    #[test]
    fn mvp_tool_specs_names_are_unique() {
        let specs = mvp_tool_specs();
        let names: BTreeSet<&str> = specs.iter().map(|s| s.name).collect();
        assert_eq!(names.len(), 21);
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

    #[test]
    fn parse_run_goal_request_normalizes_allowed_tools() {
        let request = parse_run_goal_request(&json!({
            "goal": "Collect product titles",
            "model": "anthropic/claude-sonnet-4-6",
            "allowed_tools": ["read-content", "navigate", "read_content"],
            "max_steps": 7
        }))
        .expect("request should parse");

        assert_eq!(request.goal, "Collect product titles");
        assert_eq!(request.model, "anthropic/claude-sonnet-4-6");
        assert_eq!(request.allowed_tools, vec!["navigate", "read_content"]);
        assert_eq!(request.max_steps, Some(7));
    }

    #[test]
    fn execute_run_goal_success_returns_structured_content() {
        let executor = FakeGoalExecutor {
            result: Ok(crawler::CrawlResult {
                summary: "Finished crawl".to_string(),
                extracted_data: vec![json!({"title": "Example"})],
                steps_executed: 3,
            }),
        };

        let outcome = execute_run_goal(
            &executor,
            &json!({
                "goal": "Collect titles",
                "model": "anthropic/claude-sonnet-4-6",
                "allowed_tools": ["navigate", "read-content"],
                "max_steps": 5
            }),
        );

        let RunGoalOutcome::ToolResult(result) = outcome else {
            panic!("expected tool result");
        };
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["summary"], "Finished crawl");
        assert_eq!(result["structuredContent"]["steps_executed"], 3);
        assert_eq!(result["structuredContent"]["goal"], "Collect titles");
        assert_eq!(
            result["structuredContent"]["allowed_tools"],
            json!(["navigate", "read_content"])
        );
        assert!(result["content"][0]["text"]
            .as_str()
            .expect("text payload")
            .contains("Structured result:"));
        assert!(result["content"][0]["text"]
            .as_str()
            .expect("text payload")
            .contains("\"extracted_data\""));
    }

    #[test]
    fn execute_run_goal_internal_error_returns_jsonrpc_error() {
        let executor = FakeGoalExecutor {
            result: Err(RunGoalExecutionError::Internal(
                "provider setup failed".to_string(),
            )),
        };

        let outcome = execute_run_goal(
            &executor,
            &json!({
                "goal": "Collect titles",
                "model": "anthropic/claude-sonnet-4-6"
            }),
        );

        assert_eq!(
            outcome,
            RunGoalOutcome::JsonRpcError {
                code: -32603,
                message: "provider setup failed".to_string(),
            }
        );
    }

    #[test]
    fn execute_run_goal_crawl_error_returns_tool_error_result() {
        let executor = FakeGoalExecutor {
            result: Err(RunGoalExecutionError::Crawl(
                "blocked by login wall".to_string(),
            )),
        };

        let outcome = execute_run_goal(
            &executor,
            &json!({
                "goal": "Collect titles",
                "model": "anthropic/claude-sonnet-4-6"
            }),
        );

        let RunGoalOutcome::ToolResult(result) = outcome else {
            panic!("expected tool error result");
        };
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .expect("error text")
            .contains("blocked by login wall"));
    }

    #[test]
    fn build_run_goal_system_prompt_filters_tool_listing() {
        let prompt = build_run_goal_system_prompt(&["navigate".to_string()]);
        let first_section = prompt.first().expect("prompt should have first section");
        assert!(first_section.contains("**navigate**"));
        assert!(!first_section.contains("**click**"));
    }

    #[test]
    fn render_text_with_json_embeds_pretty_payload() {
        let rendered = render_text_with_json("Summary line", &json!({"key": 1}));
        assert!(rendered.contains("Summary line"));
        assert!(rendered.contains("Structured result:"));
        assert!(rendered.contains("```json"));
        assert!(rendered.contains("\"key\": 1"));
    }

    #[test]
    fn reasoning_event_payload_wraps_reasoning_content() {
        let payload = reasoning_event_payload("chain of thought").expect("payload expected");
        assert_eq!(
            serde_json::from_str::<Value>(&payload).expect("valid reasoning payload"),
            json!({"reasoning_content": "chain of thought"})
        );
        assert_eq!(reasoning_event_payload(""), None);
    }

    // --- env init tests ---

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn headless_env_is_set_from_settings_when_not_present() {
        let _guard = env_lock();
        let temp_dir = std::env::temp_dir().join(format!("acrawl-mcp-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let saved_home = std::env::var_os("ACRAWL_CONFIG_HOME");
        let saved_headless = std::env::var("HEADLESS").ok();
        std::env::remove_var("HEADLESS");
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        runtime::update_settings(|s| {
            s.headless = Some(true);
        })
        .expect("update settings");

        let settings = runtime::load_settings();
        if std::env::var("HEADLESS").is_err() {
            std::env::set_var(
                "HEADLESS",
                if runtime::settings_get_headless(&settings) {
                    "true"
                } else {
                    "false"
                },
            );
        }
        assert_eq!(std::env::var("HEADLESS").unwrap(), "true");

        let _ = std::fs::remove_dir_all(&temp_dir);
        if let Some(h) = saved_home {
            std::env::set_var("ACRAWL_CONFIG_HOME", h);
        } else {
            std::env::remove_var("ACRAWL_CONFIG_HOME");
        }
        if let Some(h) = saved_headless {
            std::env::set_var("HEADLESS", h);
        } else {
            std::env::remove_var("HEADLESS");
        }
    }

    #[test]
    fn headless_env_not_overwritten_when_already_set() {
        let _guard = env_lock();
        let temp_dir =
            std::env::temp_dir().join(format!("acrawl-mcp-overwrite-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let saved_home = std::env::var_os("ACRAWL_CONFIG_HOME");
        let saved_headless = std::env::var("HEADLESS").ok();

        std::env::set_var("HEADLESS", "overridden-by-parent");
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        runtime::update_settings(|s| {
            s.headless = Some(true);
        })
        .expect("update settings");

        let settings = runtime::load_settings();
        if std::env::var("HEADLESS").is_err() {
            std::env::set_var(
                "HEADLESS",
                if runtime::settings_get_headless(&settings) {
                    "true"
                } else {
                    "false"
                },
            );
        }
        assert_eq!(std::env::var("HEADLESS").unwrap(), "overridden-by-parent");

        let _ = std::fs::remove_dir_all(&temp_dir);
        if let Some(h) = saved_home {
            std::env::set_var("ACRAWL_CONFIG_HOME", h);
        } else {
            std::env::remove_var("ACRAWL_CONFIG_HOME");
        }
        if let Some(h) = saved_headless {
            std::env::set_var("HEADLESS", h);
        } else {
            std::env::remove_var("HEADLESS");
        }
    }
}
