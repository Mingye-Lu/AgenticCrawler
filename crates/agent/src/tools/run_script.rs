use serde_json::Value;

use acrawl_core::script_types::{ScriptLimits, ScriptTask};

use crate::{ToolEffect, ToolExecutionError};

/// Parse the LLM's `run_script` tool input and return a `RunScript` effect.
///
/// Either `script` (inline JSON) or `name` (load from disk) must be present.
/// If both are missing, an error is returned. The actual script loading from
/// disk (when `name` is used) happens downstream in the effect dispatcher,
/// not here.
pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let script_value = input.get("script").cloned();
    let name = input.get("name").and_then(Value::as_str).map(str::to_owned);
    let save_as = input
        .get("save_as")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let script = match (script_value, &name) {
        (Some(s), _) => s,
        (None, Some(n)) => {
            serde_json::json!({ "__load_from_disk": n })
        }
        (None, None) => {
            return Err(ToolExecutionError::new(
                "run_script requires either `script` or `name`".to_string(),
            ));
        }
    };

    let limits = input
        .get("limits")
        .map(|v| {
            serde_json::from_value::<ScriptLimits>(v.clone())
                .map_err(|e| ToolExecutionError::new(format!("run_script: invalid limits: {e}")))
        })
        .transpose()?
        .unwrap_or(ScriptLimits {
            max_steps: 200,
            max_timeout_secs: 300,
            max_output_bytes: 10_485_760,
            max_parallel_branches: 10,
            per_step_timeout_secs: 30,
            max_script_size_bytes: 1_048_576,
            max_nesting_depth: 10,
        });

    Ok(ToolEffect::RunScript(ScriptTask {
        script,
        save_as,
        limits,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn run_script_with_inline_script() {
        let effect = execute(&json!({
            "script": { "steps": [{"navigate": "https://example.com"}] }
        }))
        .expect("should parse inline script");
        match effect {
            ToolEffect::RunScript(task) => {
                assert_eq!(
                    task.script,
                    json!({"steps": [{"navigate": "https://example.com"}]})
                );
                assert_eq!(task.save_as, None);
                assert_eq!(task.limits.max_steps, 200);
            }
            _ => panic!("expected RunScript effect"),
        }
    }

    #[test]
    fn run_script_with_name() {
        let effect = execute(&json!({
            "name": "my-scraper"
        }))
        .expect("should parse name reference");
        match effect {
            ToolEffect::RunScript(task) => {
                assert_eq!(task.script, json!({"__load_from_disk": "my-scraper"}));
            }
            _ => panic!("expected RunScript effect"),
        }
    }

    #[test]
    fn run_script_with_save_as_and_limits() {
        let effect = execute(&json!({
            "script": {"steps": []},
            "save_as": "fast-script",
            "limits": {
                "max_steps": 50,
                "max_timeout_secs": 60,
                "max_output_bytes": 1024,
                "max_parallel_branches": 2,
                "per_step_timeout_secs": 10,
                "max_script_size_bytes": 512,
                "max_nesting_depth": 3
            }
        }))
        .expect("should parse with limits");
        match effect {
            ToolEffect::RunScript(task) => {
                assert_eq!(task.save_as, Some("fast-script".to_string()));
                assert_eq!(task.limits.max_steps, 50);
                assert_eq!(task.limits.max_timeout_secs, 60);
                assert_eq!(task.limits.max_nesting_depth, 3);
            }
            _ => panic!("expected RunScript effect"),
        }
    }

    #[test]
    fn run_script_rejects_missing_script_and_name() {
        let err = execute(&json!({})).expect_err("should fail without script or name");
        assert!(err.to_string().contains("requires either"));
    }
}
