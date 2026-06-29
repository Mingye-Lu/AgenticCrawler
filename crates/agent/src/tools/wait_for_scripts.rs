use serde_json::Value;

use acrawl_core::script_types::ScriptWaitSpec;

use crate::{ToolEffect, ToolExecutionError};

/// Parse the LLM's `wait_for_scripts` tool input and return a `ScriptWait` effect.
///
/// Input: `{ "script_ids": ["id1", "id2"] }` or `{}` (wait for all).
pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let script_ids = input
        .get("script_ids")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| {
                    ToolExecutionError::new(
                        "wait_for_scripts script_ids must be an array".to_string(),
                    )
                })
                .and_then(|values| {
                    values
                        .iter()
                        .map(|v| {
                            v.as_str().map(str::to_owned).ok_or_else(|| {
                                ToolExecutionError::new(
                                    "wait_for_scripts script_ids entries must be strings"
                                        .to_string(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
        })
        .transpose()?;

    Ok(ToolEffect::ScriptWait(ScriptWaitSpec { script_ids }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wait_for_scripts_with_ids() {
        let effect =
            execute(&json!({"script_ids": ["s1", "s2"]})).expect("should parse script_ids");
        match effect {
            ToolEffect::ScriptWait(spec) => {
                assert_eq!(
                    spec.script_ids,
                    Some(vec!["s1".to_string(), "s2".to_string()])
                );
            }
            _ => panic!("expected ScriptWait effect"),
        }
    }

    #[test]
    fn wait_for_scripts_defaults_to_all() {
        let effect = execute(&json!({})).expect("should default to all scripts");
        match effect {
            ToolEffect::ScriptWait(spec) => assert_eq!(spec.script_ids, None),
            _ => panic!("expected ScriptWait effect"),
        }
    }

    #[test]
    fn wait_for_scripts_rejects_non_array() {
        let err = execute(&json!({"script_ids": "not-an-array"}))
            .expect_err("should fail with non-array");
        assert!(err.to_string().contains("must be an array"));
    }

    #[test]
    fn wait_for_scripts_rejects_non_string_entries() {
        let err = execute(&json!({"script_ids": [123]}))
            .expect_err("should fail with non-string entries");
        assert!(err.to_string().contains("must be strings"));
    }
}
