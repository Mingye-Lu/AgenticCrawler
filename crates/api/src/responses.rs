use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{Map, Value};

use crate::client::AuthSource;
use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolResultContentBlock, Usage,
};

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

#[derive(Debug, Clone)]
pub struct OpenAiResponsesClient {
    http: reqwest::Client,
    auth: AuthSource,
    base_url: String,
    model: String,
    codex_endpoint: bool,
    account_id: Option<String>,
}

impl OpenAiResponsesClient {
    #[must_use]
    pub fn new(auth: AuthSource, model: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth,
            base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            model: model.into(),
            codex_endpoint: false,
            account_id: None,
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    #[must_use]
    pub fn with_codex_endpoint(mut self, account_id: Option<String>) -> Self {
        self.codex_endpoint = true;
        self.account_id = account_id;
        self
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<ResponsesMessageStream, ApiError> {
        let model = if request.model.is_empty() {
            &self.model
        } else {
            &request.model
        };

        let mut body = build_responses_request(request, model);
        if let Some(effort) = request.reasoning_effort {
            body["reasoning"] = serde_json::json!({"effort": effort.as_str(), "summary": "auto"});
            body["include"] = serde_json::json!(["reasoning.encrypted_content"]);
        }

        let url = if self.codex_endpoint {
            CODEX_RESPONSES_URL.to_string()
        } else {
            format!("{}/v1/responses", self.base_url.trim_end_matches('/'))
        };
        let mut req = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("originator", "acrawl");

        if let Some(token) = self.auth.bearer_token().or_else(|| self.auth.api_key()) {
            req = req.bearer_auth(token);
        }
        if let Some(id) = &self.account_id {
            req = req.header("ChatGPT-Account-Id", id);
        }

        let response = req.json(&body).send().await.map_err(ApiError::from)?;
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

        Ok(ResponsesMessageStream {
            response,
            buffer: Vec::new(),
            state: ResponsesStreamState::new(),
            pending: VecDeque::new(),
            done: false,
        })
    }
}

#[cfg(test)]
fn is_reasoning_model(model: &str) -> bool {
    model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("codex-")
        || model.starts_with("gpt-5")
}

#[must_use]
pub fn build_responses_request(request: &MessageRequest, model: &str) -> Value {
    let instructions = request
        .system
        .clone()
        .unwrap_or_else(|| "You are a helpful assistant.".to_string());

    let mut body = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": convert_responses_messages(&request.messages),
        "stream": true,
        "store": false,
        "tool_choice": responses_tool_choice(request.tool_choice.as_ref()),
        "parallel_tool_calls": true,
    });

    if let Some(tools) = &request.tools {
        body["tools"] = Value::Array(tools.iter().map(convert_responses_tool).collect());
    }

    body
}

#[must_use]
pub fn responses_tool_choice(choice: Option<&crate::types::ToolChoice>) -> Value {
    match choice {
        Some(ToolChoice::Auto) | None => serde_json::json!("auto"),
        Some(ToolChoice::Any) => serde_json::json!("required"),
        Some(ToolChoice::Tool { name }) => {
            serde_json::json!({"type": "function", "function": {"name": name}})
        }
    }
}

#[must_use]
pub fn convert_responses_messages(messages: &[InputMessage]) -> Vec<Value> {
    let mut out = Vec::new();
    for message in messages {
        convert_responses_message(message, &mut out);
    }
    out
}

fn convert_responses_message(message: &InputMessage, out: &mut Vec<Value>) {
    match message.role.as_str() {
        "assistant" => convert_responses_assistant_message(message, out),
        "user" => convert_responses_user_message(message, out),
        role => {
            for block in &message.content {
                match block {
                    InputContentBlock::Text { text } => {
                        push_responses_message_text(role, "input_text", text, out);
                    }
                    #[allow(unreachable_patterns)]
                    _ => {
                        if let Some(reasoning) = passthrough_reasoning_block(block) {
                            out.push(reasoning);
                        }
                    }
                }
            }
        }
    }
}

