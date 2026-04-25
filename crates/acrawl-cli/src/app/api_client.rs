use api::{
    provider::{ProviderClient, ProviderRegistry},
    ContentBlockDelta, InputContentBlock, InputMessage, MessageRequest, MessageResponse,
    OutputContentBlock, ToolChoice, ToolDefinition, ToolResultContentBlock,
};
use runtime::{ApiClient, AssistantEvent, ConversationMessage, MessageRole, RuntimeError, TokenUsage};
use serde_json::json;

use super::{filter_tool_specs, AllowedToolSet};

pub(crate) struct LlmRuntimeClient {
    pub(crate) registry: ProviderRegistry,
    provider: ProviderClient,
    model: String,
    enable_tools: bool,
    allowed_tools: Option<AllowedToolSet>,
    pub(crate) reasoning_effort: Option<api::ReasoningEffort>,
}

impl LlmRuntimeClient {
    pub(crate) fn new(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
    ) -> Self {
        let store = api::load_credentials().unwrap_or_default();
        let registry = ProviderRegistry::from_credentials(&store);
        let provider = if model.is_empty() {
            ProviderClient::no_auth_placeholder()
        } else {
            match registry.build_client(&model, &store) {
                Ok(client) => client,
                Err(e) => {
                    eprintln!("Warning: {e}");
                    ProviderClient::no_auth_placeholder()
                }
            }
        };
        Self {
            registry,
            provider,
            model,
            enable_tools,
            allowed_tools,
            reasoning_effort: None,
        }
    }
}

impl ApiClient for LlmRuntimeClient {
    #[allow(clippy::too_many_lines)]
    fn stream(&mut self, request: runtime::ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let message_request = MessageRequest {
            model: api::provider::model_api_id(&self.model).to_string(),
            max_tokens: self.registry.max_tokens(&self.model),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: self.enable_tools.then(|| {
                filter_tool_specs(self.allowed_tools.as_ref())
                    .into_iter()
                    .map(|spec| ToolDefinition {
                        name: spec.name.to_string(),
                        description: Some(spec.description.to_string()),
                        input_schema: spec.input_schema,
                    })
                    .collect()
            }),
            tool_choice: self.enable_tools.then_some(ToolChoice::Auto),
            stream: true,
            reasoning_effort: self.reasoning_effort,
        };
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut events = Vec::new();
                let mut pending_tool: Option<(String, String, String)> = None;
                let mut saw_stop = false;

                let mut stream = self
                    .provider
                    .stream_message(&message_request)
                    .await
                    .map_err(|error| RuntimeError::new(error.to_string()))?;

                while let Some(event) = stream
                    .next_event()
                    .await
                    .map_err(|error| RuntimeError::new(error.to_string()))?
                {
                    match event {
                        api::StreamEvent::MessageStart(start) => {
                            for block in start.message.content {
                                push_output_block(block, &mut events, &mut pending_tool, true);
                            }
                        }
                        api::StreamEvent::ContentBlockStart(start) => {
                            push_output_block(
                                start.content_block,
                                &mut events,
                                &mut pending_tool,
                                true,
                            );
                        }
                        api::StreamEvent::ContentBlockDelta(delta) => match delta.delta {
                            ContentBlockDelta::TextDelta { text } => {
                                if !text.is_empty() {
                                    events.push(AssistantEvent::TextDelta(text));
                                }
                            }
                            ContentBlockDelta::InputJsonDelta { partial_json } => {
                                if let Some((_, _, input)) = &mut pending_tool {
                                    input.push_str(&partial_json);
                                }
                            }
                        },
                        api::StreamEvent::ContentBlockStop(_) => {
                            if let Some((id, name, input)) = pending_tool.take() {
                                let input = if input.is_empty() {
                                    "{}".to_string()
                                } else {
                                    input
                                };
                                events.push(AssistantEvent::ToolUse { id, name, input });
                            }
                        }
                        api::StreamEvent::MessageDelta(delta) => {
                            events.push(AssistantEvent::Usage(TokenUsage {
                                input_tokens: delta.usage.input_tokens,
                                output_tokens: delta.usage.output_tokens,
                                cache_creation_input_tokens: 0,
                                cache_read_input_tokens: 0,
                            }));
                        }
                        api::StreamEvent::MessageStop(_) => {
                            saw_stop = true;
                            events.push(AssistantEvent::MessageStop);
                        }
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
                if events
                    .iter()
                    .any(|event| matches!(event, AssistantEvent::MessageStop))
                {
                    return Ok(events);
                }
                if self.provider.supports_send_message() {
                    let response = self
                        .provider
                        .send_message(&MessageRequest {
                            stream: false,
                            ..message_request.clone()
                        })
                        .await
                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                    Ok(response_to_events(response))
                } else {
                    Ok(events)
                }
            })
        })
    }
}

pub(crate) fn push_output_block(
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
    }
}

pub(crate) fn response_to_events(response: MessageResponse) -> Vec<AssistantEvent> {
    let mut events = Vec::new();
    let mut pending_tool = None;
    for block in response.content {
        push_output_block(block, &mut events, &mut pending_tool, false);
        if let Some((id, name, input)) = pending_tool.take() {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }
    events.push(AssistantEvent::Usage(TokenUsage {
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
        cache_read_input_tokens: response.usage.cache_read_input_tokens,
    }));
    events.push(AssistantEvent::MessageStop);
    events
}

pub(crate) fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };
            let content = message
                .blocks
                .iter()
                .map(|block| match block {
                    runtime::ContentBlock::Text { text } => {
                        InputContentBlock::Text { text: text.clone() }
                    }
                    runtime::ContentBlock::ToolUse { id, name, input } => {
                        InputContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: serde_json::from_str(input)
                                .unwrap_or_else(|_| json!({ "raw": input })),
                        }
                    }
                    runtime::ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => InputContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![ToolResultContentBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    },
                    runtime::ContentBlock::Reasoning { data } => {
                        let parsed = serde_json::from_str::<serde_json::Value>(data)
                            .unwrap_or_else(|_| json!({}));
                        InputContentBlock::Reasoning { data: parsed }
                    }
                })
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| InputMessage {
                role: role.to_string(),
                content,
            })
        })
        .collect()
}
