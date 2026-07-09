use std::path::Path;

use acrawl_processing::image_proc;
use serde_json::{json, Value};

use crate::{ToolEffect, ToolExecutionError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::new("view_image requires 'path' field"))?;

    let max_dimension = input.get("max_dimension").and_then(Value::as_u64).map(|n| {
        #[allow(clippy::cast_possible_truncation)]
        let v = n as u32;
        v
    });

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