fn convert_responses_assistant_message(message: &InputMessage, out: &mut Vec<Value>) {
    let mut text_parts: Vec<String> = Vec::new();

    for block in &message.content {
        match block {
            InputContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            InputContentBlock::ToolUse { id, name, input } => {
                if !text_parts.is_empty() {
                    push_responses_message_text(
                        "assistant",
                        "output_text",
                        &text_parts.join("\n"),
                        out,
                    );
                    text_parts.clear();
                }

                out.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": input.to_string(),
                }));
            }
            InputContentBlock::ToolResult { .. } => {}
            InputContentBlock::Reasoning { .. } => {
                if !text_parts.is_empty() {
                    push_responses_message_text(
                        "assistant",
                        "output_text",
                        &text_parts.join("\n"),
                        out,
                    );
                    text_parts.clear();
                }

                if let Some(reasoning) = passthrough_reasoning_block(block) {
                    out.push(reasoning);
                }
            }
        }
    }

    if !text_parts.is_empty() {
        push_responses_message_text("assistant", "output_text", &text_parts.join("\n"), out);
    }
}

fn convert_responses_user_message(message: &InputMessage, out: &mut Vec<Value>) {
    let mut text_parts: Vec<String> = Vec::new();

    for block in &message.content {
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
                    push_responses_message_text("user", "input_text", &text_parts.join("\n"), out);
                    text_parts.clear();
                }

                let output = content
                    .iter()
                    .map(|block| match block {
                        ToolResultContentBlock::Text { text } => text.clone(),
                        ToolResultContentBlock::Json { value } => value.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                out.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": output,
                }));
            }
            InputContentBlock::ToolUse { .. } => {}
            InputContentBlock::Reasoning { .. } => {
                if !text_parts.is_empty() {
                    push_responses_message_text("user", "input_text", &text_parts.join("\n"), out);
                    text_parts.clear();
                }

                if let Some(reasoning) = passthrough_reasoning_block(block) {
                    out.push(reasoning);
                }
            }
        }
    }

    if !text_parts.is_empty() {
        push_responses_message_text("user", "input_text", &text_parts.join("\n"), out);
    }
}

fn push_responses_message_text(role: &str, content_type: &str, text: &str, out: &mut Vec<Value>) {
    out.push(serde_json::json!({
        "type": "message",
        "role": role,
        "content": [{"type": content_type, "text": text}],
    }));
}

fn passthrough_reasoning_block(block: &InputContentBlock) -> Option<Value> {
    let value = serde_json::to_value(block).ok()?;
    (value.get("type").and_then(Value::as_str) == Some("reasoning")).then_some(value)
}

#[must_use]
pub fn convert_responses_tool(tool: &crate::types::ToolDefinition) -> Value {
    serde_json::json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description.as_deref().unwrap_or(""),
        "parameters": tool.input_schema,
    })
}

#[derive(Debug)]
pub struct ResponsesMessageStream {
    pub(crate) response: reqwest::Response,
    pub(crate) buffer: Vec<u8>,
    pub(crate) state: ResponsesStreamState,
    pub(crate) pending: VecDeque<StreamEvent>,
    pub(crate) done: bool,
}

