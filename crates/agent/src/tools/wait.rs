use std::time::Duration;

use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

use super::feedback::InteractionKind;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 300_000;
const MAX_TIMEOUT_SECONDS: f64 = 300.0;

#[derive(Debug)]
pub struct WaitInput {
    pub selector: Option<String>,
    pub timeout_ms: u64,
    pub state: Option<String>,
    pub silent: bool,
    pub text_pattern: Option<String>,
}

pub fn parse_input(input: &Value) -> Result<WaitInput, CrawlError> {
    let selector = input
        .get("selector")
        .and_then(|v| v.as_str())
        .map(String::from);

    let timeout_ms = parse_timeout_ms(input)?;

    let state = input
        .get("state")
        .and_then(|v| v.as_str())
        .map(String::from);

    if let Some(ref s) = state {
        let valid = ["visible", "hidden", "attached", "detached"];
        if !valid.contains(&s.as_str()) {
            return Err(CrawlError::new(format!(
                "wait state must be one of: visible, hidden, attached, detached (got: {s})"
            )));
        }
    }

    if state.is_some() && selector.is_none() {
        return Err(CrawlError::new(
            "wait state requires a selector (state has no effect with a time-only wait)",
        ));
    }

    let text_pattern = input
        .get("text_pattern")
        .and_then(|v| v.as_str())
        .map(String::from);

    if text_pattern.is_some() && state.is_some() {
        return Err(CrawlError::new(
            "wait text_pattern and state are mutually exclusive",
        ));
    }

    if selector.is_none() && timeout_ms == 0 {
        return Err(CrawlError::new(
            "wait requires either a selector or a positive timeout",
        ));
    }

    let silent = input
        .get("silent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    Ok(WaitInput {
        selector,
        timeout_ms,
        state,
        silent,
        text_pattern,
    })
}

