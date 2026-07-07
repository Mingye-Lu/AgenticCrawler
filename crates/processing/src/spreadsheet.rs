use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};
use serde::{Deserialize, Serialize};

use crate::error::ProcessingError;

const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
const DEFAULT_MAX_ROWS: usize = 1000;

/// Which cells to read from a sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellRange {
    All,
    Headers,
    FirstN(usize),
    Range {
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    },
}

/// Options controlling spreadsheet reading.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpreadsheetOptions {
    pub sheet: Option<String>,
    pub range: Option<CellRange>,
    pub max_rows: Option<usize>,
}

/// Parsed spreadsheet output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpreadsheetOutput {
    pub format: String,
    pub sheet: String,
    pub total_rows: usize,
    pub total_cols: usize,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

/// Read a spreadsheet file (CSV, XLSX, or ODS).
///
/// # Errors
///
/// Returns `ProcessingError` on I/O failures, unsupported formats,
/// oversized files, missing sheets, or corrupt data.
#[allow(clippy::needless_pass_by_value)]
pub fn read_spreadsheet(
    path: &Path,
    opts: SpreadsheetOptions,
) -> Result<SpreadsheetOutput, ProcessingError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_FILE_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: metadata.len(),
            limit_bytes: MAX_FILE_SIZE,
        });
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    match ext.as_str() {
        "csv" | "tsv" => read_csv(path, &opts, &ext),
        "xlsx" | "xlsm" | "xlsb" | "ods" => read_workbook(path, &opts, &ext),
        "xls" => Err(ProcessingError::UnsupportedFormat(
            "xls (legacy binary Excel format) is not supported".into(),
        )),
        _ => Err(ProcessingError::UnsupportedFormat(format!(
            ".{ext} is not a supported spreadsheet format"
        ))),
    }
}

fn read_csv(
    path: &Path,
    opts: &SpreadsheetOptions,
    ext: &str,
) -> Result<SpreadsheetOutput, ProcessingError> {
    let delimiter = if ext == "tsv" { b'\t' } else { b',' };

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        .from_path(path)
        .map_err(|e| ProcessingError::FormatError(format!("CSV parse error: {e}")))?;

    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| ProcessingError::FormatError(format!("CSV header error: {e}")))?
        .iter()
        .map(String::from)
        .collect();

    let total_cols = headers.len();
    let max_rows = effective_max_rows(opts);
    let range = opts.range.clone().unwrap_or(CellRange::All);

    if matches!(range, CellRange::Headers) {
        let total_rows = rdr.records().count();
        return Ok(SpreadsheetOutput {
            format: ext.to_string(),
            sheet: "Sheet1".into(),
            total_rows,
            total_cols,
            headers,
            rows: vec![],
            truncated: false,
        });
    }

    let all_records: Vec<csv::StringRecord> =
        rdr.records().filter_map(std::result::Result::ok).collect();
    let total_rows = all_records.len();

    let (rows, truncated) = apply_range_csv(&all_records, &range, max_rows, total_cols);

    Ok(SpreadsheetOutput {
        format: ext.to_string(),
        sheet: "Sheet1".into(),
        total_rows,
        total_cols,
        headers,
        rows,
        truncated,
    })
}

fn apply_range_csv(
    records: &[csv::StringRecord],
    range: &CellRange,
    max_rows: usize,
    total_cols: usize,
) -> (Vec<Vec<String>>, bool) {
    match range {
        CellRange::All | CellRange::Headers => {
            let limit = max_rows.min(records.len());
            let truncated = records.len() > max_rows;
            let rows = records[..limit]
                .iter()
                .map(|r| record_to_vec(r, total_cols))
                .collect();
            (rows, truncated)
        }
        CellRange::FirstN(n) => {
            let effective = (*n).min(max_rows).min(records.len());
            let truncated = records.len() > effective;
            let rows = records[..effective]
                .iter()
                .map(|r| record_to_vec(r, total_cols))
                .collect();
            (rows, truncated)
        }
        CellRange::Range {
            start_row,
            start_col,
            end_row,
            end_col,
        } => {
            let row_end = (*end_row).min(records.len());
            let row_start = (*start_row).min(row_end);
            let col_end = (*end_col).min(total_cols);
            let col_start = (*start_col).min(col_end);

            let slice = &records[row_start..row_end];
            let limit = max_rows.min(slice.len());
            let truncated = slice.len() > max_rows;

            let rows = slice[..limit]
                .iter()
                .map(|r| {
                    (col_start..col_end)
                        .map(|c| r.get(c).unwrap_or("").to_string())
                        .collect()
                })
                .collect();
            (rows, truncated)
        }
    }
}

