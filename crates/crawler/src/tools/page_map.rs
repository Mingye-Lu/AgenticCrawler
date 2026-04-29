use serde_json::Value;

use crate::browser::BrowserContext;
use crate::{ToolEffect, ToolError};

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let _ = input;

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolError(e.to_string()))?
        .page_map()
        .await
        .map_err(|e| ToolError(e.to_string()))?;

    Ok(ToolEffect::reply_json(&result))
}

// Unit tests are intentionally omitted: `page_map` takes no input parameters
// and its `execute` function requires a live Playwright bridge, so meaningful
// coverage comes from the integration tests in `crates/crawler/tests/`.
