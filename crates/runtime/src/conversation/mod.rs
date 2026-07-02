use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::budget::{new_cost_counter, usd_to_millicents, SharedCostCounter};
use crate::compact::{
    compact_session, estimate_session_tokens, CompactionConfig, CompactionResult,
};
use crate::config::RuntimeFeatureConfig;
use crate::control::ControlState;
use crate::observer::RuntimeObserver;
use crate::session::{ContentBlock, ConversationMessage, Session};
use crate::usage::{
    estimate_cost_usd_with_pricing, pricing_for_model, ModelPricing, TokenUsage, UsageTracker,
};

pub use acrawl_core::error::{RuntimeError, ToolError};
pub use acrawl_core::event::AssistantEvent;
pub use acrawl_core::outcome::ToolOutcome;
pub use acrawl_core::traits::ToolExecutor;
pub use acrawl_core::{ApiClient, ApiRequest};
use acrawl_core::ToolEffect;

const DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD: u32 = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummary {
    pub assistant_messages: Vec<ConversationMessage>,
    pub tool_results: Vec<ConversationMessage>,
    pub iterations: usize,
    pub usage: TokenUsage,
    pub auto_compaction: Option<AutoCompactionEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCompactionEvent {
    pub removed_message_count: usize,
}

pub struct ConversationRuntime<C, T> {
    session: Session,
    api_client: C,
    tool_executor: T,
    observer: Option<Box<dyn RuntimeObserver + Send>>,
    system_prompt: Vec<String>,
    max_iterations: usize,
    usage_tracker: UsageTracker,
    auto_compaction_input_tokens_threshold: u32,
    control_state: Arc<ControlState>,
    prompt_override: Arc<Mutex<Option<Vec<String>>>>,
    last_assistant_text: Arc<Mutex<Option<String>>>,
    cumulative_cost: SharedCostCounter,
    model_supports_vision: bool,
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    #[must_use]
    pub fn new(
        session: Session,
        api_client: C,
        tool_executor: T,
        system_prompt: Vec<String>,
        prompt_override: Arc<Mutex<Option<Vec<String>>>>,
        last_assistant_text: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self::new_with_features(
            session,
            api_client,
            tool_executor,
            system_prompt,
            prompt_override,
            last_assistant_text,
            &RuntimeFeatureConfig::default(),
        )
    }

    #[must_use]
    pub fn new_with_features(
        session: Session,
        api_client: C,
        tool_executor: T,
        system_prompt: Vec<String>,
        prompt_override: Arc<Mutex<Option<Vec<String>>>>,
        last_assistant_text: Arc<Mutex<Option<String>>>,
        _feature_config: &RuntimeFeatureConfig,
    ) -> Self {
        let usage_tracker = UsageTracker::from_session(&session);
        let cumulative_cost = new_cost_counter();
        let pricing = session
            .model
            .as_deref()
            .and_then(pricing_for_model)
            .unwrap_or_else(ModelPricing::default_sonnet_tier);
        cumulative_cost.store(
            usd_to_millicents(
                estimate_cost_usd_with_pricing(usage_tracker.cumulative_usage(), pricing)
                    .total_cost_usd(),
            ),
            Ordering::Relaxed,
        );
        Self {
            session,
            api_client,
            tool_executor,
            observer: None,
            system_prompt,
            max_iterations: usize::MAX,
            usage_tracker,
            auto_compaction_input_tokens_threshold: auto_compaction_threshold_from_env(),
            control_state: Arc::new(ControlState::default()),
            prompt_override,
            last_assistant_text,
            cumulative_cost,
            model_supports_vision: false,
        }
    }

    #[must_use]
    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Box<dyn RuntimeObserver + Send>) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn set_observer(&mut self, observer: Box<dyn RuntimeObserver + Send>) {
        self.observer = Some(observer);
    }

    #[must_use]
    pub fn with_auto_compaction_input_tokens_threshold(mut self, threshold: u32) -> Self {
        self.auto_compaction_input_tokens_threshold = threshold;
        self
    }

    #[must_use]
    pub fn with_control_state(mut self, state: Arc<ControlState>) -> Self {
        self.control_state = state;
        self
    }

    #[must_use]
    pub fn with_model_supports_vision(mut self, value: bool) -> Self {
        self.model_supports_vision = value;
        self
    }

    #[must_use]
    pub fn control_state(&self) -> Arc<ControlState> {
        Arc::clone(&self.control_state)
    }

    #[must_use]
    pub fn cancel_flag(&self) -> Arc<ControlState> {
        self.control_state()
    }

    pub fn api_client_mut(&mut self) -> &mut C {
        &mut self.api_client
    }

    pub fn request_cancel(&self) {
        self.control_state.request_cancel();
    }

    fn check_cancel(&self) -> Result<(), RuntimeError> {
        if self.control_state.is_cancelled() {
            self.control_state.reset();
            return Err(RuntimeError::new("interrupted by user"));
        }
        Ok(())
    }

    pub async fn run_turn(
        &mut self,
        user_input: impl Into<String>,
    ) -> Result<TurnSummary, RuntimeError> {
        let result = async {
            self.push_user_message(user_input.into());

            let mut assistant_messages = Vec::new();
            let mut tool_results = Vec::new();
            let mut iterations = 0;

            loop {
                iterations = self.prepare_iteration(iterations)?;

                let assistant_message = self.stream_assistant_message()?;
                let pending_tool_uses = collect_pending_tool_uses(&assistant_message);

                self.store_assistant_message(&mut assistant_messages, assistant_message);

                if pending_tool_uses.is_empty() {
                    break;
                }

                self.execute_pending_tool_uses(&pending_tool_uses, &mut tool_results)
                    .await?;

                self.check_cancel()?;
            }

            let auto_compaction = self.maybe_auto_compact();

            Ok(TurnSummary {
                assistant_messages,
                tool_results,
                iterations,
                usage: self.usage_tracker.cumulative_usage(),
                auto_compaction,
            })
        }
        .await;

        notify_observer_turn_finished(&mut self.observer, &result);
        result
    }

    #[must_use]
    pub fn compact(&self, config: CompactionConfig) -> CompactionResult {
        compact_session(&self.session, config)
    }

    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        estimate_session_tokens(&self.session)
    }

    #[must_use]
    pub fn usage(&self) -> &UsageTracker {
        &self.usage_tracker
    }

    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    #[must_use]
    pub fn into_session(self) -> Session {
        self.session
    }

    pub fn tool_executor_mut(&mut self) -> &mut T {
        &mut self.tool_executor
    }

    #[must_use]
    pub fn cumulative_cost_counter(&self) -> SharedCostCounter {
        Arc::clone(&self.cumulative_cost)
    }

    fn push_user_message(&mut self, user_input: String) {
        self.session
            .messages
            .push(ConversationMessage::user_text(user_input));
    }

    fn prepare_iteration(&mut self, iterations: usize) -> Result<usize, RuntimeError> {
        self.fail_if_cancelled()?;
        self.check_cancel()?;

        if let Some(new_prompt) = self
            .prompt_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            self.system_prompt = new_prompt;
        }

        let next_iterations = iterations + 1;
        if next_iterations > self.max_iterations {
            return Err(RuntimeError::new(
                "conversation loop exceeded the maximum number of iterations",
            ));
        }

        Ok(next_iterations)
    }

    fn fail_if_cancelled(&mut self) -> Result<(), RuntimeError> {
        if self.control_state.is_cancelled() {
            self.control_state.reset();
            return Err(RuntimeError::new("interrupted by user"));
        }

        Ok(())
    }

    fn stream_assistant_message(&mut self) -> Result<ConversationMessage, RuntimeError> {
        let events = self.api_client.stream(self.build_api_request())?;
        notify_observer_about_events(&mut self.observer, &events);
        let (assistant_message, usage) = build_assistant_message(events)?;
        let assistant_text = assistant_text_from_message(&assistant_message);
        *self
            .last_assistant_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(assistant_text);
        if let Some(usage) = usage {
            self.usage_tracker.record(usage);
            let pricing = self
                .session
                .model
                .as_deref()
                .and_then(pricing_for_model)
                .unwrap_or_else(ModelPricing::default_sonnet_tier);
            let cumulative_cost_usd =
                estimate_cost_usd_with_pricing(self.usage_tracker.cumulative_usage(), pricing)
                    .total_cost_usd();
            self.cumulative_cost
                .store(usd_to_millicents(cumulative_cost_usd), Ordering::Relaxed);
        }

        Ok(assistant_message)
    }

    fn build_api_request(&self) -> ApiRequest {
        ApiRequest {
            system_prompt: self.system_prompt.clone(),
            messages: self.session.messages.clone(),
        }
    }

    fn store_assistant_message(
        &mut self,
        assistant_messages: &mut Vec<ConversationMessage>,
        assistant_message: ConversationMessage,
    ) {
        self.session.messages.push(assistant_message.clone());
        if let Some(ref mut observer) = self.observer {
            observer.on_message_completed(&assistant_message);
        }
        assistant_messages.push(assistant_message);
    }

    async fn execute_pending_tool_uses(
        &mut self,
        pending_tool_uses: &[(String, String, String)],
        tool_results: &mut Vec<ConversationMessage>,
    ) -> Result<(), RuntimeError> {
        let mut interrupted = false;

        for (idx, (tool_use_id, tool_name, input)) in pending_tool_uses.iter().enumerate() {
            if interrupted {
                self.store_tool_result(
                    tool_results,
                    interrupted_tool_result(tool_use_id.clone(), tool_name.clone()),
                );
                continue;
            }

            if let Some(result_message) = self
                .execute_tool_use(idx, tool_use_id, tool_name, input)
                .await?
            {
                self.store_tool_result(tool_results, result_message);
            } else {
                interrupted = true;
                self.store_tool_result(
                    tool_results,
                    interrupted_tool_result(tool_use_id.clone(), tool_name.clone()),
                );
            }
        }

        if interrupted {
            self.control_state.reset();
            return Err(RuntimeError::new("interrupted by user"));
        }

        Ok(())
    }

    async fn execute_tool_use(
        &mut self,
        idx: usize,
        tool_use_id: &str,
        tool_name: &str,
        input: &str,
    ) -> Result<Option<ConversationMessage>, RuntimeError> {
        let model_supports_vision = self.model_supports_vision;
        let execute_fut = self.tool_executor.execute(tool_name, input);
        let result_message = tokio::select! {
            biased;
            () = self.control_state.cancelled() => {
                let _ = idx;
                return Ok(None);
            }
            result = execute_fut => build_tool_result_message(&mut self.observer, tool_use_id, tool_name, result, model_supports_vision),
        };

        Ok(Some(result_message))
    }

    fn store_tool_result(
        &mut self,
        tool_results: &mut Vec<ConversationMessage>,
        result_message: ConversationMessage,
    ) {
        notify_observer_tool_result(&mut self.observer, &result_message);
        self.session.messages.push(result_message.clone());
        tool_results.push(result_message);
    }

    fn maybe_auto_compact(&mut self) -> Option<AutoCompactionEvent> {
        if self.usage_tracker.cumulative_usage().input_tokens
            < self.auto_compaction_input_tokens_threshold
        {
            return None;
        }

        let settings = crate::settings::load_settings();
        let config = CompactionConfig {
            max_estimated_tokens: 0,
            preserve_recent_tokens: crate::settings::settings_get_compaction_preserve_recent_tokens(
                &settings,
            ),
            preserve_recent_messages_floor:
                crate::settings::settings_get_compaction_preserve_recent_messages_floor(&settings),
            preserve_recent_messages: CompactionConfig::default().preserve_recent_messages,
            prune_protect_tokens: crate::settings::settings_get_compaction_prune_protect_tokens(
                &settings,
            ),
            prune_max_output_chars: crate::settings::settings_get_compaction_prune_max_output_chars(
                &settings,
            ),
            max_summary_chars: crate::settings::settings_get_compaction_max_summary_chars(
                &settings,
            ),
            llm_summarization: crate::settings::settings_get_compaction_llm_summarization(
                &settings,
            ),
        };

        let mut result = compact_session(&self.session, config);

        if result.removed_message_count == 0 {
            return None;
        }

        if config.llm_summarization {
            let existing_prefix = usize::from(
                self.session
                    .messages
                    .first()
                    .is_some_and(crate::compact::is_compact_continuation_message),
            );
            let removed_end =
                (existing_prefix + result.removed_message_count).min(self.session.messages.len());
            let removed_messages = &self.session.messages[existing_prefix..removed_end];

            if let Some(llm_summary) = try_llm_summarize(
                removed_messages,
                &mut self.api_client,
                self.session.model.as_deref(),
            ) {
                let continuation = crate::compact::get_compact_continuation_message(
                    &llm_summary,
                    true,
                    result.compacted_session.messages.len() > 1,
                );
                if let Some(first_msg) = result.compacted_session.messages.first_mut() {
                    first_msg.blocks = vec![ContentBlock::Text { text: continuation }];
                }
            }
        }

        self.session = result.compacted_session;
        Some(AutoCompactionEvent {
            removed_message_count: result.removed_message_count,
        })
    }
}

