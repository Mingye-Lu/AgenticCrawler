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
    let data = input.get("data").cloned();

    Ok(ToolEffect::Finish(FinishSpec { summary, data }))
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

    #[test]
    fn done_accepts_data_object() {
        let effect = execute(&json!({"summary": "s", "data": {"key": "val"}}))
            .expect("done should accept object data");
        match effect {
            ToolEffect::Finish(spec) => {
                assert_eq!(spec.summary, "s");
                assert_eq!(spec.data, Some(json!({"key": "val"})));
            }
            _ => panic!("expected finish effect"),
        }
    }

    #[test]
    fn done_has_no_data_when_omitted() {
        let effect = execute(&json!({"summary": "s"})).expect("done should allow missing data");
        match effect {
            ToolEffect::Finish(spec) => {
                assert_eq!(spec.summary, "s");
                assert_eq!(spec.data, None);
            }
            _ => panic!("expected finish effect"),
        }
    }

    #[test]
    fn done_accepts_data_array() {
        let effect = execute(&json!({"summary": "s", "data": [1, 2, 3]}))
            .expect("done should accept array data");
        match effect {
            ToolEffect::Finish(spec) => {
                assert_eq!(spec.summary, "s");
                assert_eq!(spec.data, Some(json!([1, 2, 3])));
            }
            _ => panic!("expected finish effect"),
        }
    }

    #[test]
    fn done_accepts_data_string() {
        let effect = execute(&json!({"summary": "s", "data": "just a string"}))
            .expect("done should accept string data");
        match effect {
            ToolEffect::Finish(spec) => {
                assert_eq!(spec.summary, "s");
                assert_eq!(spec.data, Some(json!("just a string")));
            }
            _ => panic!("expected finish effect"),
        }
    }
}
