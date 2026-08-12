use std::io;
use std::sync::Arc;

use super::*;
use crate::CliOutputFormat;
use render::format::format_auto_compaction_notice;
use render::markdown::{Spinner, TerminalRenderer};
use serde_json::json;

impl LiveCli {
    pub fn run_turn_tui(&mut self, input: &str) -> Result<(), CliError> {
        self.maybe_dispatch_title_generation(input);
        if let Some(tx) = self.event_sender() {
            let _ = tx.send(ReplTuiEvent::TurnStarting);
        }
        let result = block_on_runtime_future(self.runtime.run_turn(input));
        let finish: Result<(), String> = match &result {
            Ok(summary) => {
                self.capture_child_sessions();
                if let Some(ev) = summary.auto_compaction {
                    let msg = format_auto_compaction_notice(ev.removed_message_count);
                    if let Some(tx) = self.event_sender() {
                        let _ = tx.send(ReplTuiEvent::SystemMessage(msg));
                    }
                }
                self.persist_session().map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(_) => finish.map_err(std::convert::Into::into),
            Err(e) => Err(e.into()),
        }
    }

    pub fn run_turn(&mut self, input: &str) -> Result<(), CliError> {
        self.maybe_dispatch_title_generation(input);
        let mut spinner = Spinner::new();
        let mut stdout = io::stdout();
        spinner.tick(
            "🕷️Thinking...",
            TerminalRenderer::new().color_theme(),
            &mut stdout,
        )?;
        let result = block_on_runtime_future(self.runtime.run_turn(input));
        match result {
            Ok(summary) => {
                self.capture_child_sessions();
                spinner.finish("✅Done", TerminalRenderer::new().color_theme(), &mut stdout)?;
                println!();
                if let Some(event) = summary.auto_compaction {
                    println!(
                        "{}",
                        format_auto_compaction_notice(event.removed_message_count)
                    );
                }
                self.persist_session()?;
                Ok(())
            }
            Err(error) => {
                spinner.fail(
                    "❌Request failed",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                Err(CliError::from(error))
            }
        }
    }

    pub fn run_turn_with_output(
        &mut self,
        input: &str,
        output_format: CliOutputFormat,
    ) -> Result<(), CliError> {
        match output_format {
            CliOutputFormat::Text => self.run_turn(input),
            CliOutputFormat::Json => self.run_prompt_json(input),
        }
    }

    fn run_prompt_json(&mut self, input: &str) -> Result<(), CliError> {
        self.maybe_dispatch_title_generation(input);
        let session = self.runtime.session().clone();
        let mut runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            self.allowed_tools.clone(),
            self.output_mode.observer(),
        )?;
        let summary = block_on_runtime_future(runtime.run_turn(input))?;
        capture_child_sessions_into_session(&mut runtime);
        self.runtime = runtime;
        self.persist_session()?;
        println!(
            "{}",
            json!({
                "message": final_assistant_text(&summary),
                "model": self.model,
                "iterations": summary.iterations,
                "auto_compaction": summary.auto_compaction.map(|event| json!({
                    "removed_messages": event.removed_message_count,
                    "notice": format_auto_compaction_notice(event.removed_message_count),
                })),
                "tool_uses": collect_tool_uses(&summary),
                "tool_results": collect_tool_results(&summary),
                "usage": {
                    "input_tokens": summary.usage.input_tokens,
                    "output_tokens": summary.usage.output_tokens,
                    "cache_creation_input_tokens": summary.usage.cache_creation_input_tokens,
                    "cache_read_input_tokens": summary.usage.cache_read_input_tokens,
                }
            })
        );
        Ok(())
    }

    pub(super) fn capture_child_sessions(&mut self) {
        capture_child_sessions_into_session(&mut self.runtime);
    }

    pub(super) fn maybe_dispatch_title_generation(&mut self, user_input: &str) {
        if self.title_dispatched {
            return;
        }
        if self.runtime.session().title.is_some() {
            self.title_dispatched = true;
            return;
        }
        if !self.runtime.session().messages.is_empty() {
            self.title_dispatched = true;
            return;
        }
        let trimmed = user_input.trim();
        if trimmed.is_empty() {
            return;
        }
        self.title_dispatched = true;
        title_namer::spawn_title_generation(
            self.model.clone(),
            trimmed.to_string(),
            Arc::clone(&self.pending_title),
        );
    }
}

fn capture_child_sessions_into_session(
    runtime: &mut ConversationRuntime<LlmRuntimeClient, CliToolExecutor>,
) {
    let child_sessions = runtime.tool_executor_mut().take_captured_child_sessions();
    super::session::merge_child_sessions(runtime.session_mut(), child_sessions);
}

fn final_assistant_text(summary: &runtime::TurnSummary) -> String {
    summary
        .assistant_messages
        .last()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    runtime::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn collect_tool_uses(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            runtime::ContentBlock::ToolUse { id, name, input } => {
                Some(json!({"id": id, "name": name, "input": input}))
            }
            _ => None,
        })
        .collect()
}

fn collect_tool_results(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .tool_results
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            runtime::ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } => Some(json!({
                "tool_use_id": tool_use_id,
                "tool_name": tool_name,
                "output": output,
                "is_error": is_error
            })),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::final_assistant_text;
    use runtime::{ContentBlock, ConversationMessage};

    #[test]
    fn final_assistant_text_returns_joined_text_from_last_assistant_message() {
        let summary = runtime::TurnSummary {
            assistant_messages: vec![
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "ignored".to_string(),
                }]),
                ConversationMessage::assistant(vec![
                    ContentBlock::Text {
                        text: "hello".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "navigate".to_string(),
                        input: "{}".to_string(),
                    },
                    ContentBlock::Text {
                        text: " world".to_string(),
                    },
                ]),
            ],
            tool_results: vec![],
            iterations: 2,
            usage: runtime::TokenUsage::default(),
            auto_compaction: None,
        };

        assert_eq!(final_assistant_text(&summary), "hello world");
    }
}
