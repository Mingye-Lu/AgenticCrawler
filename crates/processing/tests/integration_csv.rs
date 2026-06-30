use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn read_csv_sample_file() {
    use acrawl_processing::spreadsheet::{read_spreadsheet, SpreadsheetOptions};

    let csv_path = fixtures_dir().join("sample.csv");
    let opts = SpreadsheetOptions {
        sheet: None,
        range: None,
        max_rows: None,
    };
    let result = read_spreadsheet(&csv_path, opts).unwrap();

    assert_eq!(result.format, "csv");
    assert!(!result.headers.is_empty());
    assert_eq!(result.headers[0], "name");
    assert_eq!(result.headers[1], "age");
    assert_eq!(result.headers[2], "city");
    assert_eq!(result.headers[3], "score");
    assert_eq!(result.total_rows, 10);
    assert_eq!(result.rows.len(), 10);
    assert!(!result.truncated);
}

#[test]
fn read_csv_first_row_values() {
    use acrawl_processing::spreadsheet::{read_spreadsheet, SpreadsheetOptions};

    let csv_path = fixtures_dir().join("sample.csv");
    let opts = SpreadsheetOptions::default();
    let result = read_spreadsheet(&csv_path, opts).unwrap();

    assert_eq!(result.rows[0][0], "Alice");
    assert_eq!(result.rows[0][1], "28");
    assert_eq!(result.rows[0][2], "New York");
    assert_eq!(result.rows[0][3], "95");
}

#[test]
fn read_csv_with_row_limit() {
    use acrawl_processing::spreadsheet::{read_spreadsheet, SpreadsheetOptions};

    let csv_path = fixtures_dir().join("sample.csv");
    let opts = SpreadsheetOptions {
        sheet: None,
        range: None,
        max_rows: Some(3),
    };
    let result = read_spreadsheet(&csv_path, opts).unwrap();

    assert_eq!(result.rows.len(), 3);
    assert_eq!(result.total_rows, 10);
    assert!(result.truncated);
}

#[test]
fn missing_csv_returns_error() {
    use acrawl_processing::error::ProcessingError;
    use acrawl_processing::spreadsheet::{read_spreadsheet, SpreadsheetOptions};

    let opts = SpreadsheetOptions::default();
    let result = read_spreadsheet(std::path::Path::new("/nonexistent/file.csv"), opts);
    assert!(matches!(result, Err(ProcessingError::IoError(_))));
}
