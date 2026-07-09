use std::path::Path;

use acrawl_processing::archive;
use serde_json::{json, Value};

use crate::{ToolEffect, ToolExecutionError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::new("list_archive requires 'path' field"))?;
    let path = Path::new(path_str);

    let extract_entry = input.get("extract").and_then(|v| v.as_str());

    if let Some(entry_path) = extract_entry {
        let output_dir = std::env::temp_dir().join("acrawl_extract");
        std::fs::create_dir_all(&output_dir).map_err(|e| ToolExecutionError::new(e.to_string()))?;

        let extracted = archive::extract_entry(path, entry_path, &output_dir)
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;

        return Ok(ToolEffect::reply_json(&json!({
            "extracted_path": extracted.path.to_string_lossy(),
            "size": extracted.size,
            "content": extracted.content
        })));
    }

    let output = archive::list_archive(path).map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(ToolEffect::reply_json(&json!({
        "format": output.format,
        "total_files": output.total_files,
        "total_size_bytes": output.total_size_bytes,
        "entries": output.entries
    })))
}