fn collect_pending_tool_uses(
    assistant_message: &ConversationMessage,
) -> Vec<(String, String, String)> {
    assistant_message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect()
}

fn interrupted_tool_result(tool_use_id: String, tool_name: String) -> ConversationMessage {
    ConversationMessage::tool_result(
        tool_use_id,
        tool_name,
        "Tool execution interrupted by user.".to_string(),
        true,
    )
}

fn build_tool_result_message(
    observer: &mut Option<Box<dyn RuntimeObserver + Send>>,
    tool_use_id: &str,
    tool_name: &str,
    result: Result<ToolOutcome, ToolError>,
    model_supports_vision: bool,
) -> ConversationMessage {
    let (output, is_error, outcome_effect) = match result {
        Ok(outcome) => {
            if let Some(ref mut observer) = observer {
                if let Some(effect) = outcome.effect.as_ref() {
                    observer.on_tool_effect(effect);
                }
            }
            (outcome.text, false, outcome.effect)
        }
        Err(error) => (error.to_string(), true, None),
    };

    if model_supports_vision {
        if let Some(ToolEffect::Vision(ref payload)) = outcome_effect {
            return ConversationMessage::tool_result_image(
                tool_use_id.to_string(),
                tool_name.to_string(),
                &payload.media_type,
                &payload.base64_data,
                &output,
            );
        }
    }

    ConversationMessage::tool_result(
        tool_use_id.to_string(),
        tool_name.to_string(),
        output,
        is_error,
    )
}

