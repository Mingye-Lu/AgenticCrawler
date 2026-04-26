//! `OpenAI`-compatible Chat Completions API client with SSE streaming.
//!
//! [`ChatCompletionsClient`] works with any endpoint that speaks the
//! `/v1/chat/completions` SSE protocol — `OpenAI`, Azure, Ollama, LM Studio,
//! vLLM, etc.  Auth is optional (`None` skips the `Authorization` header)
//! and the base URL is caller-provided.
//!
//! Maps streamed SSE chunks to the shared [`StreamEvent`] enum so callers can
//! consume any provider uniformly.

use std::collections::{HashMap, VecDeque};

use serde::Deserialize;
use serde_json::Value;

use crate::client::{default_http_client, AuthSource};
use crate::error::ApiError;
use crate::provider::transform::{NoOpTransform, ProviderTransform};
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolResultContentBlock, Usage,
};

pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

#[derive(Debug, Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiChunkChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChunkChoice {
    #[serde(default)]
    delta: OpenAiDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiDelta {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

pub struct ChatCompletionsClient {
    http: reqwest::Client,
    auth: Option<AuthSource>,
    pub base_url: String,
    pub chat_path: String,
    default_model: String,
    #[allow(clippy::box_collection)]
    transform: Box<dyn ProviderTransform>,
    pub extra_headers: Vec<(String, String)>,
}

impl Clone for ChatCompletionsClient {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            auth: self.auth.clone(),
            base_url: self.base_url.clone(),
            chat_path: self.chat_path.clone(),
            default_model: self.default_model.clone(),
            transform: self.transform.clone_boxed(),
            extra_headers: self.extra_headers.clone(),
        }
    }
}

/// Backwards-compatible alias.
pub type OpenAiClient = ChatCompletionsClient;

impl ChatCompletionsClient {
    #[must_use]
    pub fn with_auth(auth: AuthSource) -> Self {
        Self {
            http: default_http_client(),
            auth: Some(auth),
            base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            chat_path: "/chat/completions".to_string(),
            default_model: DEFAULT_OPENAI_MODEL.to_string(),
            transform: Box::new(NoOpTransform),
            extra_headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_no_auth(model: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: default_http_client(),
            auth: None,
            base_url: base_url.into(),
            chat_path: "/chat/completions".to_string(),
            default_model: model.into(),
            transform: Box::new(NoOpTransform),
            extra_headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    #[must_use]
    pub fn with_chat_path(mut self, path: impl Into<String>) -> Self {
        self.chat_path = path.into();
        self
    }

    #[must_use]
    pub fn with_optional_auth(mut self, auth: AuthSource) -> Self {
        self.auth = match auth {
            AuthSource::None => None,
            other => Some(other),
        };
        self
    }

    #[must_use]
    pub fn with_transform(mut self, transform: Box<dyn ProviderTransform>) -> Self {
        self.transform = transform;
        self
    }

    #[must_use]
    pub fn with_extra_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = match &self.auth {
            Some(
                AuthSource::BearerToken(token)
                | AuthSource::ApiKey(token)
                | AuthSource::ApiKeyAndBearer {
                    bearer_token: token,
                    ..
                },
            ) => req.bearer_auth(token),
            Some(AuthSource::None) | None => req,
        };
        let mut req = req;
        for (name, value) in &self.extra_headers {
            req = req.header(name.as_str(), value.as_str());
        }
        req
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<OpenAiMessageStream, ApiError> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let body = build_openai_request(request, model, self.transform.as_ref());
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), self.chat_path);

        let req = self
            .http
            .post(&url)
            .header("content-type", "application/json");
        let req = self.apply_auth(req);
        let req = req.json(&body);

        let response = req.send().await.map_err(ApiError::from)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Api {
                status,
                error_type: None,
                message: None,
                body,
                retryable: matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504),
            });
        }

        Ok(OpenAiMessageStream {
            response,
            buffer: Vec::new(),
            state: OpenAiStreamState::new(),
            pending: VecDeque::new(),
            done: false,
        })
    }
}

/// Streaming response that yields [`StreamEvent`] values from `OpenAI` SSE chunks.
#[derive(Debug)]
pub struct OpenAiMessageStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    state: OpenAiStreamState,
    pending: VecDeque<StreamEvent>,
    done: bool,
}

