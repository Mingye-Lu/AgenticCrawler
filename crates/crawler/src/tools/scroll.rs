use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<(String, i64), CrawlError> {
    let direction = input
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("down")
        .to_string();

    if direction != "up" && direction != "down" {
        return Err(CrawlError::new(format!(
            "invalid scroll direction: {direction}, expected 'up' or 'down'"
        )));
    }

    let pixels = input
        .get("amount")
        .or_else(|| input.get("pixels"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(500);

    Ok((direction, pixels))
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let (direction, pixels) = parse_input(input)?;

    browser
        .bridge_mut()
        .scroll(&direction, pixels)
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(json!({
        "success": true,
        "message": format!("Scrolled {direction} {pixels}px")
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_direction_and_pixels() {
        let input = json!({"direction": "down", "amount": 300});
        let (dir, px) = parse_input(&input).unwrap();
        assert_eq!(dir, "down");
        assert_eq!(px, 300);
    }

    #[test]
    fn defaults_to_down_500() {
        let input = json!({});
        let (dir, px) = parse_input(&input).unwrap();
        assert_eq!(dir, "down");
        assert_eq!(px, 500);
    }

    #[test]
    fn accepts_pixels_key() {
        let input = json!({"direction": "up", "pixels": 200});
        let (dir, px) = parse_input(&input).unwrap();
        assert_eq!(dir, "up");
        assert_eq!(px, 200);
    }

    #[test]
    fn rejects_invalid_direction() {
        let input = json!({"direction": "left"});
        assert!(parse_input(&input).is_err());
    }
}
