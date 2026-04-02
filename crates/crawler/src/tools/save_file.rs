use serde_json::{json, Value};

use crate::CrawlError;

pub struct SaveFileInput {
    pub url: String,
    pub filename: String,
}

pub fn parse_input(input: &Value) -> Result<SaveFileInput, CrawlError> {
    let url = input
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("save_file requires 'url' field"))?
        .to_string();

    let filename = input
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| url.rsplit('/').next().unwrap_or("download"))
        .to_string();

    Ok(SaveFileInput { url, filename })
}

pub fn execute(input: &Value) -> Result<Value, CrawlError> {
    let parsed = parse_input(input)?;
    Ok(json!({
        "tool": "save_file",
        "url": parsed.url,
        "filename": parsed.filename,
        "path": format!("workspace/{}", parsed.filename),
        "note": "bridge call required at runtime"
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_url_and_filename() {
        let input = json!({"url": "https://example.com/img.png", "filename": "photo.png"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.url, "https://example.com/img.png");
        assert_eq!(parsed.filename, "photo.png");
    }

    #[test]
    fn derives_filename_from_url() {
        let input = json!({"url": "https://example.com/files/report.pdf"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.filename, "report.pdf");
    }

    #[test]
    fn fails_without_url() {
        let input = json!({"filename": "file.txt"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn execute_returns_path() {
        let input = json!({"url": "https://example.com/data.csv", "filename": "data.csv"});
        let result = execute(&input).unwrap();
        assert_eq!(result["path"], "workspace/data.csv");
    }
}
