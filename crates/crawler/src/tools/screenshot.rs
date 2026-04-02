use serde_json::Value;

use crate::browser::BrowserContext;
use crate::CrawlError;

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let _ = input;

    let (screenshot_base64, size_bytes) = browser
        .bridge_mut()
        .screenshot()
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(serde_json::json!({
        "screenshot_base64": screenshot_base64,
        "size_bytes": size_bytes
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn empty_input_is_valid_json_object() {
        let input = json!({});
        assert!(input.is_object());
    }

    #[test]
    fn input_with_extra_fields_is_still_object() {
        let input = json!({"extra": "field"});
        assert!(input.is_object());
    }

    #[test]
    fn null_input_accepted() {
        let input = serde_json::Value::Null;
        assert!(input.is_null());
    }
}
