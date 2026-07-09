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
        if is_tar_archive(path) {
            return Err(ToolExecutionError::new(
                "TAR extraction is not yet supported. Use list_archive without 'extract' to list entries, then extract the entry manually.",
            ));
        }

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

/// Mirrors the extension-based format detection in
/// `acrawl_processing::archive::list_archive` to determine whether `path`
/// refers to a TAR-family archive (`.tar`, `.tar.gz`, `.tgz`, `.tar.bz2`).
fn is_tar_archive(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "tar" | "tgz" => true,
        "gz" | "bz2" => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            Path::new(stem)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tar"))
        }
        _ => false,
    }
}