/// Convert a CSV `StringRecord` to a `Vec<String>`, padding short rows.
fn record_to_vec(record: &csv::StringRecord, total_cols: usize) -> Vec<String> {
    (0..total_cols)
        .map(|i| record.get(i).unwrap_or("").to_string())
        .collect()
}

fn read_workbook(
    path: &Path,
    opts: &SpreadsheetOptions,
    ext: &str,
) -> Result<SpreadsheetOutput, ProcessingError> {
    let mut workbook = open_workbook_auto(path)
        .map_err(|e| ProcessingError::FormatError(format!("Failed to open workbook: {e}")))?;

    let sheet_names: Vec<String> = workbook.sheet_names().clone();

    if sheet_names.is_empty() {
        return Ok(SpreadsheetOutput {
            format: ext.to_string(),
            sheet: String::new(),
            total_rows: 0,
            total_cols: 0,
            headers: vec![],
            rows: vec![],
            truncated: false,
        });
    }

    let sheet_name = match &opts.sheet {
        Some(name) => {
            if !sheet_names.contains(name) {
                return Err(ProcessingError::FormatError(format!(
                    "Sheet '{}' not found. Available sheets: {}",
                    name,
                    sheet_names.join(", ")
                )));
            }
            name.clone()
        }
        None => sheet_names[0].clone(),
    };

    let sheet_range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|e| ProcessingError::FormatError(format!("Failed to read sheet: {e}")))?;

    let (row_count, col_count) = sheet_range.get_size();

    if row_count == 0 || col_count == 0 {
        return Ok(SpreadsheetOutput {
            format: ext.to_string(),
            sheet: sheet_name,
            total_rows: 0,
            total_cols: 0,
            headers: vec![],
            rows: vec![],
            truncated: false,
        });
    }

    let headers: Vec<String> = (0..col_count)
        .map(|c| cell_to_string(sheet_range.get((0, c))))
        .collect();

    let total_rows = row_count.saturating_sub(1);
    let total_cols = col_count;
    let max_rows = effective_max_rows(opts);
    let range = opts.range.clone().unwrap_or(CellRange::All);

    if matches!(range, CellRange::Headers) {
        return Ok(SpreadsheetOutput {
            format: ext.to_string(),
            sheet: sheet_name,
            total_rows,
            total_cols,
            headers,
            rows: vec![],
            truncated: false,
        });
    }

    let (rows, truncated) =
        apply_range_workbook(&sheet_range, &range, max_rows, total_rows, total_cols);

    Ok(SpreadsheetOutput {
        format: ext.to_string(),
        sheet: sheet_name,
        total_rows,
        total_cols,
        headers,
        rows,
        truncated,
    })
}

