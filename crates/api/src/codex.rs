//! Codex provider — `OpenAI` Chat Completions with OAuth PKCE authentication.
//!
//! [`resolve_codex_auth`] tries sources in order: stored OAuth credentials,
//! then `OPENAI_API_KEY`. Both produce [`AuthSource::BearerToken`] because
//! `OpenAI` uses `Authorization: Bearer <token>` for all auth methods.

use std::collections::{HashMap, VecDeque};

use runtime::{
    clear_oauth_credentials, generate_pkce_pair, generate_state, load_oauth_credentials,
    save_oauth_credentials, OAuthAuthorizationRequest, OAuthConfig, PkceCodePair,
};
use serde_json::{Map, Value};

use crate::client::{oauth_token_is_expired, AuthSource, OAuthTokenSet};
use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolResultContentBlock, Usage,
};

pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_CALLBACK_PORT: u16 = 1455;
pub const CODEX_SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];
pub const DEFAULT_CODEX_MODEL: &str = "codex-mini-latest";
const DEFAULT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

#[must_use]
pub fn codex_oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: OPENAI_CLIENT_ID.to_string(),
        authorize_url: OPENAI_AUTH_URL.to_string(),
        token_url: OPENAI_TOKEN_URL.to_string(),
        callback_port: Some(CODEX_CALLBACK_PORT),
        manual_redirect_url: None,
        scopes: CODEX_SCOPES.iter().map(|s| (*s).to_string()).collect(),
    }
}

#[must_use]
pub fn read_codex_model() -> String {
    std::env::var("CODEX_MODEL").unwrap_or_else(|_| DEFAULT_CODEX_MODEL.to_string())
}

#[must_use]
pub fn read_codex_responses_url() -> String {
    std::env::var("CODEX_RESPONSES_URL").unwrap_or_else(|_| DEFAULT_CODEX_RESPONSES_URL.to_string())
}

/// Loopback redirect URI with `/auth/callback` path (matches Python implementation).
#[must_use]
pub fn codex_redirect_uri() -> String {
    format!("http://localhost:{CODEX_CALLBACK_PORT}/auth/callback")
}

#[derive(Debug)]
pub struct CodexLoginRequest {
    pub authorization_url: String,
    pub pkce: PkceCodePair,
    pub state: String,
    pub config: OAuthConfig,
    pub redirect_uri: String,
}

/// Initiates the Codex OAuth PKCE login flow, returning the authorization URL
/// and PKCE artifacts needed for the token exchange after user approval.
pub fn login() -> Result<CodexLoginRequest, ApiError> {
    let config = codex_oauth_config();
    let pkce = generate_pkce_pair().map_err(ApiError::from)?;
    let state = generate_state().map_err(ApiError::from)?;
    let redirect_uri = codex_redirect_uri();

    let auth_request =
        OAuthAuthorizationRequest::from_config(&config, &redirect_uri, &state, &pkce)
            .with_extra_param("id_token_add_organizations", "true")
            .with_extra_param("codex_cli_simplified_flow", "true");

    Ok(CodexLoginRequest {
        authorization_url: auth_request.build_url(),
        pkce,
        state,
        config,
        redirect_uri,
    })
}

pub fn save_codex_credentials(token_set: &runtime::OAuthTokenSet) -> Result<(), ApiError> {
    save_oauth_credentials(token_set).map_err(ApiError::from)
}

pub fn logout() -> Result<(), ApiError> {
    clear_oauth_credentials().map_err(ApiError::from)
}

/// Resolves Codex auth: stored OAuth token > `OPENAI_API_KEY` > error.
pub fn resolve_codex_auth() -> Result<AuthSource, ApiError> {
    if let Ok(Some(token_set)) = load_oauth_credentials() {
        let api_token = OAuthTokenSet {
            access_token: token_set.access_token,
            refresh_token: token_set.refresh_token,
            expires_at: token_set.expires_at,
            scopes: token_set.scopes,
        };
        if !oauth_token_is_expired(&api_token) {
            return Ok(AuthSource::BearerToken(api_token.access_token));
        }
    }

    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            return Ok(AuthSource::BearerToken(key));
        }
    }

    Err(ApiError::Auth(
        "no Codex OAuth credentials or OPENAI_API_KEY found; run `acrawl login` to authenticate"
            .to_string(),
    ))
}

