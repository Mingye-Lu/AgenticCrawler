use serde_json::Value;

use crate::browser::BrowserContext;
use crate::CrawlError;

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let _ = input;

    let url = browser
        .bridge_mut()
        .go_back()
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(serde_json::json!({
        "url": url
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
    fn null_input_is_accepted() {
        let input = serde_json::Value::Null;
        assert!(input.is_null());
    }
}
