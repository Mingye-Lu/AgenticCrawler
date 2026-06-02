use std::sync::{Arc, Mutex};

use api::provider::{model_api_id, ProviderRegistry};
use api::{ContentBlockDelta, InputContentBlock, InputMessage, MessageRequest, StreamEvent};

const TITLE_SYSTEM_PROMPT: &str =
    "You generate short titles for chat sessions. Respond with a 3-5 word title that captures \
     the user's request. No quotes, no trailing punctuation, no preamble.";

const MAX_TITLE_LEN: usize = 60;

pub fn spawn_title_generation(
    model: String,
    user_message: String,
    slot: Arc<Mutex<Option<String>>>,
) {
    let Some(rt) = crate::TOKIO_RUNTIME.get() else {
        return;
    };
    rt.spawn(async move {
        if let Some(title) = generate_title(&model, &user_message).await {
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(title);
            }
        }
    });
}

async fn generate_title(model: &str, user_message: &str) -> Option<String> {
    if model.is_empty() {
        return None;
    }
    let store = api::load_credentials().ok()?;
    let registry = ProviderRegistry::from_credentials(&store);
    let provider = registry.build_client(model, &store).ok()?;

    let request = MessageRequest {
        model: model_api_id(model).to_string(),
        max_tokens: 64,
        messages: vec![InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text {
                text: format!("First user message:\n\n{user_message}"),
            }],
        }],
        system: Some(TITLE_SYSTEM_PROMPT.to_string()),
        tools: None,
        tool_choice: None,
        stream: true,
        reasoning_effort: None,
    };

    let mut text = String::new();
    let mut stream = provider.stream_message(&request).await.ok()?;
    while let Ok(Some(event)) = stream.next_event().await {
        match event {
            StreamEvent::ContentBlockDelta(delta) => {
                if let ContentBlockDelta::TextDelta { text: chunk } = delta.delta {
                    text.push_str(&chunk);
                }
            }
            StreamEvent::ContentBlockStart(start) => {
                if let api::OutputContentBlock::Text { text: chunk } = start.content_block {
                    text.push_str(&chunk);
                }
            }
            StreamEvent::MessageStop(_) => break,
            _ => {}
        }
    }

    sanitize_title(&text)
}

fn sanitize_title(raw: &str) -> Option<String> {
    let first_line = raw.lines().find(|l| !l.trim().is_empty())?;
    let cleaned: String = first_line
        .trim()
        .trim_matches(['"', '\'', '*', '#', '`'])
        .trim_end_matches(['.', ',', ';', ':'])
        .chars()
        .take(MAX_TITLE_LEN)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_title;

    #[test]
    fn strips_quotes_and_punctuation() {
        assert_eq!(
            sanitize_title("\"Refactor session storage.\""),
            Some("Refactor session storage".to_string())
        );
    }

    #[test]
    fn takes_first_non_empty_line() {
        assert_eq!(
            sanitize_title("\n\n  Build lazy sessions  \nextra"),
            Some("Build lazy sessions".to_string())
        );
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(sanitize_title("   \n  "), None);
    }

    #[test]
    fn truncates_long_titles() {
        let long = "a".repeat(200);
        let out = sanitize_title(&long).unwrap();
        assert_eq!(out.len(), 60);
    }
}
