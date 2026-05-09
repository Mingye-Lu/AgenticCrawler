use serde_json::Value;

use crate::{ToolEffect, ToolError};

#[derive(Debug)]
struct WaitForHumanInput {
    reason: String,
}

fn parse_input(input: &Value) -> Result<WaitForHumanInput, crate::CrawlError> {
    let reason = input
        .get("reason")
        .and_then(Value::as_str)
        .ok_or_else(|| crate::CrawlError::new("missing required field: reason"))?;

    if reason.is_empty() {
        return Err(crate::CrawlError::new("reason must not be empty"));
    }

    Ok(WaitForHumanInput {
        reason: reason.to_string(),
    })
}

pub fn execute(input: &Value) -> Result<ToolEffect, ToolError> {
    let params = parse_input(input)?;
    // Actual blocking pause/resume logic will be wired in Task 9.
    // Return Pause effect — dispatch_tool_effect handles it.
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
}
