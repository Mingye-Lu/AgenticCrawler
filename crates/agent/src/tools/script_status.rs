use serde_json::Value;

use acrawl_core::script_types::ScriptStatusSpec;

use crate::{ToolEffect, ToolExecutionError};

/// Parse the LLM's `script_status` tool input and return a `ScriptStatus` effect.
///
/// Input: `{ "script_id": "<id>" }`
pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let script_id = input
        .get("script_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolExecutionError::new("script_status requires non-empty `script_id`".to_string())
        })?
        .to_string();

    Ok(ToolEffect::ScriptStatus(ScriptStatusSpec { script_id }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn script_status_returns_effect() {
        let effect = execute(&json!({"script_id": "abc-123"})).expect("should parse script_id");
        match effect {
            ToolEffect::ScriptStatus(spec) => {
                assert_eq!(spec.script_id, "abc-123");
            }
            _ => panic!("expected ScriptStatus effect"),
        }
    }

    #[test]
    fn script_status_rejects_missing_id() {
        let err = execute(&json!({})).expect_err("should fail without script_id");
        assert!(err.to_string().contains("script_id"));
    }

    #[test]
    fn script_status_rejects_empty_id() {
        let err =
            execute(&json!({"script_id": "  "})).expect_err("should fail with empty script_id");
        assert!(err.to_string().contains("script_id"));
    }
}