#[derive(Debug, Clone)]
pub struct CodexClient {
    http: reqwest::Client,
    auth: AuthSource,
    responses_url: String,
    account_id: Option<String>,
    default_model: String,
}

impl CodexClient {
    #[must_use]
    pub fn new(auth: AuthSource, model: impl Into<String>) -> Self {
        let model_str = model.into();
        let account_id = auth.bearer_token().and_then(extract_account_id_from_jwt);
        Self {
            http: reqwest::Client::new(),
            auth,
            responses_url: read_codex_responses_url(),
            account_id,
            default_model: model_str,
        }
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<CodexMessageStream, ApiError> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let body = build_codex_request(request, model);

        let mut req = self
            .http
            .post(&self.responses_url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "codex");

        if let Some(account_id) = &self.account_id {
            req = req.header("chatgpt-account-id", account_id);
        }

        match &self.auth {
            AuthSource::BearerToken(token)
            | AuthSource::ApiKey(token)
            | AuthSource::ApiKeyAndBearer {
                bearer_token: token,
                ..
            } => {
                req = req.bearer_auth(token);
            }
            AuthSource::None => {}
        }

        req = req.json(&body);

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

        Ok(CodexMessageStream {
            response,
            buffer: Vec::new(),
            state: CodexStreamState::new(),
            pending: VecDeque::new(),
            done: false,
        })
    }
}

fn build_codex_request(request: &MessageRequest, model: &str) -> Value {
    let instructions = request
        .system
        .clone()
        .unwrap_or_else(|| "You are a helpful assistant.".to_string());

    let mut body = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": convert_codex_messages(&request.messages),
        "stream": true,
        "store": false,
        "tool_choice": codex_tool_choice(request.tool_choice.as_ref()),
        "parallel_tool_calls": true,
        "reasoning": {"effort": "high", "summary": "auto"},
        "include": ["reasoning.encrypted_content"],
    });

    if request.max_tokens > 0 {
        body["max_output_tokens"] = Value::Number(request.max_tokens.into());
    }

    if let Some(tools) = &request.tools {
        body["tools"] = Value::Array(tools.iter().map(convert_codex_tool).collect());
    }

    body
}

fn codex_tool_choice(choice: Option<&ToolChoice>) -> Value {
    match choice {
        Some(ToolChoice::Auto) | None => serde_json::json!("auto"),
        Some(ToolChoice::Any) => serde_json::json!("required"),
        Some(ToolChoice::Tool { name }) => {
            serde_json::json!({"type": "function", "function": {"name": name}})
        }
    }
}

fn convert_codex_messages(messages: &[InputMessage]) -> Vec<Value> {
    let mut out = Vec::new();
    for message in messages {
        convert_codex_message(message, &mut out);
    }
    out
}

fn convert_codex_message(message: &InputMessage, out: &mut Vec<Value>) {
    match message.role.as_str() {
        "assistant" => convert_codex_assistant_message(message, out),
        "user" => convert_codex_user_message(message, out),
        role => {
            for block in &message.content {
                if let InputContentBlock::Text { text } = block {
                    push_codex_message_text(role, "input_text", text.clone(), out);
                }
            }
        }
    }
}

