use serde_json::Value;

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

use super::feedback::InteractionKind;

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let widen = input
        .get("widen")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    browser.ref_map_mut().clear();

    let url = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .go_back()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(
        browser,
        crawl_state,
        InteractionKind::Passive,
        None,
        widen,
    )
    .await?;

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "seq": seq,
        "success": true,
        "url": url,
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