impl OpenAiMessageStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            if self.done {
                return Ok(None);
            }

            if let Some(chunk) = self.response.chunk().await? {
                self.buffer.extend_from_slice(&chunk);
                self.drain_frames()?;
            } else {
                self.finish_buffer()?;
                self.done = true;
            }
        }
    }

    fn drain_frames(&mut self) -> Result<(), ApiError> {
        loop {
            let separator = self
                .buffer
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|p| (p, 2))
                .or_else(|| {
                    self.buffer
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| (p, 4))
                });

            let Some((pos, sep_len)) = separator else {
                break;
            };

            let frame: Vec<u8> = self.buffer.drain(..pos + sep_len).collect();
            let frame_str = String::from_utf8_lossy(&frame[..frame.len().saturating_sub(sep_len)]);

            self.process_frame(&frame_str)?;
            if self.done {
                break;
            }
        }

        Ok(())
    }

    fn finish_buffer(&mut self) -> Result<(), ApiError> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let frame = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
        self.process_frame(&frame)
    }

    fn process_frame(&mut self, frame: &str) -> Result<(), ApiError> {
        let mut data_lines: Vec<&str> = Vec::new();
        for line in frame.lines() {
            if line.starts_with(':') {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start());
            }
        }

        if data_lines.is_empty() {
            return Ok(());
        }

        let payload = data_lines.join("\n");
        if payload == "[DONE]" {
            self.done = true;
            return Ok(());
        }

        let chunk: OpenAiChunk = serde_json::from_str(&payload)?;
        let events = self.state.process_chunk(&chunk);
        self.pending.extend(events);
        Ok(())
    }
}

/// State machine: `OpenAI` SSE → [`StreamEvent`].
///
/// Complex because `OpenAI` uses implicit block boundaries (no explicit
/// start/stop signals for text or tool-call completion) while the Anthropic
/// protocol requires explicit events for each transition.
#[derive(Debug)]
struct OpenAiStreamState {
    message_id: String,
    model: String,
    started: bool,
    text_block_active: bool,
    next_block_index: u32,
    active_tools: HashMap<u32, u32>,
    input_tokens: u32,
    output_tokens: u32,
}

impl OpenAiStreamState {
    fn new() -> Self {
        Self {
            message_id: String::new(),
            model: String::new(),
            started: false,
            text_block_active: false,
            next_block_index: 0,
            active_tools: HashMap::new(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn process_chunk(&mut self, chunk: &OpenAiChunk) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        if let Some(id) = &chunk.id {
            if self.message_id.is_empty() {
                self.message_id.clone_from(id);
            }
        }
        if let Some(m) = &chunk.model {
            if self.model.is_empty() {
                self.model.clone_from(m);
            }
        }

        if let Some(usage) = &chunk.usage {
            self.input_tokens = usage.prompt_tokens.unwrap_or(0);
            self.output_tokens = usage.completion_tokens.unwrap_or(0);
        }

        if chunk.choices.is_empty() {
            return events;
        }

        let choice = &chunk.choices[0];
        let delta = &choice.delta;

        self.maybe_emit_message_start(delta, &mut events);
        self.emit_text_deltas(delta, &mut events);
        self.emit_tool_call_events(delta, &mut events);

        if let Some(finish_reason) = &choice.finish_reason {
            self.emit_finish(finish_reason, &mut events);
        }

        events
    }

    fn maybe_emit_message_start(&mut self, delta: &OpenAiDelta, events: &mut Vec<StreamEvent>) {
        if self.started {
            return;
        }
        if delta.role.is_none() && delta.content.is_none() && delta.tool_calls.is_none() {
            return;
        }
        self.started = true;
        events.push(StreamEvent::MessageStart(MessageStartEvent {
            message: MessageResponse {
                id: self.message_id.clone(),
                kind: "message".to_string(),
                role: "assistant".to_string(),
                content: Vec::new(),
                model: self.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    output_tokens: 0,
                },
                request_id: None,
            },
        }));
    }

    fn emit_text_deltas(&mut self, delta: &OpenAiDelta, events: &mut Vec<StreamEvent>) {
        let Some(content) = &delta.content else {
            return;
        };
        if content.is_empty() {
            return;
        }
        if !self.text_block_active {
            self.text_block_active = true;
            let idx = self.next_block_index;
            self.next_block_index += 1;
            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index: idx,
                content_block: OutputContentBlock::Text {
                    text: String::new(),
                },
            }));
        }
        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: self.next_block_index - 1,
            delta: ContentBlockDelta::TextDelta {
                text: content.clone(),
            },
        }));
    }

    fn emit_tool_call_events(&mut self, delta: &OpenAiDelta, events: &mut Vec<StreamEvent>) {
        let Some(tool_calls) = &delta.tool_calls else {
            return;
        };
        for tc in tool_calls {
            let tc_index = tc.index;

            if !self.active_tools.contains_key(&tc_index) {
                if self.text_block_active {
                    self.text_block_active = false;
                    events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                        index: self.next_block_index - 1,
                    }));
                }

                let block_idx = self.next_block_index;
                self.active_tools.insert(tc_index, block_idx);
                self.next_block_index += 1;

                let id = tc.id.clone().unwrap_or_default();
                let name = tc
                    .function
                    .as_ref()
                    .and_then(|f| f.name.clone())
                    .unwrap_or_default();

                events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                    index: block_idx,
                    content_block: OutputContentBlock::ToolUse {
                        id,
                        name,
                        input: Value::Object(serde_json::Map::new()),
                    },
                }));
            }

            if let Some(func) = &tc.function {
                if let Some(args) = &func.arguments {
                    if !args.is_empty() {
                        let block_idx = self.active_tools[&tc_index];
                        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                            index: block_idx,
                            delta: ContentBlockDelta::InputJsonDelta {
                                partial_json: args.clone(),
                            },
                        }));
                    }
                }
            }
        }
    }

    fn emit_finish(&mut self, finish_reason: &str, events: &mut Vec<StreamEvent>) {
        if self.text_block_active {
            self.text_block_active = false;
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: self.next_block_index - 1,
            }));
        }

        let mut tool_indices: Vec<u32> = self.active_tools.values().copied().collect();
        tool_indices.sort_unstable();
        for idx in tool_indices {
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: idx,
            }));
        }
        self.active_tools.clear();

        let stop_reason = match finish_reason {
            "stop" => "end_turn",
            "tool_calls" => "tool_use",
            "length" => "max_tokens",
            "content_filter" => "content_filter",
            other => other,
        };

        events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: MessageDelta {
                stop_reason: Some(stop_reason.to_string()),
                stop_sequence: None,
            },
            usage: Usage {
                input_tokens: self.input_tokens,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: self.output_tokens,
            },
        }));

        events.push(StreamEvent::MessageStop(MessageStopEvent {}));
    }
}

