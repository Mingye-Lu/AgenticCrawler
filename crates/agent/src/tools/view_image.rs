use std::path::Path;

use acrawl_processing::image_proc;
use serde_json::{json, Value};

use crate::{BrowserContext, ToolEffect, ToolExecutionError};

pub async fn execute(
    input: &Value,
    _browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::new("view_image requires 'path' field"))?;

    let max_dimension = input
        .get("max_dimension")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    let output = image_proc::view_image(Path::new(path_str), max_dimension)
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(ToolEffect::reply_json(&json!({
        "screenshot_base64": output.image_base64,
        "media_type": output.media_type,
        "size_bytes": output.size_bytes,
        "format": output.format,
        "original_dimensions": output.original_dimensions,
        "resized_dimensions": output.resized_dimensions
    })))
}
