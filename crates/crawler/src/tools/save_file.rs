use std::path::PathBuf;

use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

pub struct SaveFileInput {
    pub url: String,
    pub filename: String,
    pub subdir: Option<String>,
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

    let subdir = input
        .get("subdir")
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(SaveFileInput {
        url,
        filename,
        subdir,
    })
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let parsed = parse_input(input)?;

    let settings = runtime::load_settings();
    let workspace = runtime::settings_get_workspace_dir(&settings).to_string();
    let mut target = PathBuf::from(&workspace);
    if let Some(ref sub) = parsed.subdir {
        target.push(sub);
    }
    target.push(&parsed.filename);

    let path_str = target.to_string_lossy().to_string();

    let saved_path = browser
        .acquire_bridge()
        .await
        .save_file(&parsed.url, &path_str)
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(json!({
        "success": true,
        "path": saved_path,
        "url": parsed.url
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
    fn parses_subdir() {
        let input = json!({"url": "https://example.com/data.csv", "subdir": "exports"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.subdir.as_deref(), Some("exports"));
    }
}