fn try_llm_summarize<C: ApiClient>(
    removed: &[ConversationMessage],
    api_client: &mut C,
    model: Option<&str>,
) -> Option<String> {
    const MAX_SUMMARIZER_TOKENS: usize = 100_000;
    const SUMMARY_TEMPLATE: &str = "Summarize this conversation segment concisely.\n\
Output EXACTLY this structure with no other text:\n\
## Goal\n\
[what the user was trying to accomplish]\n\
## Progress (Done / In Progress)\n\
[what has been completed and what is in progress]\n\
## Key Decisions\n\
[important choices made during the conversation]\n\
## Next Steps\n\
[pending work or what should happen next]\n\
## Relevant Files\n\
[any file paths mentioned, or 'None']";

    let mut context_parts = Vec::new();
    let mut estimated_tokens = 0usize;

    for msg in removed {
        let role = match msg.role {
            crate::session::MessageRole::System => "system",
            crate::session::MessageRole::User => "user",
            crate::session::MessageRole::Assistant => "assistant",
            crate::session::MessageRole::Tool => "tool",
        };
        let text = msg
            .blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => text.as_str(),
                ContentBlock::ToolUse { input, .. } => input.as_str(),
                ContentBlock::ToolResult { output, .. } => output.as_str(),
                ContentBlock::Reasoning { data } => data.as_str(),
                ContentBlock::ToolResultImage { caption, .. } => caption.as_str(),
            })
            .collect::<Vec<_>>()
            .join(" | ");
        let part = format!("[{role}]: {text}");
        let part_tokens = part.len() / 4 + 1;
        if estimated_tokens + part_tokens > MAX_SUMMARIZER_TOKENS {
            break;
        }
        estimated_tokens += part_tokens;
        context_parts.push(part);
    }

    if context_parts.is_empty() {
        return None;
    }

    let user_content = format!(
        "Conversation to summarize:\n\n{}\n\n---\n\n{SUMMARY_TEMPLATE}",
        context_parts.join("\n\n")
    );

    let system_hint = model.map_or_else(
        || "You are summarizing a conversation segment.".to_string(),
        |model| format!("You are summarizing a conversation with model {model}."),
    );

    let events = match api_client.stream(ApiRequest {
        system_prompt: vec![system_hint],
        messages: vec![ConversationMessage::user_text(user_content)],
    }) {
        Ok(events) => events,
        Err(err) => {
            eprintln!(
                "warning: LLM summarization failed ({err}); falling back to mechanical summary"
            );
            return None;
        }
    };

    let llm_summary: String = events
        .iter()
        .filter_map(|event| match event {
            AssistantEvent::TextDelta(text) => Some(text.as_str()),
            AssistantEvent::ToolUse { .. }
            | AssistantEvent::Reasoning { .. }
            | AssistantEvent::Usage(_)
            | AssistantEvent::MessageStop => None,
        })
        .collect();

    let llm_summary = llm_summary.trim();
    if llm_summary.is_empty() {
        eprintln!(
            "warning: LLM summarization returned empty response; falling back to mechanical summary"
        );
        return None;
    }

    let compressed = crate::summary_compression::compress_summary_text(llm_summary);
    if compressed.is_empty() {
        eprintln!(
            "warning: LLM summary was empty after compression; falling back to mechanical summary"
        );
        return None;
    }

    Some(format!("<summary>{compressed}</summary>"))
}

