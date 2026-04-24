use std::time::Duration;

use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 300_000;
const MAX_TIMEOUT_SECONDS: f64 = 300.0;

pub struct WaitInput {
    pub selector: Option<String>,
    pub timeout_ms: u64,
}

pub fn parse_input(input: &Value) -> Result<WaitInput, CrawlError> {
    let selector = input
        .get("selector")
        .and_then(|v| v.as_str())
        .map(String::from);

    let timeout_ms = parse_timeout_ms(input)?;

    if selector.is_none() && timeout_ms == 0 {
        return Err(CrawlError::new(
            "wait requires either a selector or a positive timeout",
        ));
    }

    Ok(WaitInput {
        selector,
        timeout_ms,
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

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let parsed = parse_input(input)?;

    if let Some(ref selector) = parsed.selector {
        let found = browser
            .acquire_bridge()
            .await
            .map_err(|e| CrawlError::new(e.to_string()))?
            .wait_for_selector(selector, parsed.timeout_ms)
            .await
            .map_err(|e| CrawlError::new(e.to_string()))?;

        Ok(json!({
            "success": true,
            "found": found,
            "selector": selector,
            "timeout_ms": parsed.timeout_ms
        }))
    } else {
        tokio::time::sleep(Duration::from_millis(parsed.timeout_ms)).await;

        Ok(json!({
            "success": true,
            "waited_ms": parsed.timeout_ms
        }))
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
}