fn build_openai_request(
    request: &MessageRequest,
    model: &str,
    transform: &dyn ProviderTransform,
) -> Value {
    let messages = convert_messages(request, transform);

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    if request.max_tokens > 0 {
        body["max_tokens"] = Value::Number(request.max_tokens.into());
    }

    if let Some(tools) = &request.tools {
        let openai_tools: Vec<Value> = tools.iter().map(convert_tool).collect();
        body["tools"] = Value::Array(openai_tools);
    }

    if let Some(tc) = &request.tool_choice {
        body["tool_choice"] = match tc {
            ToolChoice::Auto => serde_json::json!("auto"),
            ToolChoice::Any => serde_json::json!("required"),
            ToolChoice::Tool { name } => {
                serde_json::json!({"type": "function", "function": {"name": name}})
            }
        };
    }

    body
}

fn convert_messages(request: &MessageRequest, transform: &dyn ProviderTransform) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();

    if let Some(system) = &request.system {
        messages.push(serde_json::json!({
            "role": "system",
            "content": system,
        }));
    }

    for msg in &request.messages {
        convert_input_message(msg, &mut messages, transform);
    }

    messages
}

fn convert_input_message(
    msg: &InputMessage,
    out: &mut Vec<Value>,
    transform: &dyn ProviderTransform,
) {
    match msg.role.as_str() {
        "assistant" => convert_assistant_message(msg, out, transform),
        "user" => convert_user_message(msg, out, transform),
        other => {
            for block in &msg.content {
                if let InputContentBlock::Text { text } = block {
                    out.push(serde_json::json!({
                        "role": other,
                        "content": text,
                    }));
                }
            }
        }
    }
}

fn convert_assistant_message(
    msg: &InputMessage,
    out: &mut Vec<Value>,
    transform: &dyn ProviderTransform,
) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    for block in &msg.content {
        match block {
            InputContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            InputContentBlock::ToolUse { id, name, input } => {
                let transformed_id = transform.transform_tool_call_id(id);
                tool_calls.push(serde_json::json!({
                    "id": transformed_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": input.to_string(),
                    },
                }));
            }
            InputContentBlock::ToolResult { .. } | InputContentBlock::Reasoning { .. } => {}
        }
    }

    let content = if text_parts.is_empty() {
        Value::Null
    } else {
        Value::String(text_parts.join("\n"))
    };

    let mut msg_obj = serde_json::json!({
        "role": "assistant",
        "content": content,
    });

    if !tool_calls.is_empty() {
        msg_obj["tool_calls"] = Value::Array(tool_calls);
    }

    out.push(msg_obj);
}

