use std::collections::{HashMap, VecDeque};

use serde::Deserialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::sse::SseParser;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolDefinition, ToolResultContentBlock, Usage,
};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_MESSAGE_ID: &str = "gemini-message";

#[derive(Debug, Clone)]
pub struct GeminiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl GeminiClient {
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<GeminiMessageStream, ApiError> {
        if self.api_key.is_empty() {
            return Err(ApiError::MissingApiKey);
        }

        let body = build_gemini_request(request);
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url.trim_end_matches('/'),
            request.model,
            self.api_key,
        );

        let response = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(ApiError::from)?;
        let response = expect_success(response).await?;

        Ok(GeminiMessageStream {
            response,
            parser: SseParser::new(),
            state: GeminiStreamState::new(request.model.clone()),
            pending: VecDeque::new(),
            done: false,
        })
    }
}

#[derive(Debug)]
pub struct GeminiMessageStream {
    response: reqwest::Response,
    parser: SseParser,
    state: GeminiStreamState,
    pending: VecDeque<StreamEvent>,
    done: bool,
}

impl GeminiMessageStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            if self.done {
                return Ok(None);
            }

            if let Some(chunk) = self.response.chunk().await? {
                for frame in self.parser.push_frames(&chunk) {
                    if let Some(response) = parse_gemini_frame(&frame)? {
                        self.pending.extend(self.state.process_response(&response));
                    }
                }
            } else {
                for frame in self.parser.finish_frames() {
                    if let Some(response) = parse_gemini_frame(&frame)? {
                        self.pending.extend(self.state.process_response(&response));
                    }
                }
                self.done = true;
            }
        }
    }
}

#[derive(Debug)]
struct GeminiStreamState {
    model: String,
    started: bool,
    next_block_index: u32,
    active_block: Option<ActiveBlock>,
    usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveBlockKind {
    Text,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveBlock {
    index: u32,
    kind: ActiveBlockKind,
}

impl GeminiStreamState {
    fn new(model: String) -> Self {
        Self {
            model,
            started: false,
            next_block_index: 0,
            active_block: None,
            usage: Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: 0,
            },
        }
    }

    fn process_response(&mut self, response: &GenerateContentResponse) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        if let Some(usage) = &response.usage_metadata {
            self.usage.input_tokens = usage.prompt_token_count.unwrap_or(0);
            self.usage.output_tokens = usage.candidates_token_count.unwrap_or(0);
        }

        let candidate = response.candidates.first();

        if !self.started && candidate.is_some() {
            self.started = true;
            events.push(StreamEvent::MessageStart(MessageStartEvent {
                message: MessageResponse {
                    id: DEFAULT_MESSAGE_ID.to_string(),
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

        if let Some(candidate) = candidate {
            if let Some(content) = &candidate.content {
                for part in &content.parts {
                    if let Some(text) = &part.text {
                        let index = self.ensure_active_block(
                            ActiveBlockKind::Text,
                            &mut events,
                            OutputContentBlock::Text {
                                text: String::new(),
                            },
                        );
                        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                            index,
                            delta: ContentBlockDelta::TextDelta { text: text.clone() },
                        }));
                    }

                    if let Some(function_call) = &part.function_call {
                        let index = self.ensure_active_block(
                            ActiveBlockKind::Tool,
                            &mut events,
                            convert_function_call_to_block(function_call, self.next_block_index),
                        );
                        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                            index,
                            delta: ContentBlockDelta::InputJsonDelta {
                                partial_json: serde_json::to_string(&function_call.args)
                                    .expect("function call args must serialize"),
                            },
                        }));
                    }
                }
            }

            if let Some(finish_reason) = candidate.finish_reason.as_deref() {
                self.stop_active_block(&mut events);
                events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
                    delta: MessageDelta {
                        stop_reason: Some(map_finish_reason(finish_reason).to_string()),
                        stop_sequence: None,
                    },
                    usage: self.usage.clone(),
                }));
                events.push(StreamEvent::MessageStop(MessageStopEvent {}));
            }
        }

        events
    }

    fn ensure_active_block(
        &mut self,
        kind: ActiveBlockKind,
        events: &mut Vec<StreamEvent>,
        block: OutputContentBlock,
    ) -> u32 {
        if let Some(active) = self.active_block {
            if active.kind == kind {
                return active.index;
            }
            self.stop_active_block(events);
        }

        let index = self.next_block_index;
        self.next_block_index += 1;
        self.active_block = Some(ActiveBlock { index, kind });
        events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index,
            content_block: block,
        }));
        index
    }

    fn stop_active_block(&mut self, events: &mut Vec<StreamEvent>) {
        if let Some(active) = self.active_block.take() {
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: active.index,
            }));
        }
    }
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContent>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Clone, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Deserialize)]
struct GeminiUsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
}

