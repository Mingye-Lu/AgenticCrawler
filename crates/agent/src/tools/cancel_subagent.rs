use serde_json::Value;

use crate::{tool_effect::CancelSpec, ToolEffect, ToolExecutionError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let raw_ids = input
        .get("child_ids")
        .ok_or_else(|| ToolExecutionError::new("cancel_subagent requires child_ids".to_string()))?
        .as_array()
        .ok_or_else(|| {
            ToolExecutionError::new("cancel_subagent child_ids must be an array".to_string())
        })?;

    if raw_ids.is_empty() {
        return Err(ToolExecutionError::new(
            "cancel_subagent child_ids must not be empty".to_string(),
        ));
    }

    let child_ids = raw_ids
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                ToolExecutionError::new(
                    "cancel_subagent child_ids entries must be strings".to_string(),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let reason = input
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(ToolEffect::Cancel(CancelSpec { child_ids, reason }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn cancel_returns_cancel_effect() {
        let effect = execute(&json!({"child_ids": ["c1", "c2"], "reason": "timed out goal"}))
            .expect("cancel_subagent should parse");
        match effect {
            ToolEffect::Cancel(spec) => {
                assert_eq!(spec.child_ids, vec!["c1".to_string(), "c2".to_string()]);
                assert_eq!(spec.reason.as_deref(), Some("timed out goal"));
            }
            _ => panic!("expected Cancel effect"),
        }
    }

    #[test]
    fn cancel_rejects_missing_child_ids() {
        let err = execute(&json!({})).expect_err("cancel_subagent without child_ids should fail");
        assert!(err.to_string().contains("requires child_ids"));
    }

    #[test]
    fn cancel_rejects_empty_child_ids() {
        let err = execute(&json!({"child_ids": []})).expect_err("empty child_ids should fail");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn cancel_rejects_non_string_entries() {
        let err = execute(&json!({"child_ids": [42]})).expect_err("non-string entry should fail");
        assert!(err.to_string().contains("must be strings"));
    }
}