impl ResponsesMessageStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            if self.done {
                return Ok(None);
            }

            match self.response.chunk().await? {
                Some(chunk) => {
                    self.buffer.extend_from_slice(&chunk);
                    self.drain_lines()?;
                }
                None => {
                    self.done = true;
                }
            }
        }
    }

    fn drain_lines(&mut self) -> Result<(), ApiError> {
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=position).collect();
            if matches!(line.last(), Some(b'\n')) {
                line.pop();
            }

            let events = self.state.push_line(&line)?;
            self.pending.extend(events);
            if self.state.is_done() {
                self.done = true;
                break;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct ResponsesStreamState {
    started: bool,
    text_block_active: bool,
    next_block_index: u32,
    active_tools: HashMap<String, u32>,
    calls_with_deltas: HashSet<String>,
    input_tokens: u32,
    output_tokens: u32,
    pending_event_name: Option<String>,
    pending_data_lines: Vec<String>,
    done: bool,
}

impl ResponsesStreamState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: false,
            text_block_active: false,
            next_block_index: 0,
            active_tools: HashMap::new(),
            calls_with_deltas: HashSet::new(),
            input_tokens: 0,
            output_tokens: 0,
            pending_event_name: None,
            pending_data_lines: Vec::new(),
            done: false,
        }
    }

    pub fn push_line(&mut self, line: &[u8]) -> Result<Vec<StreamEvent>, ApiError> {
        let line = trim_ascii_line_end(line);
        if line.is_empty() {
            return self.finish_pending_event();
        }

        if line.starts_with(b":") {
            return Ok(Vec::new());
        }

        if let Some(name) = strip_ascii_prefix(line, b"event:") {
            self.pending_event_name = Some(String::from_utf8_lossy(name).trim().to_string());
            return Ok(Vec::new());
        }

        if let Some(data) = strip_ascii_prefix(line, b"data:") {
            self.pending_data_lines
                .push(String::from_utf8_lossy(data).trim_start().to_string());
        }

        Ok(Vec::new())
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn finish_pending_event(&mut self) -> Result<Vec<StreamEvent>, ApiError> {
        if self.pending_data_lines.is_empty() {
            self.pending_event_name = None;
            return Ok(Vec::new());
        }

        let payload = self.pending_data_lines.join("\n");
        let event_name = self.pending_event_name.take();
        self.pending_data_lines.clear();

        if payload == "[DONE]" {
            self.done = true;
            return Ok(Vec::new());
        }

        let data: Value = serde_json::from_str(&payload)?;
        let event_type = data
            .get("type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or(event_name)
            .unwrap_or_default();

        if event_type.is_empty() {
            return Ok(Vec::new());
        }

        self.process_event(&event_type, &data)
    }

    fn process_event(
        &mut self,
        event_type: &str,
        data: &Value,
    ) -> Result<Vec<StreamEvent>, ApiError> {
        let mut events = Vec::new();

        match event_type {
            "response.created" => {
                self.ensure_message_started(data, &mut events);
            }
            "response.output_text.delta" => {
                self.ensure_message_started(data, &mut events);
                self.emit_text_delta(data, &mut events);
            }
            "response.output_text.done" => {
                self.stop_text_block(&mut events);
            }
            "response.output_item.added" => {
                self.ensure_message_started(data, &mut events);
                self.emit_function_call_start(data, &mut events);
            }
            "response.function_call_arguments.delta" => {
                self.ensure_message_started(data, &mut events);
                self.emit_function_call_delta(data, &mut events);
            }
            "response.function_call_arguments.done" => {
                self.emit_function_call_arguments_done(data, &mut events);
            }
            "response.output_item.done" => {
                self.emit_function_call_stop(data, &mut events);
            }
            "response.completed" => {
                self.ensure_message_started(data, &mut events);
                self.update_usage(data);
                self.close_all_blocks(&mut events);
                let stop_reason = completed_stop_reason(data);

                events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
                    delta: MessageDelta {
                        stop_reason: Some(stop_reason),
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
            "error" => return Err(responses_stream_error(data)),
            _ => {}
        }

        Ok(events)
    }

    fn ensure_message_started(&mut self, data: &Value, events: &mut Vec<StreamEvent>) {
        if self.started {
            return;
        }

        self.started = true;
        events.push(StreamEvent::MessageStart(MessageStartEvent {
            message: MessageResponse {
                id: extract_response_id(data),
                kind: "message".to_string(),
                role: "assistant".to_string(),
                content: Vec::new(),
                model: extract_response_model(data),
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

    fn emit_text_delta(&mut self, data: &Value, events: &mut Vec<StreamEvent>) {
        let delta = data
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.is_empty() {
            return;
        }

        self.close_tool_blocks(events);

        if !self.text_block_active {
            self.text_block_active = true;
            let index = self.next_block_index;
            self.next_block_index += 1;
            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index,
                content_block: OutputContentBlock::Text {
                    text: String::new(),
                },
            }));
        }

        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: self.current_text_block_index(),
            delta: ContentBlockDelta::TextDelta {
                text: delta.to_string(),
            },
        }));
    }

    fn emit_function_call_start(&mut self, data: &Value, events: &mut Vec<StreamEvent>) {
        let item = data.get("item").unwrap_or(&Value::Null);
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type != "function_call" {
            return;
        }

        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| data.get("call_id").and_then(Value::as_str));
        let Some(call_id) = call_id.filter(|value| !value.is_empty()) else {
            return;
        };

        self.stop_text_block(events);

        let block_index = if let Some(index) = self.active_tools.get(call_id).copied() {
            index
        } else {
            let index = self.next_block_index;
            self.next_block_index += 1;
            self.active_tools.insert(call_id.to_string(), index);
            if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                self.active_tools.insert(item_id.to_string(), index);
            }

            let name = item
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| data.get("name").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();

            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index,
                content_block: OutputContentBlock::ToolUse {
                    id: call_id.to_string(),
                    name,
                    input: Value::Object(Map::new()),
                },
            }));

            index
        };

        if let Some(arguments) = item
            .get("arguments")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                index: block_index,
                delta: ContentBlockDelta::InputJsonDelta {
                    partial_json: arguments.to_string(),
                },
            }));
        }
    }

    fn emit_function_call_delta(&mut self, data: &Value, events: &mut Vec<StreamEvent>) {
        let delta = data
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.is_empty() {
            return;
        }

        let call_id = data
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| data.get("item_id").and_then(Value::as_str))
            .or_else(|| {
                data.get("item")
                    .and_then(|item| item.get("call_id"))
                    .and_then(Value::as_str)
            });
        let Some(call_id) = call_id.filter(|value| !value.is_empty()) else {
            return;
        };

        let block_index = if let Some(index) = self.active_tools.get(call_id).copied() {
            index
        } else {
            self.stop_text_block(events);
            let index = self.next_block_index;
            self.next_block_index += 1;
            self.active_tools.insert(call_id.to_string(), index);

            let name = data
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| {
                    data.get("item")
                        .and_then(|item| item.get("name"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .to_string();

            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index,
                content_block: OutputContentBlock::ToolUse {
                    id: call_id.to_string(),
                    name,
                    input: Value::Object(Map::new()),
                },
            }));

            index
        };

        self.calls_with_deltas.insert(call_id.to_string());

        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: block_index,
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: delta.to_string(),
            },
        }));
    }

    fn emit_function_call_arguments_done(&mut self, data: &Value, events: &mut Vec<StreamEvent>) {
        let call_id = data
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| data.get("item_id").and_then(Value::as_str));
        let Some(call_id) = call_id.filter(|value| !value.is_empty()) else {
            return;
        };

        let Some(block_index) = self.active_tools.get(call_id).copied() else {
            return;
        };

        if self.calls_with_deltas.contains(call_id)
            || self
                .active_tools
                .iter()
                .any(|(id, index)| *index == block_index && self.calls_with_deltas.contains(id))
        {
            return;
        }

        let arguments = data.get("arguments").and_then(Value::as_str);
        let Some(arguments) = arguments.filter(|value| !value.is_empty()) else {
            return;
        };

        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: block_index,
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: arguments.to_string(),
            },
        }));
    }

    fn emit_function_call_stop(&mut self, data: &Value, events: &mut Vec<StreamEvent>) {
        let item = data.get("item").unwrap_or(&Value::Null);
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type != "function_call" {
            return;
        }

        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| data.get("call_id").and_then(Value::as_str));

        let Some(call_id) = call_id else {
            return;
        };

        if let Some(index) = self.active_tools.remove(call_id) {
            let aliases: Vec<String> = self
                .active_tools
                .iter()
                .filter_map(|(id, alias_index)| (*alias_index == index).then_some(id.clone()))
                .collect();
            for alias in aliases {
                self.active_tools.remove(&alias);
                self.calls_with_deltas.remove(&alias);
            }

            if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                self.active_tools.remove(item_id);
                self.calls_with_deltas.remove(item_id);
            }

            self.calls_with_deltas.remove(call_id);
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index,
            }));
        }
    }

    fn stop_text_block(&mut self, events: &mut Vec<StreamEvent>) {
        if !self.text_block_active {
            return;
        }

        self.text_block_active = false;
        events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
            index: self.current_text_block_index(),
        }));
    }

    fn close_tool_blocks(&mut self, events: &mut Vec<StreamEvent>) {
        if self.active_tools.is_empty() {
            return;
        }

        let mut indices: Vec<u32> = self.active_tools.values().copied().collect();
        indices.sort_unstable();
        indices.dedup();
        for index in indices {
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index,
            }));
        }
        self.active_tools.clear();
        self.calls_with_deltas.clear();
    }

    fn close_all_blocks(&mut self, events: &mut Vec<StreamEvent>) {
        self.stop_text_block(events);
        self.close_tool_blocks(events);
    }

    fn current_text_block_index(&self) -> u32 {
        self.next_block_index.saturating_sub(1)
    }

    fn update_usage(&mut self, data: &Value) {
        let usage = data
            .get("response")
            .and_then(|response| response.get("usage"));
        let usage = usage.or_else(|| data.get("usage"));

        if let Some(input_tokens) = usage
            .and_then(|value| value.get("input_tokens"))
            .and_then(value_to_u32)
            .or_else(|| {
                usage
                    .and_then(|value| value.get("prompt_tokens"))
                    .and_then(value_to_u32)
            })
        {
            self.input_tokens = input_tokens;
        }

        if let Some(output_tokens) = usage
            .and_then(|value| value.get("output_tokens"))
            .and_then(value_to_u32)
            .or_else(|| {
                usage
                    .and_then(|value| value.get("completion_tokens"))
                    .and_then(value_to_u32)
            })
        {
            self.output_tokens = output_tokens;
        }
    }
}