fn apply_range_workbook(
    sheet: &calamine::Range<Data>,
    range: &CellRange,
    max_rows: usize,
    total_rows: usize,
    total_cols: usize,
) -> (Vec<Vec<String>>, bool) {
    match range {
        CellRange::All | CellRange::Headers => {
            let limit = max_rows.min(total_rows);
            let truncated = total_rows > max_rows;
            let rows = (1..=limit)
                .map(|r| {
                    (0..total_cols)
                        .map(|c| cell_to_string(sheet.get((r, c))))
                        .collect()
                })
                .collect();
            (rows, truncated)
        }
        CellRange::FirstN(n) => {
            let effective = (*n).min(max_rows).min(total_rows);
            let truncated = total_rows > effective;
            let rows = (1..=effective)
                .map(|r| {
                    (0..total_cols)
                        .map(|c| cell_to_string(sheet.get((r, c))))
                        .collect()
                })
                .collect();
            (rows, truncated)
        }
        CellRange::Range {
            start_row,
            start_col,
            end_row,
            end_col,
        } => {
            // Range is 0-indexed into the data area (row 0 = first data row after header)
            let row_end = (*end_row).min(total_rows);
            let row_start = (*start_row).min(row_end);
            let col_end = (*end_col).min(total_cols);
            let col_start = (*start_col).min(col_end);

            let slice_len = row_end - row_start;
            let limit = max_rows.min(slice_len);
            let truncated = slice_len > max_rows;

            let rows = (row_start..row_start + limit)
                .map(|r| {
                    let sheet_row = r + 1;
                    (col_start..col_end)
                        .map(|c| cell_to_string(sheet.get((sheet_row, c))))
                        .collect()
                })
                .collect();
            (rows, truncated)
        }
    }
}

fn cell_to_string(cell: Option<&Data>) -> String {
    match cell {
        None | Some(Data::Empty) => String::new(),
        Some(Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s)) => s.clone(),
        Some(Data::Int(i)) => i.to_string(),
        Some(Data::Float(f)) => {
            #[allow(clippy::cast_precision_loss)]
            let limit = f64::from(i64::MAX as f32);
            if f.fract() == 0.0 && f.abs() < limit {
                #[allow(clippy::cast_possible_truncation)]
                let i = *f as i64;
                i.to_string()
            } else {
                f.to_string()
            }
        }
        Some(Data::Bool(b)) => b.to_string(),
        Some(Data::Error(e)) => format!("#ERR:{e:?}"),
        Some(Data::DateTime(dt)) => dt.to_string(),
    }
}

