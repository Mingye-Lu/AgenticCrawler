use serde_json::Value;

use crate::{ToolEffect, ToolExecutionError};

#[derive(Debug)]
struct WaitForHumanInput {
    reason: String,
}

fn parse_input(input: &Value) -> Result<WaitForHumanInput, crate::CrawlError> {
    let reason = input
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| crate::CrawlError::new("missing required field: reason"))?;

    if reason.is_empty() {
        return Err(crate::CrawlError::new("reason must not be empty"));
    }

    Ok(WaitForHumanInput {
        reason: reason.to_string(),
    })
}

pub fn execute(input: &Value, is_interactive: bool) -> Result<ToolEffect, ToolExecutionError> {
    if !is_interactive {
        return Err(ToolExecutionError::new(
            "wait_for_human is only available in an interactive TUI session \
             (not in `prompt` or `--resume`)."
                .to_string(),
        ));
    }
    let params = parse_input(input)?;
    Ok(ToolEffect::Pause {
        reason: params.reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_reason() {
        let input = json!({"reason": "captcha detected"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.reason, "captcha detected");
    }

    #[test]
    fn parse_missing_reason_returns_error() {
        let input = json!({});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("reason"));
    }

    #[test]
    fn parse_empty_reason_returns_error() {
        let input = json!({"reason": ""});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn wait_for_human_non_interactive_errors() {
        let input = json!({"reason": "captcha detected"});
        let result = super::execute(&input, false);
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("interactive TUI"),
            "expected interactive-TUI error, got: {err}"
        );
    }

    #[test]
    fn wait_for_human_returns_pause() {
        let input = json!({"reason": "captcha"});
        let result = super::execute(&input, true);
        match result.unwrap() {
            ToolEffect::Pause { reason } => assert_eq!(reason, "captcha"),
            other => panic!("expected Pause, got {other:?}"),
        }
    }

    #[test]
    fn parse_whitespace_only_reason_returns_error() {
        let input = json!({"reason": "   "});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }
}

