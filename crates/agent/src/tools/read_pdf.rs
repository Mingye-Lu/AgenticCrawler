use std::path::Path;

use acrawl_processing::pdf::{self, PageRange};
use serde_json::{json, Value};

use crate::{ToolEffect, ToolExecutionError};

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::new("read_pdf requires 'path' field"))?;
    let path = Path::new(path_str);

    let mode = input.get("mode").and_then(|v| v.as_str()).unwrap_or("text");
    let pages_str = input.get("pages").and_then(|v| v.as_str());

    if mode == "metadata" {
        let meta = pdf::metadata(path).map_err(|e| ToolExecutionError::new(e.to_string()))?;
        return Ok(ToolEffect::reply_json(&json!({ "metadata": meta })));
    }

    let page_range = parse_page_range(pages_str);

    let output =
        pdf::extract_text(path, page_range).map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(ToolEffect::reply_json(&json!({
        "pages_extracted": output.pages_extracted,
        "total_pages": output.total_pages,
        "content": output.content,
        "truncated": output.truncated,
        "metadata": output.metadata
    })))
}

fn parse_page_range(pages: Option<&str>) -> Option<PageRange> {
    let s = pages?;
    if s.contains('-') {
        let parts: Vec<&str> = s.splitn(2, '-').collect();
        let start = parts[0].parse::<usize>().ok()?;
        if parts[1].is_empty() {
            Some(PageRange::From(start))
        } else {
            let end = parts[1].parse::<usize>().ok()?;
            Some(PageRange::Range(start, end))
        }
    } else if let Ok(n) = s.parse::<usize>() {
        Some(PageRange::Single(n))
    } else {
        None
    }
}