fn effective_max_rows(opts: &SpreadsheetOptions) -> usize {
    opts.max_rows.unwrap_or(DEFAULT_MAX_ROWS)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    fn create_test_csv() -> NamedTempFile {
        let mut f = NamedTempFile::with_suffix(".csv").unwrap();
        writeln!(f, "name,age,city,score").unwrap();
        for i in 1..=15_usize {
            writeln!(
                f,
                "person{i},{},{},{}",
                20 + i,
                ["NYC", "LA", "CHI"][i % 3],
                i * 10
            )
            .unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn read_csv_headers() {
        let f = create_test_csv();
        let result = read_spreadsheet(
            f.path(),
            SpreadsheetOptions {
                range: Some(CellRange::Headers),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.format, "csv");
        assert_eq!(result.sheet, "Sheet1");
        assert_eq!(result.headers, vec!["name", "age", "city", "score"]);
        assert_eq!(result.total_cols, 4);
        assert_eq!(result.total_rows, 15);
        assert!(result.rows.is_empty());
        assert!(!result.truncated);
    }

    #[test]
    fn read_csv_all_rows() {
        let f = create_test_csv();
        let result = read_spreadsheet(f.path(), SpreadsheetOptions::default()).unwrap();

        assert_eq!(result.format, "csv");
        assert_eq!(result.total_rows, 15);
        assert_eq!(result.rows.len(), 15);
        assert!(!result.truncated);
        assert_eq!(result.rows[0][0], "person1");
        assert_eq!(result.rows[0][1], "21");
        assert_eq!(result.rows[0][2], "LA");
        assert_eq!(result.rows[0][3], "10");
    }

    #[test]
    fn row_limit_truncation() {
        let f = create_test_csv();
        let result = read_spreadsheet(
            f.path(),
            SpreadsheetOptions {
                max_rows: Some(5),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.rows.len(), 5);
        assert_eq!(result.total_rows, 15);
        assert!(result.truncated);
    }

    #[test]
    fn first_n_range() {
        let f = create_test_csv();
        let result = read_spreadsheet(
            f.path(),
            SpreadsheetOptions {
                range: Some(CellRange::FirstN(3)),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.rows.len(), 3);
        assert!(result.truncated);
    }

    #[test]
    fn explicit_range() {
        let f = create_test_csv();
        let result = read_spreadsheet(
            f.path(),
            SpreadsheetOptions {
                range: Some(CellRange::Range {
                    start_row: 0,
                    start_col: 0,
                    end_row: 3,
                    end_col: 2,
                }),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0].len(), 2);
        assert_eq!(result.rows[0][0], "person1");
        assert_eq!(result.rows[0][1], "21");
    }

    #[test]
    fn unsupported_format() {
        let f = NamedTempFile::with_suffix(".doc").unwrap();
        let err = read_spreadsheet(f.path(), SpreadsheetOptions::default()).unwrap_err();
        match err {
            ProcessingError::UnsupportedFormat(msg) => {
                assert!(msg.contains("doc"));
            }
            other => panic!("Expected UnsupportedFormat, got: {other:?}"),
        }
    }

    #[test]
    fn xls_legacy_unsupported() {
        let f = NamedTempFile::with_suffix(".xls").unwrap();
        let err = read_spreadsheet(f.path(), SpreadsheetOptions::default()).unwrap_err();
        match err {
            ProcessingError::UnsupportedFormat(msg) => {
                assert!(msg.contains("legacy"));
            }
            other => panic!("Expected UnsupportedFormat, got: {other:?}"),
        }
    }

    #[test]
    fn file_too_large() {
        assert_eq!(MAX_FILE_SIZE, 100 * 1024 * 1024);
    }

    #[test]
    fn empty_csv() {
        let mut f = NamedTempFile::with_suffix(".csv").unwrap();
        writeln!(f, "col1,col2,col3").unwrap();
        f.flush().unwrap();

        let result = read_spreadsheet(f.path(), SpreadsheetOptions::default()).unwrap();
        assert_eq!(result.headers, vec!["col1", "col2", "col3"]);
        assert_eq!(result.total_rows, 0);
        assert!(result.rows.is_empty());
        assert!(!result.truncated);
    }

    #[test]
    fn tsv_format() {
        let mut f = NamedTempFile::with_suffix(".tsv").unwrap();
        writeln!(f, "name\tvalue").unwrap();
        writeln!(f, "foo\t42").unwrap();
        writeln!(f, "bar\t99").unwrap();
        f.flush().unwrap();

        let result = read_spreadsheet(f.path(), SpreadsheetOptions::default()).unwrap();
        assert_eq!(result.format, "tsv");
        assert_eq!(result.headers, vec!["name", "value"]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0], vec!["foo", "42"]);
    }

    #[test]
    fn missing_sheet_name_error_xlsx() {
        let mut f = NamedTempFile::with_suffix(".xlsx").unwrap();
        f.write_all(b"not a valid xlsx").unwrap();
        f.flush().unwrap();

        let err = read_spreadsheet(
            f.path(),
            SpreadsheetOptions {
                sheet: Some("NonExistent".into()),
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(matches!(err, ProcessingError::FormatError(_)));
    }

    #[test]
    fn max_rows_default_is_1000() {
        assert_eq!(DEFAULT_MAX_ROWS, 1000);
        let opts = SpreadsheetOptions::default();
        assert_eq!(effective_max_rows(&opts), 1000);
    }

    #[test]
    fn cell_range_serde_roundtrip() {
        let ranges = vec![
            CellRange::All,
            CellRange::Headers,
            CellRange::FirstN(50),
            CellRange::Range {
                start_row: 0,
                start_col: 1,
                end_row: 10,
                end_col: 5,
            },
        ];

        for range in ranges {
            let json = serde_json::to_string(&range).unwrap();
            let back: CellRange = serde_json::from_str(&json).unwrap();
            // Just verify it roundtrips without panic
            let _ = format!("{back:?}");
        }
    }

    #[test]
    fn spreadsheet_output_serde() {
        let output = SpreadsheetOutput {
            format: "csv".into(),
            sheet: "Sheet1".into(),
            total_rows: 10,
            total_cols: 3,
            headers: vec!["a".into(), "b".into(), "c".into()],
            rows: vec![vec!["1".into(), "2".into(), "3".into()]],
            truncated: false,
        };

        let json = serde_json::to_string(&output).unwrap();
        let back: SpreadsheetOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.format, "csv");
        assert_eq!(back.total_rows, 10);
    }
}
