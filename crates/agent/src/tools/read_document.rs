use std::path::Path;

use acrawl_processing::document;
use serde_json::{json, Value};

use crate::{ToolEffect, ToolExecutionError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::new("read_document requires 'path' field"))?;

    let output = document::extract_text(Path::new(path_str))
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(ToolEffect::reply_json(&json!({
        "format": output.format,
        "content": output.content,
        "word_count": output.word_count,
        "truncated": output.truncated,
        "metadata": output.metadata
    })))
}
