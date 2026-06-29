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
    let max_rows = input.get("max_rows").and_then(Value::as_u64).map(|n| {
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
        s if s.contains(':') => parse_excel_range(s).unwrap_or(CellRange::All),
        _ => CellRange::All,
    }
}

/// Parse Excel-style range like "A1:D10" into `CellRange::Range`.
/// Column letters: A=0, B=1, ..., Z=25, AA=26, etc.
/// Row numbers are 1-indexed in Excel notation, 0-indexed in `CellRange`.
fn parse_excel_range(s: &str) -> Option<CellRange> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let (start_col, start_row) = parse_cell_ref(parts[0])?;
    let (end_col, end_row) = parse_cell_ref(parts[1])?;
    Some(CellRange::Range {
        start_row,
        start_col,
        end_row,
        end_col,
    })
}

/// Parse a cell reference like "A1" into (`col_index`, `row_index`) (both 0-indexed).
fn parse_cell_ref(s: &str) -> Option<(usize, usize)> {
    let s = s.trim().to_uppercase();
    let col_end = s.find(|c: char| c.is_ascii_digit())?;
    let col_str = &s[..col_end];
    let row_str = &s[col_end..];

    if col_str.is_empty() || row_str.is_empty() {
        return None;
    }

    // Convert column letters to 0-indexed: A=0, B=1, ..., Z=25, AA=26, etc.
    let col_index = col_str
        .chars()
        .fold(0usize, |acc, c| acc * 26 + (c as usize - 'A' as usize + 1))
        .saturating_sub(1);

    // Row is 1-indexed in Excel, convert to 0-indexed
    let row_index = row_str.parse::<usize>().ok()?.saturating_sub(1);

    Some((col_index, row_index))
}
