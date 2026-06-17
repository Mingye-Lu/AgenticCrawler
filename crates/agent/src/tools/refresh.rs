use serde_json::Value;

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let _ = input;

    browser.ref_map_mut().clear();

    let _page_info = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .reload()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let page_state = super::feedback::post_action_page_state(browser).await;

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "seq": 0,
        "success": true,
        "page_state": page_state
    })))
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
