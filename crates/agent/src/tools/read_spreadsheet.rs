use std::path::Path;

use acrawl_processing::spreadsheet::{self, CellRange, SpreadsheetOptions};
use serde_json::{json, Value};

use crate::{BrowserContext, ToolEffect, ToolExecutionError};

#[allow(clippy::unused_async)]
pub async fn execute(
    input: &Value,
    _browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::new("read_spreadsheet requires 'path' field"))?;

    let sheet = input
        .get("sheet")
        .and_then(|v| v.as_str())
        .map(String::from);
    let range = input
        .get("range")
        .and_then(|v| v.as_str())
        .map(parse_cell_range);
    let max_rows = input
        .get("max_rows")
        .and_then(Value::as_u64)
        .map(|n| {
            #[allow(clippy::cast_possible_truncation)]
            let v = n as usize;
            v
        });

    let opts = SpreadsheetOptions {
        sheet,
        range,
        max_rows,
    };

    let output = spreadsheet::read_spreadsheet(Path::new(path_str), opts)
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(ToolEffect::reply_json(&json!({
        "format": output.format,
        "sheet": output.sheet,
        "total_rows": output.total_rows,
        "total_cols": output.total_cols,
        "headers": output.headers,
        "rows": output.rows,
        "truncated": output.truncated
    })))
}

fn parse_cell_range(s: &str) -> CellRange {
    match s {
        "headers" => CellRange::Headers,
        s if s.starts_with("first_") => s["first_".len()..]
            .parse::<usize>()
            .ok()
            .map_or(CellRange::All, CellRange::FirstN),
        _ => CellRange::All,
    }
}