fn convert_codex_assistant_message(message: &InputMessage, out: &mut Vec<Value>) {
    let mut text_parts: Vec<String> = Vec::new();

    for block in &message.content {
        match block {
            InputContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            InputContentBlock::ToolUse { id, name, input } => {
                if !text_parts.is_empty() {
                    push_codex_message_text(
                        "assistant",
                        "output_text",
                        text_parts.join("\n"),
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
        }
    }

    if !text_parts.is_empty() {
        push_codex_message_text("assistant", "output_text", text_parts.join("\n"), out);
    }
}

fn convert_codex_user_message(message: &InputMessage, out: &mut Vec<Value>) {
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
                    push_codex_message_text("user", "input_text", text_parts.join("\n"), out);
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
        }
    }

    if !text_parts.is_empty() {
        push_codex_message_text("user", "input_text", text_parts.join("\n"), out);
    }
}

fn push_codex_message_text(role: &str, content_type: &str, text: String, out: &mut Vec<Value>) {
    out.push(serde_json::json!({
        "type": "message",
        "role": role,
        "content": [{"type": content_type, "text": text}],
    }));
}

fn convert_codex_tool(tool: &crate::types::ToolDefinition) -> Value {
    serde_json::json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description.as_deref().unwrap_or(""),
        "parameters": tool.input_schema,
    })
}

fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = decode_base64url(payload)?;
    let payload_json: Value = serde_json::from_slice(&decoded).ok()?;

    if let Some(account_id) = payload_json
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("account_id"))
        .and_then(Value::as_str)
    {
        if !account_id.is_empty() {
            return Some(account_id.to_string());
        }
    }

    payload_json
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .filter(|account_id| !account_id.is_empty())
        .map(ToOwned::to_owned)
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    let mut normalized = input.replace('-', "+").replace('_', "/");
    while normalized.len() % 4 != 0 {
        normalized.push('=');
    }
    decode_base64_standard(&normalized)
}

fn decode_base64_standard(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    if bytes.len() % 4 != 0 {
        return None;
    }

    let chunk_count = bytes.len() / 4;
    let mut output = Vec::with_capacity(chunk_count.saturating_mul(3));

    for (idx, chunk) in bytes.chunks(4).enumerate() {
        let is_last = idx + 1 == chunk_count;
        if chunk[0] == b'=' || chunk[1] == b'=' {
            return None;
        }

        let pad = match (chunk[2], chunk[3]) {
            (b'=', b'=') => 2,
            (b'=', _) => return None,
            (_, b'=') => 1,
            _ => 0,
        };

        if pad > 0 && !is_last {
            return None;
        }

        let v0 = decode_base64_value(chunk[0])?;
        let v1 = decode_base64_value(chunk[1])?;
        let v2 = if chunk[2] == b'=' {
            0
        } else {
            decode_base64_value(chunk[2])?
        };
        let v3 = if chunk[3] == b'=' {
            0
        } else {
            decode_base64_value(chunk[3])?
        };

        let block = (u32::from(v0) << 18)
            | (u32::from(v1) << 12)
            | (u32::from(v2) << 6)
            | u32::from(v3);

        output.push(((block >> 16) & 0xFF) as u8);
        if pad < 2 {
            output.push(((block >> 8) & 0xFF) as u8);
        }
        if pad < 1 {
            output.push((block & 0xFF) as u8);
        }
    }

    Some(output)
}

fn decode_base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[derive(Debug)]
pub struct CodexMessageStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    state: CodexStreamState,
    pending: VecDeque<StreamEvent>,
    done: bool,
}

impl CodexMessageStream {
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
                    self.drain_frames()?;
                }
                None => {
                    self.done = true;
                }
            }
        }
    }

    fn drain_frames(&mut self) -> Result<(), ApiError> {
        loop {
            let separator = self
                .buffer
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (position, 2))
                .or_else(|| {
                    self.buffer
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|position| (position, 4))
                });

            let Some((position, separator_len)) = separator else {
                break;
            };

            let frame: Vec<u8> = self.buffer.drain(..position + separator_len).collect();
            let frame_str =
                String::from_utf8_lossy(&frame[..frame.len().saturating_sub(separator_len)]);

            let mut event_name: Option<String> = None;
            let mut data_lines: Vec<&str> = Vec::new();
            for line in frame_str.lines() {
                if line.starts_with(':') {
                    continue;
                }
                if let Some(name) = line.strip_prefix("event:") {
                    event_name = Some(name.trim().to_string());
                    continue;
                }
                if let Some(data) = line.strip_prefix("data:") {
                    data_lines.push(data.trim_start());
                }
            }

            if data_lines.is_empty() {
                continue;
            }

            let payload = data_lines.join("\n");
            if payload == "[DONE]" {
                self.done = true;
                break;
            }

            let data: Value = serde_json::from_str(&payload)?;
            let event_type = data
                .get("type")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or(event_name)
                .unwrap_or_default();

            if event_type.is_empty() {
                continue;
            }

            let events = self.state.process_event(&event_type, &data)?;
            self.pending.extend(events);
        }

        Ok(())
    }
}

