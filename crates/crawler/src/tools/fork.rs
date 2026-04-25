use serde_json::Value;

use crate::{ForkSpec, ToolEffect, ToolError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolError> {
    let goal = input
        .get("sub_goal")
        .and_then(Value::as_str)
        .map_or("", str::trim)
        .to_string();
    if goal.is_empty() {
        return Err(ToolError("fork requires non-empty sub_goal".to_string()));
    }

    Ok(ToolEffect::Spawn(ForkSpec {
        goal,
        page_index: None,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fork_returns_spawn_effect() {
        let effect =
            execute(&json!({"sub_goal": "collect details"})).expect("fork should parse sub_goal");
        match effect {
            ToolEffect::Spawn(spec) => {
                assert_eq!(spec.goal, "collect details");
                assert_eq!(spec.page_index, None);
            }
            _ => panic!("expected spawn effect"),
        }
    }

    #[test]
    fn fork_rejects_empty_sub_goal() {
        let error = execute(&json!({"sub_goal": "   "})).expect_err("empty sub_goal should fail");
        assert_eq!(error.to_string(), "fork requires non-empty sub_goal");
    }
}
