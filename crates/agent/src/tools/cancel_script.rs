use serde_json::Value;

use acrawl_core::script_types::ScriptCancelSpec;

use crate::{ToolEffect, ToolExecutionError};

/// Parse the LLM's `cancel_script` tool input and return a `ScriptCancel` effect.
///
/// Input: `{ "script_id": "<id>" }`
pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let script_id = input
        .get("script_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolExecutionError::new("cancel_script requires non-empty `script_id`".to_string())
        })?
        .to_string();

    Ok(ToolEffect::ScriptCancel(ScriptCancelSpec { script_id }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn cancel_script_returns_effect() {
        let effect = execute(&json!({"script_id": "xyz-789"})).expect("should parse script_id");
        match effect {
            ToolEffect::ScriptCancel(spec) => {
                assert_eq!(spec.script_id, "xyz-789");
            }
            _ => panic!("expected ScriptCancel effect"),
        }
    }

    #[test]
    fn cancel_script_rejects_missing_id() {
        let err = execute(&json!({})).expect_err("should fail without script_id");
        assert!(err.to_string().contains("script_id"));
    }

    #[test]
    fn cancel_script_rejects_empty_id() {
        let err = execute(&json!({"script_id": ""})).expect_err("should fail with empty script_id");
        assert!(err.to_string().contains("script_id"));
    }
}