fn convert_user_message(
    msg: &InputMessage,
    out: &mut Vec<Value>,
    transform: &dyn ProviderTransform,
) {
    let mut text_parts: Vec<String> = Vec::new();

    for block in &msg.content {
        match block {
            InputContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            InputContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                if !text_parts.is_empty() {
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": text_parts.join("\n"),
                    }));
                    text_parts.clear();
                }

                let transformed_id = transform.transform_tool_call_id(tool_use_id);
                let content_text = content
                    .iter()
                    .map(|b| match b {
                        ToolResultContentBlock::Text { text } => text.clone(),
                        ToolResultContentBlock::Json { value } => value.to_string(),
                        ToolResultContentBlock::Image { .. } => {
                            "[image omitted]".to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                out.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": transformed_id,
                    "content": content_text,
                }));
            }
            InputContentBlock::ToolUse { .. } | InputContentBlock::Reasoning { .. } => {}
        }
    }

    if !text_parts.is_empty() {
        out.push(serde_json::json!({
            "role": "user",
            "content": text_parts.join("\n"),
        }));
    }
}

fn convert_tool(tool: &crate::types::ToolDefinition) -> Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description.as_deref().unwrap_or(""),
            "parameters": tool.input_schema,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{InputContentBlock, InputMessage, ToolDefinition};

    #[test]
    fn with_auth_creates_client() {
        let client = OpenAiClient::with_auth(AuthSource::BearerToken("token".to_string()));
        assert_eq!(
            client.auth,
            Some(AuthSource::BearerToken("token".to_string()))
        );
    }

    #[test]
    fn convert_simple_user_text_message() {
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 1024,
            messages: vec![InputMessage::user_text("Hello, world!")],
            system: Some("You are helpful.".to_string()),
            tools: None,
            tool_choice: None,
            stream: false,
            reasoning_effort: None,
        };

        let body = build_openai_request(&request, "gpt-4o", &NoOpTransform);
        let messages = body["messages"].as_array().expect("messages array");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello, world!");
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 1024);
    }

    #[test]
    fn convert_assistant_tool_use_and_tool_result() {
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 2048,
            messages: vec![
                InputMessage::user_text("Navigate to example.com"),
                InputMessage {
                    role: "assistant".to_string(),
                    content: vec![InputContentBlock::ToolUse {
                        id: "call_abc".to_string(),
                        name: "navigate".to_string(),
                        input: serde_json::json!({"url": "https://example.com"}),
                    }],
                },
                InputMessage::user_tool_result("call_abc", "Page loaded.", false),
            ],
            system: None,
            tools: Some(vec![ToolDefinition {
                name: "navigate".to_string(),
                description: Some("Go to a URL".to_string()),
                input_schema: serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
            }]),
            tool_choice: None,
            stream: false,
            reasoning_effort: None,
        };

        let body = build_openai_request(&request, "gpt-4o", &NoOpTransform);
        let messages = body["messages"].as_array().expect("messages array");

        assert_eq!(messages.len(), 3);

        let assistant = &messages[1];
        assert_eq!(assistant["role"], "assistant");
        assert!(assistant["content"].is_null());
        let tool_calls = assistant["tool_calls"].as_array().expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_abc");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "navigate");

        let tool_msg = &messages[2];
        assert_eq!(tool_msg["role"], "tool");
        assert_eq!(tool_msg["tool_call_id"], "call_abc");
        assert_eq!(tool_msg["content"], "Page loaded.");

        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "navigate");
    }

    fn make_chunk(json_str: &str) -> OpenAiChunk {
        serde_json::from_str(json_str).expect("valid chunk json")
    }

    #[test]
    fn sse_text_stream_produces_correct_events() {
        let mut state = OpenAiStreamState::new();

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
        ));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::MessageStart(_)));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
        ));
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockStart(ref e) if e.index == 0
        ));
        assert!(matches!(
            events[1],
            StreamEvent::ContentBlockDelta(ref e) if matches!(
                &e.delta,
                ContentBlockDelta::TextDelta { text } if text == "Hello"
            )
        ));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}"#,
        ));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockDelta(ref e) if matches!(
                &e.delta,
                ContentBlockDelta::TextDelta { text } if text == " world"
            )
        ));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        ));
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], StreamEvent::ContentBlockStop(_)));
        assert!(matches!(
            events[1],
            StreamEvent::MessageDelta(ref e) if e.delta.stop_reason.as_deref() == Some("end_turn")
        ));
        assert!(matches!(events[2], StreamEvent::MessageStop(_)));
    }

    #[test]
    fn sse_tool_call_stream_produces_correct_events() {
        let mut state = OpenAiStreamState::new();

        state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-2","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":null},"finish_reason":null}]}"#,
        ));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-2","model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_xyz","type":"function","function":{"name":"navigate","arguments":""}}]},"finish_reason":null}]}"#,
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockStart(ref cbs) if matches!(
                &cbs.content_block,
                OutputContentBlock::ToolUse { name, id, .. } if name == "navigate" && id == "call_xyz"
            )
        )));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-2","model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"url\":"}}]},"finish_reason":null}]}"#,
        ));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockDelta(ref e) if matches!(
                &e.delta,
                ContentBlockDelta::InputJsonDelta { partial_json } if partial_json == r#"{"url":"#
            )
        ));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-2","model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"https://example.com\"}"}}]},"finish_reason":null}]}"#,
        ));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockDelta(ref e) if matches!(
                &e.delta,
                ContentBlockDelta::InputJsonDelta { partial_json } if partial_json == r#""https://example.com"}"#
            )
        ));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-2","model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        ));
        assert!(events
            .iter()
            .any(|e| matches!(e, StreamEvent::ContentBlockStop(_))));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::MessageDelta(ref md) if md.delta.stop_reason.as_deref() == Some("tool_use")
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, StreamEvent::MessageStop(_))));
    }

    #[test]
    fn sse_usage_chunk_captured() {
        let mut state = OpenAiStreamState::new();

        state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-3","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"Hi"},"finish_reason":null}]}"#,
        ));

        state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-3","model":"gpt-4o","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
        ));

        let events = state.process_chunk(&make_chunk(
            r#"{"id":"chatcmpl-3","model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        ));

        let delta_event = events
            .iter()
            .find(|e| matches!(e, StreamEvent::MessageDelta(_)));
        assert!(delta_event.is_some());
        if let Some(StreamEvent::MessageDelta(md)) = delta_event {
            assert_eq!(md.usage.input_tokens, 10);
            assert_eq!(md.usage.output_tokens, 5);
        }
    }

    #[test]
    fn tool_choice_conversion() {
        let request = MessageRequest {
            model: String::new(),
            max_tokens: 1024,
            messages: vec![InputMessage::user_text("test")],
            system: None,
            tools: None,
            tool_choice: Some(ToolChoice::Any),
            stream: false,
            reasoning_effort: None,
        };
        let body = build_openai_request(&request, "gpt-4o", &NoOpTransform);
        assert_eq!(body["tool_choice"], "required");

        let request2 = MessageRequest {
            tool_choice: Some(ToolChoice::Tool {
                name: "navigate".to_string(),
            }),
            ..request
        };
        let body2 = build_openai_request(&request2, "gpt-4o", &NoOpTransform);
        assert_eq!(body2["tool_choice"]["type"], "function");
        assert_eq!(body2["tool_choice"]["function"]["name"], "navigate");
    }

    #[test]
    fn with_no_auth_sets_none_auth() {
        let client = ChatCompletionsClient::with_no_auth("llama3", "http://localhost:11434/v1");
        assert!(client.auth.is_none());
        assert_eq!(client.base_url, "http://localhost:11434/v1");
        assert_eq!(client.chat_path, "/chat/completions");
        assert_eq!(client.default_model, "llama3");
    }

    #[test]
    fn custom_base_url_used_in_request_url() {
        let client = ChatCompletionsClient::with_no_auth("llama3", "http://localhost:11434/v1");
        let url = format!(
            "{}{}",
            client.base_url.trim_end_matches('/'),
            client.chat_path
        );
        assert_eq!(url, "http://localhost:11434/v1/chat/completions");

        let client2 = ChatCompletionsClient::with_no_auth("llama3", "http://localhost:11434");
        let url2 = format!(
            "{}{}",
            client2.base_url.trim_end_matches('/'),
            client2.chat_path
        );
        assert_eq!(url2, "http://localhost:11434/chat/completions");
    }

    #[test]
    fn test_chat_completions_url_uses_preset_chat_path() {
        let client = ChatCompletionsClient::with_no_auth("grok-2", "https://api.x.ai/v1")
            .with_chat_path("/chat/completions");
        let url = format!(
            "{}{}",
            client.base_url.trim_end_matches('/'),
            client.chat_path
        );
        assert_eq!(url, "https://api.x.ai/v1/chat/completions");
        assert!(!url.contains("/v1/v1/"));
    }

    #[test]
    fn backwards_compat_alias_works() {
        let client = OpenAiClient::with_auth(AuthSource::BearerToken("tok".to_string()));
        assert_eq!(
            client.auth,
            Some(AuthSource::BearerToken("tok".to_string()))
        );
        assert_eq!(client.default_model, DEFAULT_OPENAI_MODEL);
    }
}