#[derive(Debug)]
struct CodexStreamState {
    started: bool,
    text_block_active: bool,
    next_block_index: u32,
    active_tools: HashMap<String, u32>,
    input_tokens: u32,
    output_tokens: u32,
}

impl CodexStreamState {
    fn new() -> Self {
        Self {
            started: false,
            text_block_active: false,
            next_block_index: 0,
            active_tools: HashMap::new(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn process_event(&mut self, event_type: &str, data: &Value) -> Result<Vec<StreamEvent>, ApiError> {
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
            "error" => {
                return Err(codex_stream_error(data));
            }
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
        let delta = data.get("delta").and_then(Value::as_str).unwrap_or_default();
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

        let call_id = item.get("call_id").and_then(Value::as_str).or_else(|| {
            data.get("call_id").and_then(Value::as_str)
        });
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
        let delta = data.get("delta").and_then(Value::as_str).unwrap_or_default();
        if delta.is_empty() {
            return;
        }

        let call_id = data
            .get("call_id")
            .and_then(Value::as_str)
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

        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: block_index,
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: delta.to_string(),
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
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent { index }));
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
        for index in indices {
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent { index }));
        }
        self.active_tools.clear();
    }

    fn close_all_blocks(&mut self, events: &mut Vec<StreamEvent>) {
        self.stop_text_block(events);
        self.close_tool_blocks(events);
    }

    fn current_text_block_index(&self) -> u32 {
        self.next_block_index.saturating_sub(1)
    }

    fn update_usage(&mut self, data: &Value) {
        let usage = data.get("response").and_then(|response| response.get("usage"));
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

fn value_to_u32(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|v| u32::try_from(v).ok())
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

fn codex_stream_error(data: &Value) -> ApiError {
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use runtime::{
        clear_oauth_credentials, code_challenge_s256, load_oauth_credentials,
        save_oauth_credentials,
    };

    use super::*;
    use crate::error::ApiError;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn temp_config_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "codex-api-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_secs()
    }

    fn base64url_encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut output = String::new();
        let mut index = 0;

        while index + 3 <= bytes.len() {
            let block = (u32::from(bytes[index]) << 16)
                | (u32::from(bytes[index + 1]) << 8)
                | u32::from(bytes[index + 2]);
            output.push(TABLE[((block >> 18) & 0x3F) as usize] as char);
            output.push(TABLE[((block >> 12) & 0x3F) as usize] as char);
            output.push(TABLE[((block >> 6) & 0x3F) as usize] as char);
            output.push(TABLE[(block & 0x3F) as usize] as char);
            index += 3;
        }

        match bytes.len().saturating_sub(index) {
            1 => {
                let block = u32::from(bytes[index]) << 16;
                output.push(TABLE[((block >> 18) & 0x3F) as usize] as char);
                output.push(TABLE[((block >> 12) & 0x3F) as usize] as char);
            }
            2 => {
                let block = (u32::from(bytes[index]) << 16) | (u32::from(bytes[index + 1]) << 8);
                output.push(TABLE[((block >> 18) & 0x3F) as usize] as char);
                output.push(TABLE[((block >> 12) & 0x3F) as usize] as char);
                output.push(TABLE[((block >> 6) & 0x3F) as usize] as char);
            }
            _ => {}
        }

        output
    }

    fn build_jwt(payload: Value) -> String {
        let header = serde_json::json!({"alg": "none", "typ": "JWT"});
        format!(
            "{}.{}.sig",
            base64url_encode(header.to_string().as_bytes()),
            base64url_encode(payload.to_string().as_bytes())
        )
    }

    #[test]
    fn codex_responses_url_reads_from_env() {
        let _guard = env_lock();
        std::env::set_var("CODEX_RESPONSES_URL", "https://proxy.example.com/codex/responses");
        assert_eq!(
            read_codex_responses_url(),
            "https://proxy.example.com/codex/responses"
        );
        std::env::remove_var("CODEX_RESPONSES_URL");
    }

    #[test]
    fn build_codex_request_maps_user_and_assistant_messages() {
        let request = MessageRequest {
            model: String::new(),
            max_tokens: 512,
            messages: vec![
                InputMessage::user_text("Hello"),
                InputMessage {
                    role: "assistant".to_string(),
                    content: vec![InputContentBlock::Text {
                        text: "Hi there".to_string(),
                    }],
                },
            ],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
        };

        let body = build_codex_request(&request, "gpt-5.3-codex");
        let input = body["input"].as_array().expect("input array");

        assert_eq!(body["model"], "gpt-5.3-codex");
        assert_eq!(body["instructions"], "You are a helpful assistant.");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["parallel_tool_calls"], true);
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["max_output_tokens"], 512);

        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "Hello");

        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[1]["content"][0]["text"], "Hi there");
    }

    #[test]
    fn build_codex_request_maps_tool_use_and_tool_result() {
        let request = MessageRequest {
            model: String::new(),
            max_tokens: 0,
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
            system: Some("Use tools to browse pages.".to_string()),
            tools: Some(vec![crate::types::ToolDefinition {
                name: "navigate".to_string(),
                description: Some("Go to a URL".to_string()),
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
        };

        let body = build_codex_request(&request, "gpt-5.3-codex");
        let input = body["input"].as_array().expect("input array");
        let tools = body["tools"].as_array().expect("tools array");

        assert_eq!(body["instructions"], "Use tools to browse pages.");
        assert_eq!(input.len(), 3);

        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_abc");
        assert_eq!(input[1]["name"], "navigate");
        assert_eq!(input[1]["arguments"], r#"{"url":"https://example.com"}"#);

        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_abc");
        assert_eq!(input[2]["output"], "Page loaded.");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "navigate");
        assert_eq!(tools[0]["description"], "Go to a URL");
        assert_eq!(
            body["tool_choice"]["function"]["name"],
            "navigate"
        );
    }

    #[test]
    fn extract_account_id_from_jwt_reads_account_claims() {
        let nested_claim_token = build_jwt(serde_json::json!({
            "https://api.openai.com/auth": {
                "account_id": "org_abc123"
            }
        }));
        assert_eq!(
            extract_account_id_from_jwt(&nested_claim_token),
            Some("org_abc123".to_string())
        );

        let fallback_claim_token = build_jwt(serde_json::json!({
            "chatgpt_account_id": "org_fallback"
        }));
        assert_eq!(
            extract_account_id_from_jwt(&fallback_claim_token),
            Some("org_fallback".to_string())
        );
    }

    #[test]
    fn codex_stream_state_processes_text_delta_events() {
        let mut state = CodexStreamState::new();

        let events = state
            .process_event(
                "response.created",
                &serde_json::json!({
                    "type": "response.created",
                    "response": {"id": "resp_1", "model": "gpt-5.3-codex"}
                }),
            )
            .expect("response.created should parse");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::MessageStart(_)));

        let events = state
            .process_event(
                "response.output_text.delta",
                &serde_json::json!({
                    "type": "response.output_text.delta",
                    "delta": "Hello"
                }),
            )
            .expect("response.output_text.delta should parse");
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockStart(ref e) if e.index == 0
        ));
        assert!(matches!(
            events[1],
            StreamEvent::ContentBlockDelta(ref e)
                if matches!(&e.delta, ContentBlockDelta::TextDelta { text } if text == "Hello")
        ));

        let events = state
            .process_event(
                "response.output_text.done",
                &serde_json::json!({"type": "response.output_text.done"}),
            )
            .expect("response.output_text.done should parse");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockStop(ref e) if e.index == 0
        ));

        let events = state
            .process_event(
                "response.completed",
                &serde_json::json!({
                    "type": "response.completed",
                    "response": {
                        "usage": {"input_tokens": 7, "output_tokens": 3},
                        "output": [{"type": "output_text"}]
                    }
                }),
            )
            .expect("response.completed should parse");
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            StreamEvent::MessageDelta(ref event)
                if event.delta.stop_reason.as_deref() == Some("end_turn")
                    && event.usage.input_tokens == 7
                    && event.usage.output_tokens == 3
        ));
        assert!(matches!(events[1], StreamEvent::MessageStop(_)));
    }

    #[test]
    fn codex_stream_state_processes_function_call_events() {
        let mut state = CodexStreamState::new();

        state
            .process_event(
                "response.created",
                &serde_json::json!({
                    "type": "response.created",
                    "response": {"id": "resp_2", "model": "gpt-5.3-codex"}
                }),
            )
            .expect("response.created should parse");

        let events = state
            .process_event(
                "response.output_item.added",
                &serde_json::json!({
                    "type": "response.output_item.added",
                    "item": {
                        "type": "function_call",
                        "call_id": "call_abc",
                        "name": "navigate"
                    }
                }),
            )
            .expect("response.output_item.added should parse");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockStart(ref e)
                if e.index == 0
                    && matches!(
                        &e.content_block,
                        OutputContentBlock::ToolUse { id, name, .. }
                            if id == "call_abc" && name == "navigate"
                    )
        ));

        let events = state
            .process_event(
                "response.function_call_arguments.delta",
                &serde_json::json!({
                    "type": "response.function_call_arguments.delta",
                    "call_id": "call_abc",
                    "delta": "{\"url\":"
                }),
            )
            .expect("response.function_call_arguments.delta should parse");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockDelta(ref e)
                if e.index == 0
                    && matches!(
                        &e.delta,
                        ContentBlockDelta::InputJsonDelta { partial_json }
                            if partial_json == r#"{"url":"#
                    )
        ));

        let events = state
            .process_event(
                "response.output_item.done",
                &serde_json::json!({
                    "type": "response.output_item.done",
                    "item": {
                        "type": "function_call",
                        "call_id": "call_abc"
                    }
                }),
            )
            .expect("response.output_item.done should parse");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            StreamEvent::ContentBlockStop(ref e) if e.index == 0
        ));

        let events = state
            .process_event(
                "response.completed",
                &serde_json::json!({
                    "type": "response.completed",
                    "response": {
                        "usage": {"input_tokens": 9, "output_tokens": 4},
                        "output": [{"type": "function_call"}]
                    }
                }),
            )
            .expect("response.completed should parse");
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            StreamEvent::MessageDelta(ref event)
                if event.delta.stop_reason.as_deref() == Some("tool_use")
                    && event.usage.input_tokens == 9
                    && event.usage.output_tokens == 4
        ));
        assert!(matches!(events[1], StreamEvent::MessageStop(_)));
    }

    #[test]
    fn codex_oauth_config_has_correct_endpoints() {
        let config = codex_oauth_config();
        assert_eq!(config.client_id, OPENAI_CLIENT_ID);
        assert_eq!(config.authorize_url, OPENAI_AUTH_URL);
        assert_eq!(config.token_url, OPENAI_TOKEN_URL);
        assert_eq!(config.callback_port, Some(CODEX_CALLBACK_PORT));
        assert_eq!(
            config.scopes,
            CODEX_SCOPES
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn codex_redirect_uri_matches_expected_format() {
        assert_eq!(codex_redirect_uri(), "http://localhost:1455/auth/callback");
    }

    #[test]
    fn pkce_s256_challenge_matches_rfc7636_vector() {
        let challenge = code_challenge_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn pkce_challenge_is_url_safe_and_unpadded() {
        let challenge = code_challenge_s256("test-verifier-string-for-codex");
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
        assert!(!challenge.contains('='));
        assert!(!challenge.is_empty());
    }

    #[test]
    fn default_codex_model_is_codex_mini_latest() {
        let _guard = env_lock();
        std::env::remove_var("CODEX_MODEL");
        assert_eq!(read_codex_model(), DEFAULT_CODEX_MODEL);
    }

    #[test]
    fn codex_model_reads_from_env() {
        let _guard = env_lock();
        std::env::set_var("CODEX_MODEL", "codex-large-2025");
        assert_eq!(read_codex_model(), "codex-large-2025");
        std::env::remove_var("CODEX_MODEL");
    }

    #[test]
    #[cfg(unix)]
    fn login_produces_valid_authorization_url() {
        let request = login().expect("login should produce a request");
        assert!(request
            .authorization_url
            .starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(request.authorization_url.contains("response_type=code"));
        assert!(request
            .authorization_url
            .contains(&format!("client_id={OPENAI_CLIENT_ID}")));
        assert!(request
            .authorization_url
            .contains("code_challenge_method=S256"));
        assert!(request
            .authorization_url
            .contains("codex_cli_simplified_flow=true"));
        assert!(!request.pkce.verifier.is_empty());
        assert!(!request.pkce.challenge.is_empty());
        assert!(!request.state.is_empty());
        assert_eq!(request.redirect_uri, codex_redirect_uri());
    }

    #[test]
    fn resolve_codex_auth_uses_stored_oauth_token() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "codex-oauth-token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_secs() + 3600),
            scopes: vec!["openid".to_string()],
        })
        .expect("save credentials");

        let auth = resolve_codex_auth().expect("should resolve from OAuth");
        assert_eq!(auth.bearer_token(), Some("codex-oauth-token"));

        clear_oauth_credentials().expect("clear");
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }

    #[test]
    fn resolve_codex_auth_skips_expired_oauth_falls_back_to_api_key() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::set_var("OPENAI_API_KEY", "sk-fallback-key");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "expired-codex-token".to_string(),
            refresh_token: None,
            expires_at: Some(1),
            scopes: Vec::new(),
        })
        .expect("save expired credentials");

        let auth = resolve_codex_auth().expect("should fall back to API key");
        assert_eq!(auth.bearer_token(), Some("sk-fallback-key"));

        clear_oauth_credentials().expect("clear");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }

    #[test]
    fn resolve_codex_auth_uses_api_key_when_no_oauth_stored() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::set_var("OPENAI_API_KEY", "sk-test-key");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        let auth = resolve_codex_auth().expect("should resolve from API key");
        assert_eq!(auth.bearer_token(), Some("sk-test-key"));

        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn resolve_codex_auth_errors_when_no_credentials() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        let error = resolve_codex_auth().expect_err("should error without credentials");
        assert!(matches!(error, ApiError::Auth(ref msg) if msg.contains("acrawl login")));

        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn logout_clears_stored_credentials() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "token-to-clear".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_secs() + 3600),
            scopes: Vec::new(),
        })
        .expect("save credentials");

        logout().expect("logout should succeed");

        let loaded = load_oauth_credentials().expect("load after logout");
        assert!(loaded.is_none());

        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }

    #[test]
    fn save_codex_credentials_persists_token_set() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("CLAUDE_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        let token_set = runtime::OAuthTokenSet {
            access_token: "new-codex-token".to_string(),
            refresh_token: Some("new-refresh".to_string()),
            expires_at: Some(now_secs() + 7200),
            scopes: vec!["openid".to_string(), "offline_access".to_string()],
        };
        save_codex_credentials(&token_set).expect("save codex credentials");

        let loaded = load_oauth_credentials()
            .expect("load credentials")
            .expect("token set present");
        assert_eq!(loaded.access_token, "new-codex-token");
        assert_eq!(loaded.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(
            loaded.scopes,
            vec!["openid".to_string(), "offline_access".to_string()]
        );

        clear_oauth_credentials().expect("clear");
        std::env::remove_var("CLAUDE_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }
}
