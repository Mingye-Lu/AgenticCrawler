use serde_json::{json, Value};
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Csv,
    Text,
}

/// Error type for output operations.
#[derive(Debug)]
pub enum OutputError {
    /// I/O error during write.
    IoError(String),
    /// CSV serialization error.
    CsvError(String),
    /// Invalid data format for the requested output type.
    InvalidDataFormat(String),
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputError::IoError(msg) => write!(f, "I/O error: {}", msg),
            OutputError::CsvError(msg) => write!(f, "CSV error: {}", msg),
            OutputError::InvalidDataFormat(msg) => write!(f, "Invalid data format: {}", msg),
        }
    }
}

impl std::error::Error for OutputError {}

/// Write extracted data in the requested format to the provided writer.
///
/// # Arguments
///
/// * `data` - The data to serialize (typically a JSON value)
/// * `format` - The desired output format (JSON, CSV, or Text)
/// * `writer` - The output writer (e.g., stdout, file)
///
/// # Errors
///
/// Returns an `OutputError` if serialization or writing fails.
pub fn write_output(
    data: &Value,
    format: &OutputFormat,
    writer: &mut dyn Write,
) -> Result<(), OutputError> {
    match format {
        OutputFormat::Json => write_json(data, writer),
        OutputFormat::Csv => write_csv(data, writer),
        OutputFormat::Text => write_text(data, writer),
    }
}

/// Write data as pretty-printed JSON.
fn write_json(data: &Value, writer: &mut dyn Write) -> Result<(), OutputError> {
    let json_str = serde_json::to_string_pretty(data)
        .map_err(|e| OutputError::IoError(format!("JSON serialization failed: {}", e)))?;
    writer
        .write_all(json_str.as_bytes())
        .map_err(|e| OutputError::IoError(format!("Failed to write JSON: {}", e)))?;
    writer
        .write_all(b"\n")
        .map_err(|e| OutputError::IoError(format!("Failed to write newline: {}", e)))?;
    Ok(())
}

/// Write data as CSV format.
///
/// Expects data to be an array of objects. Each object becomes a row,
/// with keys as column headers.
fn write_csv(data: &Value, writer: &mut dyn Write) -> Result<(), OutputError> {
    // Extract rows from data
    let rows = extract_rows(data)?;

    if rows.is_empty() {
        return Ok(());
    }

    // Collect all unique keys in order
    let mut fieldnames: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for row in &rows {
        if let Value::Object(map) = row {
            for key in map.keys() {
                if seen.insert(key.clone()) {
                    fieldnames.push(key.clone());
                }
            }
        }
    }

    // Create CSV writer
    let mut csv_writer = csv::Writer::from_writer(writer);

    // Write header
    csv_writer
        .write_record(&fieldnames)
        .map_err(|e| OutputError::CsvError(format!("Failed to write CSV header: {}", e)))?;

    // Write rows
    for row in rows {
        if let Value::Object(map) = row {
            let record: Vec<String> = fieldnames
                .iter()
                .map(|key| map.get(key).map(|v| value_to_string(v)).unwrap_or_default())
                .collect();
            csv_writer
                .write_record(&record)
                .map_err(|e| OutputError::CsvError(format!("Failed to write CSV row: {}", e)))?;
        }
    }

    csv_writer
        .flush()
        .map_err(|e| OutputError::CsvError(format!("Failed to flush CSV writer: {}", e)))?;

    Ok(())
}

/// Write data as human-readable text format.
///
/// Formats data as key-value pairs or tables depending on structure.
fn write_text(data: &Value, writer: &mut dyn Write) -> Result<(), OutputError> {
    let text = format_text(data);
    writer
        .write_all(text.as_bytes())
        .map_err(|e| OutputError::IoError(format!("Failed to write text: {}", e)))?;
    writer
        .write_all(b"\n")
        .map_err(|e| OutputError::IoError(format!("Failed to write newline: {}", e)))?;
    Ok(())
}