impl Default for ResponsesStreamState {
    fn default() -> Self {
        Self::new()
    }
}

fn trim_ascii_line_end(mut line: &[u8]) -> &[u8] {
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line = &line[..line.len() - 1];
    }
    line
}

fn strip_ascii_prefix<'a>(line: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    line.strip_prefix(prefix)
}

fn value_to_u32(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|value| u32::try_from(value).ok())
}

fn completed_stop_reason(data: &Value) -> String {
    if let Some(reason) = data
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
    {
        return match reason {
            "max_output_tokens" => "max_tokens".to_string(),
            other => other.to_string(),
        };
    }

    if response_contains_function_call(data) {
        return "tool_use".to_string();
    }

    "end_turn".to_string()
}

fn response_contains_function_call(data: &Value) -> bool {
    let Some(output) = data
        .get("response")
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)
    else {
        return false;
    };

    output
        .iter()
        .any(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
}

fn extract_response_id(data: &Value) -> String {
    data.get("response")
        .and_then(|response| response.get("id"))
        .or_else(|| data.get("response_id"))
        .or_else(|| data.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn extract_response_model(data: &Value) -> String {
    data.get("response")
        .and_then(|response| response.get("model"))
        .or_else(|| data.get("model"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn responses_stream_error(data: &Value) -> ApiError {
    let nested_error = data.get("error");
    let error_type = nested_error
        .and_then(|error| error.get("type"))
        .or_else(|| data.get("type"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let message = nested_error
        .and_then(|error| error.get("message"))
        .or_else(|| data.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    ApiError::Api {
        status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        error_type,
        message,
        body: data.to_string(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;
    use crate::types::{ToolDefinition, ToolResultContentBlock};
    use crate::AuthSource;

    const TEXT_ONLY_STREAM_FIXTURE: &str = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-4.1\"}}\n",
        "\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n",
        "\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n",
        "\n",
        "event: response.output_text.done\n",
        "data: {\"type\":\"response.output_text.done\"}\n",
        "\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":3},\"output\":[{\"type\":\"output_text\"}]}}\n",
        "\n"
    );

    const TOOL_CALL_STREAM_FIXTURE: &str = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_2\",\"model\":\"gpt-4.1\"}}\n",
        "\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_abc\",\"name\":\"navigate\"}}\n",
        "\n",
        "event: response.function_call_arguments.delta\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"url\\\":\"}\n",
        "\n",
        "event: response.function_call_arguments.done\n",
        "data: {\"type\":\"response.function_call_arguments.done\",\"call_id\":\"call_abc\",\"arguments\":\"{\\\"url\\\":\\\"https://example.com\\\"}\"}\n",
        "\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_abc\"}}\n",
        "\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":4},\"output\":[{\"type\":\"function_call\"}]}}\n",
        "\n"
    );

    const ERROR_STREAM_FIXTURE: &str = concat!(
        "event: error\n",
        "data: {\"error\":{\"type\":\"server_error\",\"message\":\"boom\"}}\n",
        "\n"
    );

    fn sample_request() -> MessageRequest {
        MessageRequest {
            model: String::new(),
            max_tokens: 512,
            messages: vec![InputMessage::user_text("Hello")],
            system: Some("Be precise.".to_string()),
            tools: Some(vec![ToolDefinition {
                name: "navigate".to_string(),
                description: Some("Go to a page".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"url": {"type": "string"}},
                    "required": ["url"]
                }),
            }]),
            tool_choice: Some(ToolChoice::Tool {
                name: "navigate".to_string(),
            }),
            stream: false,
            reasoning_effort: None,
        }
    }

    fn collect_fixture_events(fixture: &str) -> Result<Vec<StreamEvent>, ApiError> {
        let mut state = ResponsesStreamState::new();
        let mut events = Vec::new();
        for line in fixture.split('\n') {
            let mut line_bytes = line.as_bytes().to_vec();
            if let Some(b'\r') = line_bytes.last() {
                line_bytes.pop();
            }
            events.extend(state.push_line(&line_bytes)?);
        }
        Ok(events)
    }

    fn spawn_test_server(response: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("local addr");
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];
            let mut headers_end = None;
            let mut content_length = 0_usize;

            loop {
                let read = stream.read(&mut chunk).expect("read request");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);

                if headers_end.is_none() {
                    headers_end = buffer
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|position| position + 4);

                    if let Some(end) = headers_end {
                        let headers = String::from_utf8_lossy(&buffer[..end]).to_lowercase();
                        content_length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                    }
                }

                if let Some(end) = headers_end {
                    let body_len = buffer.len().saturating_sub(end);
                    if body_len >= content_length {
                        break;
                    }
                }
            }

            tx.send(String::from_utf8(buffer).expect("utf8 request"))
                .expect("send request bytes");
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        (format!("http://{address}"), rx)
    }

    fn request_body_from_raw_http(raw_request: &str) -> Value {
        let body = raw_request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("request body separator");
        serde_json::from_str(body).expect("json body")
    }

    #[test]
    fn request_has_input_not_messages() {
        let body = build_responses_request(&sample_request(), "gpt-4.1");
        assert!(body.get("input").is_some());
        assert!(body.get("messages").is_none());
    }

    #[test]
    fn request_has_instructions_not_system() {
        let body = build_responses_request(&sample_request(), "gpt-4.1");
        assert_eq!(body["instructions"], "Be precise.");
        let input = body["input"].as_array().expect("input array");
        assert!(input
            .iter()
            .all(|item| item.get("role") != Some(&Value::String("system".to_string()))));
    }

    #[test]
    fn request_has_store_false() {
        let body = build_responses_request(&sample_request(), "gpt-4.1");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["parallel_tool_calls"], true);
    }

    #[test]
    fn request_flat_tool_format() {
        let body = build_responses_request(&sample_request(), "gpt-4.1");
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "navigate");
        assert_eq!(tools[0]["description"], "Go to a page");
        assert!(tools[0].get("function").is_none());
    }

    #[test]
    fn request_no_reasoning_field_by_default() {
        let body = build_responses_request(&sample_request(), "gpt-4.1");
        assert!(body.get("reasoning").is_none());
        assert!(body.get("include").is_none());
    }

    #[test]
    fn test_is_reasoning_model_o3() {
        assert!(is_reasoning_model("o3"));
    }

    #[test]
    fn test_is_reasoning_model_o4_mini() {
        assert!(is_reasoning_model("o4-mini"));
    }

    #[test]
    fn test_is_reasoning_model_codex_mini() {
        assert!(is_reasoning_model("codex-mini-latest"));
    }

    #[test]
    fn test_is_reasoning_model_gpt5() {
        assert!(is_reasoning_model("gpt-5"));
    }

    #[test]
    fn test_non_reasoning_model_gpt4o() {
        assert!(!is_reasoning_model("gpt-4o"));
    }

    #[test]
    fn test_non_reasoning_model_gpt4_turbo() {
        assert!(!is_reasoning_model("gpt-4-turbo"));
    }

    #[tokio::test]
    async fn request_body_has_reasoning_params_for_o3_model() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: text/event-stream\r\n",
            "Connection: close\r\n",
            "\r\n",
            "data: [DONE]\n\n"
        );
        let (base_url, requests) = spawn_test_server(response);
        let client = OpenAiResponsesClient::new(AuthSource::ApiKey("sk-test".to_string()), "o3")
            .with_base_url(base_url);

        let mut request = sample_request();
        request.reasoning_effort = Some(crate::types::ReasoningEffort::High);
        let _stream = client
            .stream_message(&request)
            .await
            .expect("request should succeed");

        let raw_request = requests.recv().expect("captured request");
        let raw_request_lower = raw_request.to_lowercase();
        assert!(raw_request.starts_with("POST /v1/responses HTTP/1.1"));
        assert!(raw_request_lower.contains("authorization: bearer sk-test\r\n"));
        assert!(raw_request_lower.contains("accept: text/event-stream\r\n"));

        let body = request_body_from_raw_http(&raw_request);
        assert_eq!(body["model"], "o3");
        assert_eq!(
            body["reasoning"],
            serde_json::json!({"effort": "high", "summary": "auto"})
        );
        assert_eq!(
            body["include"],
            serde_json::json!(["reasoning.encrypted_content"])
        );
        assert_eq!(body["store"], false);
    }

    #[tokio::test]
    async fn request_body_has_no_reasoning_params_for_gpt4o_model() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: text/event-stream\r\n",
            "Connection: close\r\n",
            "\r\n",
            "data: [DONE]\n\n"
        );
        let (base_url, requests) = spawn_test_server(response);
        let client =
            OpenAiResponsesClient::new(AuthSource::BearerToken("oauth-test".to_string()), "gpt-4o")
                .with_base_url(base_url);

        let _stream = client
            .stream_message(&sample_request())
            .await
            .expect("request should succeed");

        let raw_request = requests.recv().expect("captured request");
        assert!(raw_request
            .to_lowercase()
            .contains("authorization: bearer oauth-test\r\n"));

        let body = request_body_from_raw_http(&raw_request);
        assert_eq!(body["model"], "gpt-4o");
        assert!(body.get("reasoning").is_none());
        assert!(body.get("include").is_none());
        assert_eq!(body["store"], false);
    }

    #[test]
    fn user_text_converts_correctly() {
        let converted = convert_responses_messages(&[InputMessage::user_text("Hello")]);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["type"], "message");
        assert_eq!(converted[0]["role"], "user");
        assert_eq!(converted[0]["content"][0]["type"], "input_text");
        assert_eq!(converted[0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn assistant_text_converts_correctly() {
        let converted = convert_responses_messages(&[InputMessage {
            role: "assistant".to_string(),
            content: vec![InputContentBlock::Text {
                text: "Hi there".to_string(),
            }],
        }]);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "assistant");
        assert_eq!(converted[0]["content"][0]["type"], "output_text");
        assert_eq!(converted[0]["content"][0]["text"], "Hi there");
    }

    #[test]
    fn tool_call_converts_to_function_call() {
        let converted = convert_responses_messages(&[InputMessage {
            role: "assistant".to_string(),
            content: vec![InputContentBlock::ToolUse {
                id: "call_abc".to_string(),
                name: "navigate".to_string(),
                input: serde_json::json!({"url": "https://example.com"}),
            }],
        }]);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["type"], "function_call");
        assert_eq!(converted[0]["call_id"], "call_abc");
        assert_eq!(converted[0]["name"], "navigate");
        assert_eq!(
            converted[0]["arguments"],
            r#"{"url":"https://example.com"}"#
        );
    }

    #[test]
    fn tool_result_converts_to_function_call_output() {
        let converted = convert_responses_messages(&[InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: "call_abc".to_string(),
                content: vec![
                    ToolResultContentBlock::Text {
                        text: "Page loaded".to_string(),
                    },
                    ToolResultContentBlock::Json {
                        value: serde_json::json!({"ok": true}),
                    },
                ],
                is_error: false,
            }],
        }]);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["type"], "function_call_output");
        assert_eq!(converted[0]["call_id"], "call_abc");
        assert_eq!(converted[0]["output"], "Page loaded\n{\"ok\":true}");
    }

    #[test]
    fn streaming_text_only() {
        let events =
            collect_fixture_events(TEXT_ONLY_STREAM_FIXTURE).expect("fixture should parse");
        assert_eq!(events.len(), 7);
        assert!(matches!(events[0], StreamEvent::MessageStart(_)));
        assert!(matches!(events[1], StreamEvent::ContentBlockStart(ref event) if event.index == 0));
        assert!(
            matches!(events[2], StreamEvent::ContentBlockDelta(ref event)
            if matches!(&event.delta, ContentBlockDelta::TextDelta { text } if text == "Hello"))
        );
        assert!(
            matches!(events[3], StreamEvent::ContentBlockDelta(ref event)
            if matches!(&event.delta, ContentBlockDelta::TextDelta { text } if text == " world"))
        );
        assert!(matches!(events[4], StreamEvent::ContentBlockStop(ref event) if event.index == 0));
        assert!(matches!(events[5], StreamEvent::MessageDelta(ref event)
            if event.delta.stop_reason.as_deref() == Some("end_turn")
                && event.usage.input_tokens == 7
                && event.usage.output_tokens == 3));
        assert!(matches!(events[6], StreamEvent::MessageStop(_)));
    }

    #[test]
    fn streaming_tool_call() {
        let events =
            collect_fixture_events(TOOL_CALL_STREAM_FIXTURE).expect("fixture should parse");
        assert_eq!(events.len(), 6);
        assert!(matches!(events[0], StreamEvent::MessageStart(_)));
        assert!(
            matches!(events[1], StreamEvent::ContentBlockStart(ref event)
            if matches!(&event.content_block, OutputContentBlock::ToolUse { id, name, .. }
                if id == "call_abc" && name == "navigate"))
        );
        assert!(
            matches!(events[2], StreamEvent::ContentBlockDelta(ref event)
            if matches!(&event.delta, ContentBlockDelta::InputJsonDelta { partial_json }
                if partial_json == r#"{"url":"#))
        );
        assert!(matches!(events[3], StreamEvent::ContentBlockStop(ref event) if event.index == 0));
        assert!(matches!(events[4], StreamEvent::MessageDelta(ref event)
            if event.delta.stop_reason.as_deref() == Some("tool_use")
                && event.usage.input_tokens == 9
                && event.usage.output_tokens == 4));
        assert!(matches!(events[5], StreamEvent::MessageStop(_)));
    }

    #[test]
    fn streaming_error_event() {
        let error =
            collect_fixture_events(ERROR_STREAM_FIXTURE).expect_err("error event should fail");
        assert!(
            matches!(error, ApiError::Api { ref error_type, ref message, .. }
            if error_type.as_deref() == Some("server_error") && message.as_deref() == Some("boom"))
        );
    }

    #[test]
    fn streaming_function_call_done_without_deltas_uses_done_arguments() {
        let mut state = ResponsesStreamState::new();
        state
            .process_event(
                "response.created",
                &serde_json::json!({"type": "response.created", "response": {"id": "resp_3", "model": "gpt-4.1"}}),
            )
            .expect("response.created should parse");
        state
            .process_event(
                "response.output_item.added",
                &serde_json::json!({"type": "response.output_item.added", "item": {"type": "function_call", "call_id": "call_xyz", "name": "extract"}}),
            )
            .expect("response.output_item.added should parse");

        let events = state
            .process_event(
                "response.function_call_arguments.done",
                &serde_json::json!({"type": "response.function_call_arguments.done", "call_id": "call_xyz", "arguments": "{\"selector\":\"h1\"}"}),
            )
            .expect("response.function_call_arguments.done should parse");

        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], StreamEvent::ContentBlockDelta(ref event)
            if matches!(&event.delta, ContentBlockDelta::InputJsonDelta { partial_json }
                if partial_json == r#"{"selector":"h1"}"#))
        );
    }

    #[test]
    fn tool_choice_conversion_matches_responses_api() {
        assert_eq!(responses_tool_choice(None), serde_json::json!("auto"));
        assert_eq!(
            responses_tool_choice(Some(&ToolChoice::Any)),
            serde_json::json!("required")
        );
        assert_eq!(
            responses_tool_choice(Some(&ToolChoice::Tool {
                name: "navigate".to_string(),
            })),
            serde_json::json!({"type": "function", "function": {"name": "navigate"}})
        );
    }
}