#[must_use]
pub fn auto_compaction_threshold_from_env() -> u32 {
    let settings = crate::settings::load_settings();
    let tokens = crate::settings::settings_get_auto_compact_tokens(&settings);
    u32::try_from(tokens.min(u64::from(u32::MAX)))
        .unwrap_or(DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD)
}

#[cfg(test)]
#[must_use]
fn parse_auto_compaction_threshold(value: Option<&str>) -> u32 {
    value
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|threshold| *threshold > 0)
        .unwrap_or(DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD)
}

fn build_assistant_message(
    events: Vec<AssistantEvent>,
) -> Result<(ConversationMessage, Option<TokenUsage>), RuntimeError> {
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut finished = false;
    let mut usage = None;

    for event in events {
        match event {
            AssistantEvent::TextDelta(delta) => text.push_str(&delta),
            AssistantEvent::ToolUse { id, name, input } => {
                flush_text_block(&mut text, &mut blocks);
                blocks.push(ContentBlock::ToolUse { id, name, input });
            }
            AssistantEvent::Reasoning { data } => {
                flush_text_block(&mut text, &mut blocks);
                blocks.push(ContentBlock::Reasoning { data });
            }
            AssistantEvent::Usage(value) => usage = Some(value),
            AssistantEvent::MessageStop => {
                finished = true;
            }
        }
    }

    flush_text_block(&mut text, &mut blocks);

    if !finished {
        return Err(RuntimeError::new(
            "assistant stream ended without a message stop event",
        ));
    }
    if blocks.is_empty() {
        return Err(RuntimeError::new("assistant stream produced no content"));
    }

    Ok((
        ConversationMessage::assistant_with_usage(blocks, usage),
        usage,
    ))
}