/// Extract rows from data, handling both arrays and single objects.
fn extract_rows(data: &Value) -> Result<Vec<Value>, OutputError> {
    match data {
        Value::Array(arr) => {
            // If array contains objects, use them directly
            if arr.iter().all(|v| v.is_object()) {
                Ok(arr.clone())
            } else if arr.is_empty() {
                Ok(Vec::new())
            } else {
                // If array contains non-objects, wrap each in an object
                Ok(arr.iter().map(|v| json!({ "value": v })).collect())
            }
        }
        Value::Object(_) => {
            // Single object becomes a single row
            Ok(vec![data.clone()])
        }
        _ => Err(OutputError::InvalidDataFormat(
            "Data must be an object or array of objects".to_string(),
        )),
    }
}

/// Convert a JSON value to a string representation.
fn value_to_string(val: &Value) -> String {
    match val {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            // For arrays, join elements with semicolon
            arr.iter()
                .map(|v| value_to_string(v))
                .collect::<Vec<_>>()
                .join("; ")
        }
        Value::Object(map) => {
            // For objects, format as key: value pairs
            map.iter()
                .map(|(k, v)| format!("{}: {}", k, value_to_string(v)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

/// Format data as human-readable text.
fn format_text(data: &Value) -> String {
    match data {
        Value::Array(arr) => {
            if arr.is_empty() {
                "No data extracted.".to_string()
            } else if arr.iter().all(|v| v.is_object()) {
                // Array of objects: render as table-like format
                render_items_as_text(arr)
            } else {
                // Array of primitives: render as bullet list
                arr.iter()
                    .map(|v| format!("- {}", value_to_string(v)))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        Value::Object(_) => {
            // Single object: render as key-value pairs
            dict_to_keyvalue(data)
        }
        _ => value_to_string(data),
    }
}

/// Render an array of items as text.
fn render_items_as_text(items: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            parts.push(String::new()); // Blank line between items
        }

        if let Value::Object(_) = item {
            parts.push(dict_to_keyvalue(item));
        } else {
            parts.push(format!("- {}", value_to_string(item)));
        }
    }

    parts.join("\n")
}

/// Render a single object as key: value lines.
fn dict_to_keyvalue(obj: &Value) -> String {
    if let Value::Object(map) = obj {
        map.iter()
            .map(|(k, v)| format!("{}: {}", k, value_to_string(v)))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        value_to_string(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_json_simple_object() {
        let data = json!({
            "name": "Alice",
            "age": 30
        });

        let mut output = Vec::new();
        let result = write_json(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("\"name\""));
        assert!(output_str.contains("\"Alice\""));
        assert!(output_str.contains("\"age\""));
        assert!(output_str.contains("30"));
    }

    #[test]
    fn test_write_json_array() {
        let data = json!([
            { "id": 1, "name": "Alice" },
            { "id": 2, "name": "Bob" }
        ]);

        let mut output = Vec::new();
        let result = write_json(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("\"id\""));
        assert!(output_str.contains("\"Alice\""));
        assert!(output_str.contains("\"Bob\""));
    }

    #[test]
    fn test_write_csv_array_of_objects() {
        let data = json!([
            { "name": "Alice", "age": 30 },
            { "name": "Bob", "age": 25 }
        ]);

        let mut output = Vec::new();
        let result = write_csv(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("name,age") | output_str.contains("age,name"));
        assert!(output_str.contains("Alice"));
        assert!(output_str.contains("Bob"));
        assert!(output_str.contains("30"));
        assert!(output_str.contains("25"));
    }

    #[test]
    fn test_write_csv_single_object() {
        let data = json!({ "name": "Alice", "age": 30 });

        let mut output = Vec::new();
        let result = write_csv(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("name") || output_str.contains("age"));
        assert!(output_str.contains("Alice"));
    }

    #[test]
    fn test_write_csv_empty_array() {
        let data = json!([]);

        let mut output = Vec::new();
        let result = write_csv(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.is_empty());
    }

    #[test]
    fn test_write_csv_with_missing_fields() {
        let data = json!([
            { "name": "Alice", "age": 30 },
            { "name": "Bob", "city": "NYC" }
        ]);

        let mut output = Vec::new();
        let result = write_csv(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        // Should have headers for all fields
        assert!(output_str.contains("name"));
        assert!(output_str.contains("age"));
        assert!(output_str.contains("city"));
    }

    #[test]
    fn test_write_text_array_of_objects() {
        let data = json!([
            { "name": "Alice", "age": 30 },
            { "name": "Bob", "age": 25 }
        ]);

        let mut output = Vec::new();
        let result = write_text(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("name"));
        assert!(output_str.contains("Alice"));
        assert!(output_str.contains("Bob"));
    }

    #[test]
    fn test_write_text_single_object() {
        let data = json!({ "name": "Alice", "age": 30 });

        let mut output = Vec::new();
        let result = write_text(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("name"));
        assert!(output_str.contains("Alice"));
        assert!(output_str.contains("age"));
        assert!(output_str.contains("30"));
    }

    #[test]
    fn test_write_text_array_of_primitives() {
        let data = json!(["apple", "banana", "cherry"]);

        let mut output = Vec::new();
        let result = write_text(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("apple"));
        assert!(output_str.contains("banana"));
        assert!(output_str.contains("cherry"));
    }

    #[test]
    fn test_write_text_empty_array() {
        let data = json!([]);

        let mut output = Vec::new();
        let result = write_text(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("No data extracted"));
    }

    #[test]
    fn test_write_output_json_format() {
        let data = json!({ "test": "value" });
        let mut output = Vec::new();

        let result = write_output(&data, &OutputFormat::Json, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("\"test\""));
        assert!(output_str.contains("\"value\""));
    }

    #[test]
    fn test_write_output_csv_format() {
        let data = json!([{ "col1": "val1", "col2": "val2" }]);
        let mut output = Vec::new();

        let result = write_output(&data, &OutputFormat::Csv, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("col1"));
        assert!(output_str.contains("val1"));
    }

    #[test]
    fn test_write_output_text_format() {
        let data = json!({ "key": "value" });
        let mut output = Vec::new();

        let result = write_output(&data, &OutputFormat::Text, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("key"));
        assert!(output_str.contains("value"));
    }

    #[test]
    fn test_value_to_string_nested_object() {
        let val = json!({ "nested": { "key": "value" } });
        let result = value_to_string(&val);
        assert!(result.contains("nested"));
        assert!(result.contains("key"));
        assert!(result.contains("value"));
    }

    #[test]
    fn test_value_to_string_array() {
        let val = json!(["a", "b", "c"]);
        let result = value_to_string(&val);
        assert!(result.contains("a"));
        assert!(result.contains("b"));
        assert!(result.contains("c"));
    }

    #[test]
    fn test_csv_with_special_characters() {
        let data = json!([
            { "name": "Alice, Bob", "note": "Has \"quotes\"" }
        ]);

        let mut output = Vec::new();
        let result = write_csv(&data, &mut output);

        assert!(result.is_ok());
        let output_str = String::from_utf8(output).unwrap();
        // CSV writer should properly escape these
        assert!(output_str.contains("Alice"));
    }

    #[test]
    fn test_output_error_display() {
        let err = OutputError::IoError("test error".to_string());
        assert_eq!(err.to_string(), "I/O error: test error");

        let err = OutputError::CsvError("csv error".to_string());
        assert_eq!(err.to_string(), "CSV error: csv error");

        let err = OutputError::InvalidDataFormat("invalid".to_string());
        assert_eq!(err.to_string(), "Invalid data format: invalid");
    }
}
