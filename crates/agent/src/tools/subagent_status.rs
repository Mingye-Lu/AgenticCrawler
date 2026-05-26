use serde_json::Value;

use crate::{tool_effect::StatusSpec, ToolEffect, ToolExecutionError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let child_ids = match input.get("child_ids") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let array = value.as_array().ok_or_else(|| {
                ToolExecutionError::new("subagent_status child_ids must be an array".to_string())
            })?;
            let ids = array
                .iter()
                .map(|entry| {
                    entry.as_str().map(str::to_owned).ok_or_else(|| {
                        ToolExecutionError::new(
                            "subagent_status child_ids entries must be strings".to_string(),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Some(ids)
        }
    };

    Ok(ToolEffect::Status(StatusSpec { child_ids }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn status_returns_status_effect_with_filter() {
        let effect = execute(&json!({"child_ids": ["c1", "c2"]}))
            .expect("subagent_status should parse child_ids");
        match effect {
            ToolEffect::Status(spec) => {
                assert_eq!(
                    spec.child_ids,
                    Some(vec!["c1".to_string(), "c2".to_string()])
                );
            }
            _ => panic!("expected Status effect"),
        }
    }

    #[test]
    fn status_returns_unfiltered_when_no_child_ids() {
        let effect = execute(&json!({})).expect("status should default to all children");
        match effect {
            ToolEffect::Status(spec) => assert_eq!(spec.child_ids, None),
            _ => panic!("expected Status effect"),
        }
    }

    #[test]
    fn status_rejects_non_string_entries() {
        let err = execute(&json!({"child_ids": [123]})).expect_err("non-string entry should fail");
        assert!(err.to_string().contains("must be strings"));
    }

    #[test]
    fn status_rejects_non_array_child_ids() {
        let err = execute(&json!({"child_ids": "c1"})).expect_err("non-array should fail");
        assert!(err.to_string().contains("must be an array"));
    }
}