fn parse_timeout_ms(input: &Value) -> Result<u64, CrawlError> {
    if let Some(timeout_ms) = input.get("timeout_ms").and_then(serde_json::Value::as_u64) {
        if timeout_ms > MAX_TIMEOUT_MS {
            return Err(CrawlError::new(format!(
                "wait timeout_ms must be <= {MAX_TIMEOUT_MS}"
            )));
        }
        return Ok(timeout_ms);
    }

    let Some(seconds) = input.get("seconds") else {
        return Ok(DEFAULT_TIMEOUT_MS);
    };
    let seconds = seconds
        .as_f64()
        .ok_or_else(|| CrawlError::new("wait seconds must be a number"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(CrawlError::new(
            "wait seconds must be a finite non-negative number",
        ));
    }

    if seconds > MAX_TIMEOUT_SECONDS {
        return Err(CrawlError::new(format!(
            "wait seconds must be <= {}",
            MAX_TIMEOUT_MS / 1000
        )));
    }

    let millis = seconds * 1000.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(millis as u64)
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let parsed = parse_input(input)?;

    if let Some(ref text_pattern) = parsed.text_pattern {
        let found = browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .wait_for_text(parsed.selector.as_deref(), text_pattern, parsed.timeout_ms)
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;

        let page_state = super::feedback::post_action_page_state(
            browser,
            crawl_state,
            InteractionKind::Passive,
            None,
            false,
        )
        .await?;

        Ok(ToolEffect::reply_json(&json!({
            "success": true,
            "found": found,
            "text_pattern": text_pattern,
            "selector": parsed.selector,
            "timeout_ms": parsed.timeout_ms,
            "page_state": page_state
        })))
    } else if let Some(ref selector) = parsed.selector {
        let found = browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .wait_for_selector(selector, parsed.timeout_ms, parsed.state.as_deref())
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;

        let page_state = super::feedback::post_action_page_state(
            browser,
            crawl_state,
            InteractionKind::Passive,
            None,
            false,
        )
        .await?;

        Ok(ToolEffect::reply_json(&json!({
            "success": true,
            "found": found,
            "selector": selector,
            "timeout_ms": parsed.timeout_ms,
            "page_state": page_state
        })))
    } else {
        tokio::time::sleep(Duration::from_millis(parsed.timeout_ms)).await;

        if parsed.silent {
            return Ok(ToolEffect::reply_json(&json!({
                "success": true,
                "waited_ms": parsed.timeout_ms
            })));
        }

        let page_state = super::feedback::post_action_page_state(
            browser,
            crawl_state,
            InteractionKind::Passive,
            None,
            false,
        )
        .await?;

        Ok(ToolEffect::reply_json(&json!({
            "success": true,
            "waited_ms": parsed.timeout_ms,
            "page_state": page_state
        })))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_selector_and_timeout() {
        let input = json!({"selector": "#btn", "timeout_ms": 3000});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.selector.as_deref(), Some("#btn"));
        assert_eq!(parsed.timeout_ms, 3000);
    }

    #[test]
    fn converts_seconds_to_ms() {
        let input = json!({"selector": ".item", "seconds": 2.5});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.timeout_ms, 2500);
    }

    #[test]
    fn defaults_timeout_to_5000() {
        let input = json!({"selector": "div"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.timeout_ms, 5000);
    }

    #[test]
    fn allows_timeout_only() {
        let input = json!({"timeout_ms": 1000});
        let parsed = parse_input(&input).unwrap();
        assert!(parsed.selector.is_none());
        assert_eq!(parsed.timeout_ms, 1000);
    }

    #[test]
    fn rejects_invalid_seconds() {
        let input = json!({"seconds": -1.0});
        assert!(parse_input(&input).is_err());

        let input = json!({"seconds": 301.0});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn parses_valid_state() {
        for state in &["visible", "hidden", "attached", "detached"] {
            let input = json!({"selector": "div", "state": state});
            let parsed = parse_input(&input).unwrap();
            assert_eq!(parsed.state.as_deref(), Some(*state));
        }
    }

    #[test]
    fn rejects_invalid_state() {
        let input = json!({"selector": "div", "state": "bogus"});
        let err = parse_input(&input).unwrap_err();
        assert!(err
            .to_string()
            .contains("visible, hidden, attached, detached"));
    }

    #[test]
    fn rejects_state_without_selector() {
        let input = json!({"seconds": 5, "state": "visible"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("state requires a selector"));
    }

    #[test]
    fn state_none_when_omitted() {
        let input = json!({"selector": "#btn"});
        let parsed = parse_input(&input).unwrap();
        assert!(parsed.state.is_none());
    }

    #[test]
    fn silent_defaults_to_false() {
        let input = json!({"seconds": 1});
        let parsed = parse_input(&input).unwrap();
        assert!(!parsed.silent);
    }

    #[test]
    fn parses_silent_true() {
        let input = json!({"seconds": 1, "silent": true});
        let parsed = parse_input(&input).unwrap();
        assert!(parsed.silent);
    }

    #[test]
    fn parses_silent_false_explicit() {
        let input = json!({"seconds": 1, "silent": false});
        let parsed = parse_input(&input).unwrap();
        assert!(!parsed.silent);
    }

    #[test]
    fn parses_text_pattern_with_selector() {
        let input =
            json!({"selector": "#status", "text_pattern": "completed", "timeout_ms": 10000});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.text_pattern.as_deref(), Some("completed"));
        assert_eq!(parsed.selector.as_deref(), Some("#status"));
        assert_eq!(parsed.timeout_ms, 10000);
    }

    #[test]
    fn parses_text_pattern_without_selector() {
        let input = json!({"text_pattern": "success", "seconds": 5});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.text_pattern.as_deref(), Some("success"));
        assert!(parsed.selector.is_none());
    }

    #[test]
    fn rejects_text_pattern_with_state() {
        let input = json!({"selector": "#btn", "text_pattern": "done", "state": "visible"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn text_pattern_none_when_omitted() {
        let input = json!({"selector": "#btn"});
        let parsed = parse_input(&input).unwrap();
        assert!(parsed.text_pattern.is_none());
    }

    fn effect_json(effect: ToolEffect) -> Value {
        let ToolEffect::Reply(body) = effect else {
            panic!("expected ToolEffect::Reply, got {effect:?}");
        };
        serde_json::from_str(&body).unwrap()
    }

    #[tokio::test]
    async fn execute_silent_time_only_wait_omits_page_state() {
        use crate::tools::test_support::browser_with_observations;

        let mut browser = browser_with_observations(vec![]);
        let mut state = CrawlState::default();

        let effect = execute(
            &json!({"timeout_ms": 1, "silent": true}),
            &mut browser,
            &mut state,
        )
        .await
        .unwrap();
        let response = effect_json(effect);

        assert_eq!(response["waited_ms"], 1);
        assert!(response.get("page_state").is_none());
    }

    #[tokio::test]
    async fn execute_non_silent_time_only_wait_includes_page_state() {
        use crate::tools::test_support::browser_with_observations;

        let mut browser = browser_with_observations(vec![]);
        let mut state = CrawlState::default();

        let effect = execute(&json!({"timeout_ms": 1}), &mut browser, &mut state)
            .await
            .unwrap();
        let response = effect_json(effect);

        assert!(response.get("page_state").is_some());
    }

    #[tokio::test]
    async fn execute_silent_with_selector_still_includes_page_state() {
        use crate::tools::test_support::browser_with_observations;

        let mut browser = browser_with_observations(vec![]);
        let mut state = CrawlState::default();

        let effect = execute(
            &json!({"selector": "#btn", "silent": true}),
            &mut browser,
            &mut state,
        )
        .await
        .unwrap();
        let response = effect_json(effect);

        assert!(response.get("page_state").is_some());
    }
}
