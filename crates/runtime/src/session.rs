use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::json::{JsonError, JsonValue};

pub use acrawl_core::message::{ContentBlock, ConversationMessage, MessageRole, TokenUsage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSession {
    pub id: String,
    pub goal: String,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub version: u32,
    pub model: Option<String>,
    pub title: Option<String>,
    pub messages: Vec<ConversationMessage>,
    pub child_sessions: Vec<ChildSession>,
}

#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Json(JsonError),
    Format(String),
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<JsonError> for SessionError {
    fn from(value: JsonError) -> Self {
        Self::Json(value)
    }
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: 2,
            model: None,
            title: None,
            messages: Vec::new(),
            child_sessions: Vec::new(),
        }
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), SessionError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let temp_path = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&temp_path)?;
            file.write_all(self.to_json().render().as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(temp_path, path)?;
        Ok(())
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let contents = fs::read_to_string(path)?;
        Self::from_json(&JsonValue::parse(&contents)?)
    }

    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        object.insert(
            "version".to_string(),
            JsonValue::Number(i64::from(self.version)),
        );
        if let Some(model) = &self.model {
            object.insert("model".to_string(), JsonValue::String(model.clone()));
        }
        if let Some(title) = &self.title {
            object.insert("title".to_string(), JsonValue::String(title.clone()));
        }
        object.insert(
            "messages".to_string(),
            JsonValue::Array(
                self.messages
                    .iter()
                    .map(conversation_message_to_json)
                    .collect(),
            ),
        );
        object.insert(
            "child_sessions".to_string(),
            JsonValue::Array(
                self.child_sessions
                    .iter()
                    .map(child_session_to_json)
                    .collect(),
            ),
        );
        JsonValue::Object(object)
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("session must be an object".to_string()))?;
        let version = object
            .get("version")
            .and_then(JsonValue::as_i64)
            .ok_or_else(|| SessionError::Format("missing version".to_string()))?;
        let version = u32::try_from(version)
            .map_err(|_| SessionError::Format("version out of range".to_string()))?;
        if version != 1 && version != 2 {
            return Err(SessionError::Format(format!(
                "unsupported session version {version}"
            )));
        }
        let model = object
            .get("model")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned);
        let title = object
            .get("title")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned);
        let messages = object
            .get("messages")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| SessionError::Format("missing messages".to_string()))?
            .iter()
            .map(conversation_message_from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let child_sessions = object
            .get("child_sessions")
            .and_then(JsonValue::as_array)
            .map(|arr| {
                arr.iter()
                    .map(child_session_from_json)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            version,
            model,
            title,
            messages,
            child_sessions,
        })
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

fn conversation_message_to_json(msg: &ConversationMessage) -> JsonValue {
    let mut object = BTreeMap::new();
    object.insert(
        "role".to_string(),
        JsonValue::String(
            match msg.role {
                MessageRole::System => "system",
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::Tool => "tool",
            }
            .to_string(),
        ),
    );
    object.insert(
        "blocks".to_string(),
        JsonValue::Array(msg.blocks.iter().map(content_block_to_json).collect()),
    );
    if let Some(usage) = msg.usage {
        object.insert("usage".to_string(), usage_to_json(usage));
    }
    JsonValue::Object(object)
}

fn conversation_message_from_json(value: &JsonValue) -> Result<ConversationMessage, SessionError> {
    let object = value
        .as_object()
        .ok_or_else(|| SessionError::Format("message must be an object".to_string()))?;
    let role = match object
        .get("role")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| SessionError::Format("missing role".to_string()))?
    {
        "system" => MessageRole::System,
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        other => {
            return Err(SessionError::Format(format!(
                "unsupported message role: {other}"
            )))
        }
    };
    let blocks = object
        .get("blocks")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| SessionError::Format("missing blocks".to_string()))?
        .iter()
        .map(content_block_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    let usage = object.get("usage").map(usage_from_json).transpose()?;
    Ok(ConversationMessage {
        role,
        blocks,
        usage,
    })
}

