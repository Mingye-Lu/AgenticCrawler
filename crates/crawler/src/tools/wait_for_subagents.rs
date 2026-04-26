use serde_json::Value;

use crate::{ToolEffect, ToolError, WaitSpec};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolError> {
    let child_ids = input
        .get("child_ids")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| {
                    ToolError("wait_for_subagents child_ids must be an array".to_string())
                })
                .and_then(|values| {
                    values
                        .iter()
                        .map(|value| {
                            value.as_str().map(str::to_owned).ok_or_else(|| {
                                ToolError(
                                    "wait_for_subagents child_ids entries must be strings"
                                        .to_string(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
        })
        .transpose()?;

    Ok(ToolEffect::Wait(WaitSpec { child_ids }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wait_returns_wait_effect() {
        let effect = execute(&json!({"child_ids": ["child-1", "child-2"]}))
            .expect("wait should parse child_ids");
        match effect {
            ToolEffect::Wait(spec) => {
                assert_eq!(
                    spec.child_ids,
                    Some(vec!["child-1".to_string(), "child-2".to_string()])
                );
            }
            _ => panic!("expected wait effect"),
        }
    }

    #[test]
    fn wait_defaults_to_all_children() {
        let effect = execute(&json!({})).expect("wait should default to all children");
        match effect {
            ToolEffect::Wait(spec) => assert_eq!(spec.child_ids, None),
            _ => panic!("expected wait effect"),
        }
    }
}