fn build_gemini_request(request: &MessageRequest) -> Value {
    let mut body = serde_json::json!({
        "contents": convert_messages(&request.messages),
        "generationConfig": {
            "maxOutputTokens": request.max_tokens,
        },
    });

    if let Some(system) = &request.system {
        body["systemInstruction"] = serde_json::json!({
            "parts": [{"text": system}],
        });
    }

    if let Some(tools) = &request.tools {
        if !tools.is_empty() {
            body["tools"] = serde_json::json!([
                {
                    "functionDeclarations": tools.iter().map(convert_tool).collect::<Vec<_>>(),
                }
            ]);
        }
    }

    if let Some(tool_choice) = &request.tool_choice {
        body["toolConfig"] = convert_tool_choice(tool_choice);
    }

    body
}

fn convert_messages(messages: &[InputMessage]) -> Vec<Value> {
    let mut contents = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    for message in messages {
        if let Some(content) = convert_message(message, &mut tool_names_by_id) {
            contents.push(content);
        }
    }

    contents
}

fn convert_message(
    message: &InputMessage,
    tool_names_by_id: &mut HashMap<String, String>,
) -> Option<Value> {
    let role = if message.role == "assistant" {
        "model"
    } else {
        "user"
    };

    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            InputContentBlock::Text { text } => {
                parts.push(serde_json::json!({"text": text}));
            }
            InputContentBlock::ToolUse { id, name, input } if message.role == "assistant" => {
                tool_names_by_id.insert(id.clone(), name.clone());
                parts.push(serde_json::json!({
                    "functionCall": {
                        "name": name,
                        "args": input,
                    }
                }));
            }
            InputContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if message.role == "user" => {
                let name = tool_names_by_id
                    .get(tool_use_id)
                    .cloned()
                    .unwrap_or_else(|| tool_use_id.clone());
                parts.push(serde_json::json!({
                    "functionResponse": {
                        "name": name,
                        "response": tool_result_response(content, *is_error),
                    }
                }));
            }
            InputContentBlock::ToolUse { .. }
            | InputContentBlock::ToolResult { .. }
            | InputContentBlock::Reasoning { .. } => {}
        }
    }

    (!parts.is_empty()).then(|| {
        serde_json::json!({
            "role": role,
            "parts": parts,
        })
    })
}

fn tool_result_response(content: &[ToolResultContentBlock], is_error: bool) -> Value {
    if let [ToolResultContentBlock::Json { value }] = content {
        if !is_error {
            return value.clone();
        }
    }

    let joined = content
        .iter()
        .map(|block| match block {
            ToolResultContentBlock::Text { text } => text.clone(),
            ToolResultContentBlock::Json { value } => value.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");

    serde_json::json!({
        "content": joined,
        "is_error": is_error,
    })
}

fn convert_tool(tool: &ToolDefinition) -> Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description.as_deref().unwrap_or(""),
        "parameters": tool.input_schema,
    })
}

fn convert_tool_choice(tool_choice: &ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::Auto => serde_json::json!({
            "functionCallingConfig": { "mode": "AUTO" }
        }),
        ToolChoice::Any => serde_json::json!({
            "functionCallingConfig": { "mode": "ANY" }
        }),
        ToolChoice::Tool { name } => serde_json::json!({
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": [name],
            }
        }),
    }
}