fn content_block_to_json(block: &ContentBlock) -> JsonValue {
    let mut object = BTreeMap::new();
    match block {
        ContentBlock::Text { text } => {
            object.insert("type".to_string(), JsonValue::String("text".to_string()));
            object.insert("text".to_string(), JsonValue::String(text.clone()));
        }
        ContentBlock::ToolUse { id, name, input } => {
            object.insert(
                "type".to_string(),
                JsonValue::String("tool_use".to_string()),
            );
            object.insert("id".to_string(), JsonValue::String(id.clone()));
            object.insert("name".to_string(), JsonValue::String(name.clone()));
            object.insert("input".to_string(), JsonValue::String(input.clone()));
        }
        ContentBlock::ToolResult {
            tool_use_id,
            tool_name,
            output,
            is_error,
        } => {
            object.insert(
                "type".to_string(),
                JsonValue::String("tool_result".to_string()),
            );
            object.insert(
                "tool_use_id".to_string(),
                JsonValue::String(tool_use_id.clone()),
            );
            object.insert(
                "tool_name".to_string(),
                JsonValue::String(tool_name.clone()),
            );
            object.insert("output".to_string(), JsonValue::String(output.clone()));
            object.insert("is_error".to_string(), JsonValue::Bool(*is_error));
        }
        ContentBlock::Reasoning { data } => {
            object.insert(
                "type".to_string(),
                JsonValue::String("reasoning".to_string()),
            );
            object.insert("data".to_string(), JsonValue::String(data.clone()));
        }
    }
    JsonValue::Object(object)
}

fn content_block_from_json(value: &JsonValue) -> Result<ContentBlock, SessionError> {
    let object = value
        .as_object()
        .ok_or_else(|| SessionError::Format("block must be an object".to_string()))?;
    match object
        .get("type")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| SessionError::Format("missing block type".to_string()))?
    {
        "text" => Ok(ContentBlock::Text {
            text: required_string(object, "text")?,
        }),
        "tool_use" => Ok(ContentBlock::ToolUse {
            id: required_string(object, "id")?,
            name: required_string(object, "name")?,
            input: required_string(object, "input")?,
        }),
        "tool_result" => Ok(ContentBlock::ToolResult {
            tool_use_id: required_string(object, "tool_use_id")?,
            tool_name: required_string(object, "tool_name")?,
            output: required_string(object, "output")?,
            is_error: object
                .get("is_error")
                .and_then(JsonValue::as_bool)
                .ok_or_else(|| SessionError::Format("missing is_error".to_string()))?,
        }),
        "reasoning" => Ok(ContentBlock::Reasoning {
            data: required_string(object, "data")?,
        }),
        other => Err(SessionError::Format(format!(
            "unsupported block type: {other}"
        ))),
    }
}

fn usage_to_json(usage: TokenUsage) -> JsonValue {
    let mut object = BTreeMap::new();
    object.insert(
        "input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.input_tokens)),
    );
    object.insert(
        "output_tokens".to_string(),
        JsonValue::Number(i64::from(usage.output_tokens)),
    );
    object.insert(
        "cache_creation_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_creation_input_tokens)),
    );
    object.insert(
        "cache_read_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_read_input_tokens)),
    );
    JsonValue::Object(object)
}

fn usage_from_json(value: &JsonValue) -> Result<TokenUsage, SessionError> {
    let object = value
        .as_object()
        .ok_or_else(|| SessionError::Format("usage must be an object".to_string()))?;
    Ok(TokenUsage {
        input_tokens: required_u32(object, "input_tokens")?,
        output_tokens: required_u32(object, "output_tokens")?,
        cache_creation_input_tokens: required_u32(object, "cache_creation_input_tokens")?,
        cache_read_input_tokens: required_u32(object, "cache_read_input_tokens")?,
    })
}

fn required_string(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<String, SessionError> {
    object
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))
}

fn required_u32(object: &BTreeMap<String, JsonValue>, key: &str) -> Result<u32, SessionError> {
    let value = object
        .get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))?;
    u32::try_from(value).map_err(|_| SessionError::Format(format!("{key} out of range")))
}