fn assistant_text_from_message(message: &ConversationMessage) -> String {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::ToolUse { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Reasoning { .. }
            | ContentBlock::ToolResultImage { .. } => None,
        })
        .collect()
}

fn notify_observer_about_events(
    observer: &mut Option<Box<dyn RuntimeObserver + Send>>,
    events: &[AssistantEvent],
) {
    let Some(observer) = observer.as_mut() else {
        return;
    };

    for event in events {
        match event {
            AssistantEvent::TextDelta(delta) => observer.on_text_delta(delta),
            AssistantEvent::ToolUse { id, name, input } => {
                observer.on_tool_call_start(id, name, input);
            }
            AssistantEvent::Usage(usage) => observer.on_usage(usage),
            AssistantEvent::Reasoning { .. } | AssistantEvent::MessageStop => {}
        }
    }
}

fn notify_observer_tool_result(
    observer: &mut Option<Box<dyn RuntimeObserver + Send>>,
    result_message: &ConversationMessage,
) {
    let Some(observer) = observer.as_mut() else {
        return;
    };

    for block in &result_message.blocks {
        if let ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } = block
        {
            observer.on_tool_result(tool_name, output, *is_error);
        }
    }
    observer.on_message_completed(result_message);
}

fn notify_observer_turn_finished(
    observer: &mut Option<Box<dyn RuntimeObserver + Send>>,
    result: &Result<TurnSummary, RuntimeError>,
) {
    if let Some(observer) = observer.as_mut() {
        let observer_result = result.as_ref().map(|_| ()).map_err(ToString::to_string);
        observer.on_turn_finished(&observer_result);
    }
}

fn flush_text_block(text: &mut String, blocks: &mut Vec<ContentBlock>) {
    if !text.is_empty() {
        blocks.push(ContentBlock::Text {
            text: std::mem::take(text),
        });
    }
}

type ToolHandler = Box<dyn FnMut(&str) -> Result<ToolOutcome, ToolError> + Send>;

#[derive(Default)]
pub struct StaticToolExecutor {
    handlers: BTreeMap<String, ToolHandler>,
}

impl StaticToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn register(
        mut self,
        tool_name: impl Into<String>,
        handler: impl FnMut(&str) -> Result<ToolOutcome, ToolError> + Send + 'static,
    ) -> Self {
        self.handlers.insert(tool_name.into(), Box::new(handler));
        self
    }
}

impl ToolExecutor for StaticToolExecutor {
    #[allow(clippy::manual_async_fn)]
    fn execute(
        &mut self,
        tool_name: &str,
        input: &str,
    ) -> impl std::future::Future<Output = Result<ToolOutcome, ToolError>> + Send {
        async move {
            self.handlers
                .get_mut(tool_name)
                .ok_or_else(|| ToolError::new(format!("unknown tool: {tool_name}")))?(
                input
            )
        }
    }
}

#[cfg(test)]
mod tests;
