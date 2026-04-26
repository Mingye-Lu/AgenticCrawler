use serde_json::Value;

use crate::{FinishSpec, ToolEffect, ToolError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolError> {
    let summary = match input.get("summary") {
        Some(value) => value
            .as_str()
            .ok_or_else(|| ToolError("done summary must be a string".to_string()))?
            .to_string(),
        None => "Task complete".to_string(),
    };

    Ok(ToolEffect::Finish(FinishSpec { summary }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn done_returns_finish_effect() {
        let effect = execute(&json!({"summary": "Completed"})).expect("done should parse summary");
        match effect {
            ToolEffect::Finish(spec) => assert_eq!(spec.summary, "Completed"),
            _ => panic!("expected finish effect"),
        }
    }

    #[test]
    fn done_defaults_summary() {
        let effect = execute(&json!({})).expect("done should default summary");
        match effect {
            ToolEffect::Finish(spec) => assert_eq!(spec.summary, "Task complete"),
            _ => panic!("expected finish effect"),
        }
    }
}