fn child_session_to_json(child: &ChildSession) -> JsonValue {
    let mut object = BTreeMap::new();
    object.insert("id".to_string(), JsonValue::String(child.id.clone()));
    object.insert("goal".to_string(), JsonValue::String(child.goal.clone()));
    object.insert(
        "messages".to_string(),
        JsonValue::Array(
            child
                .messages
                .iter()
                .map(conversation_message_to_json)
                .collect(),
        ),
    );
    JsonValue::Object(object)
}

fn child_session_from_json(value: &JsonValue) -> Result<ChildSession, SessionError> {
    let object = value
        .as_object()
        .ok_or_else(|| SessionError::Format("child_session must be an object".to_string()))?;
    let id = required_string(object, "id")?;
    let goal = required_string(object, "goal")?;
    let messages = object
        .get("messages")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| SessionError::Format("missing messages in child_session".to_string()))?
        .iter()
        .map(conversation_message_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChildSession { id, goal, messages })
}

#[cfg(test)]
mod tests {
    use super::{ChildSession, ContentBlock, ConversationMessage, MessageRole, Session};
    use crate::usage::TokenUsage;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn persists_and_restores_session_json() {
        let mut session = Session::new();
        session
            .messages
            .push(ConversationMessage::user_text("hello"));
        session
            .messages
            .push(ConversationMessage::assistant_with_usage(
                vec![
                    ContentBlock::Text {
                        text: "thinking".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: "echo hi".to_string(),
                    },
                ],
                Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 4,
                    cache_creation_input_tokens: 1,
                    cache_read_input_tokens: 2,
                }),
            ));
        session.messages.push(ConversationMessage::tool_result(
            "tool-1", "bash", "hi", false,
        ));

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("runtime-session-{nanos}.json"));
        session.save_to_path(&path).expect("session should save");
        let restored = Session::load_from_path(&path).expect("session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        assert_eq!(restored, session);
        assert_eq!(restored.messages[2].role, MessageRole::Tool);
        assert_eq!(
            restored.messages[1].usage.expect("usage").total_tokens(),
            17
        );
    }

    #[test]
    fn reasoning_items_survive_session_roundtrip() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::assistant(vec![
            ContentBlock::Reasoning {
                data: r#"{"id":"rs_abc","content":[]}"#.to_string(),
            },
            ContentBlock::Text {
                text: "result".to_string(),
            },
        ]));

        let json = session.to_json();
        let restored = Session::from_json(&json).expect("session should deserialize");

        assert_eq!(restored, session);
        assert!(matches!(
            &restored.messages[0].blocks[0],
            ContentBlock::Reasoning { data } if data == r#"{"id":"rs_abc","content":[]}"#
        ));
        assert!(matches!(
            &restored.messages[0].blocks[1],
            ContentBlock::Text { text } if text == "result"
        ));
    }

    #[test]
    fn rejects_unsupported_session_version() {
        let value =
            crate::json::JsonValue::parse(r#"{"version":999,"messages":[]}"#).expect("valid json");
        let error = Session::from_json(&value).expect_err("version should be rejected");

        assert!(error.to_string().contains("unsupported session version"));
    }

    #[test]
    fn v1_session_loads_with_empty_child_sessions() {
        let json =
            crate::json::JsonValue::parse(r#"{"version":1,"messages":[]}"#).expect("valid json");
        let session = Session::from_json(&json).expect("v1 session should load");
        assert!(session.child_sessions.is_empty());
    }

    #[test]
    fn v2_session_roundtrips_child_sessions() {
        let mut session = Session::new();
        session
            .messages
            .push(ConversationMessage::user_text("hello"));

        let child = ChildSession {
            id: "child-1".to_string(),
            goal: "scrape titles".to_string(),
            messages: vec![ConversationMessage::user_text("child goal")],
        };
        session.child_sessions.push(child);

        let json = session.to_json();
        let restored = Session::from_json(&json).expect("v2 session should roundtrip");

        assert_eq!(restored, session);
        assert_eq!(restored.child_sessions.len(), 1);
        assert_eq!(restored.child_sessions[0].id, "child-1");
        assert_eq!(restored.child_sessions[0].goal, "scrape titles");
        assert_eq!(restored.child_sessions[0].messages.len(), 1);
    }
}