fn parse_gemini_frame(frame: &str) -> Result<Option<GenerateContentResponse>, ApiError> {
    let trimmed = frame.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut data_lines = Vec::new();
    let mut event_name: Option<&str> = None;

    for line in trimmed.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(name) = line.strip_prefix("event:") {
            event_name = Some(name.trim());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start());
        }
    }

    if matches!(event_name, Some("ping")) || data_lines.is_empty() {
        return Ok(None);
    }

    let payload = data_lines.join("\n");
    if payload == "[DONE]" {
        return Ok(None);
    }

    serde_json::from_str(&payload)
        .map(Some)
        .map_err(ApiError::from)
}

fn convert_function_call_to_block(
    function_call: &GeminiFunctionCall,
    fallback_index: u32,
) -> OutputContentBlock {
    OutputContentBlock::ToolUse {
        id: format!("gemini_tool_{fallback_index}"),
        name: function_call.name.clone(),
        input: serde_json::json!({}),
    }
}

fn map_finish_reason(finish_reason: &str) -> &str {
    match finish_reason {
        "STOP" => "end_turn",
        "MAX_TOKENS" => "max_tokens",
        other => other,
    }
}

async fn expect_success(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    Err(ApiError::Api {
        status,
        error_type: None,
        message: None,
        body,
        retryable: matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_request_format() {
        let request = MessageRequest {
            model: "gemini-2.0-flash".to_string(),
            max_tokens: 512,
            messages: vec![InputMessage::user_text("Hello Gemini")],
            system: Some("Be concise.".to_string()),
            tools: Some(vec![ToolDefinition {
                name: "navigate".to_string(),
                description: Some("Open a URL".to_string()),
                input_schema: serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
            }]),
            tool_choice: Some(ToolChoice::Any),
            stream: false,
            reasoning_effort: None,
        };

        let body = build_gemini_request(&request);

        assert!(body.get("messages").is_none());
        let contents = body["contents"].as_array().expect("contents array");
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello Gemini");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 512);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be concise.");
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "navigate");
        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
    }

    #[test]
    fn test_gemini_stream_parse() {
        let response = parse_gemini_frame(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2}}\n\n",
        )
        .expect("frame parses")
        .expect("response present");

        let mut state = GeminiStreamState::new("gemini-2.0-flash".to_string());
        let events = state.process_response(&response);

        assert!(matches!(events[0], StreamEvent::MessageStart(_)));
        assert!(matches!(
            events[1],
            StreamEvent::ContentBlockStart(ContentBlockStartEvent { index: 0, .. })
        ));
        assert!(matches!(
            events[2],
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                index: 0,
                delta: ContentBlockDelta::TextDelta { ref text }
            }) if text == "Hello"
        ));
        assert!(matches!(
            events[3],
            StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
        ));
        assert!(matches!(
            events[4],
            StreamEvent::MessageDelta(MessageDeltaEvent {
                delta: MessageDelta { stop_reason: Some(ref reason), .. },
                usage: Usage { input_tokens: 3, output_tokens: 2, .. }
            }) if reason == "end_turn"
        ));
        assert!(matches!(events[5], StreamEvent::MessageStop(_)));
    }

    #[test]
    fn test_gemini_tool_call_conversion() {
        let response: GenerateContentResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "navigate",
                            "args": {"url": "https://example.com"}
                        }
                    }]
                }
            }]
        }))
        .expect("response json");

        let mut state = GeminiStreamState::new("gemini-2.0-flash".to_string());
        let events = state.process_response(&response);

        assert!(matches!(events[0], StreamEvent::MessageStart(_)));
        assert!(matches!(
            events[1],
            StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                content_block: OutputContentBlock::ToolUse { ref name, ref id, ref input },
                ..
            }) if name == "navigate" && id == "gemini_tool_0" && input == &serde_json::json!({})
        ));
        assert!(matches!(
            events[2],
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                delta: ContentBlockDelta::InputJsonDelta { ref partial_json },
                ..
            }) if partial_json == r#"{"url":"https://example.com"}"#
        ));
    }
}
