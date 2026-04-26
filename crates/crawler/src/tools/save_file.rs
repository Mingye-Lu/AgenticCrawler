use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolError};

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
        .map_or_else(|| filename_from_url(&url), ToOwned::to_owned);
    validate_filename(&filename)?;

    let subdir = input
        .get("subdir")
        .and_then(|v| v.as_str())
        .map(String::from);
    if let Some(ref subdir) = subdir {
        validate_relative_path("subdir", subdir)?;
    }

    Ok(SaveFileInput {
        url,
        filename,
        subdir,
    })
}

fn filename_from_url(url: &str) -> String {
    url.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("download")
        .to_string()
}

fn validate_filename(filename: &str) -> Result<(), CrawlError> {
    if filename.trim().is_empty() {
        return Err(CrawlError::new("save_file filename must not be empty"));
    }

    let path = Path::new(filename);
    if path.components().count() != 1 {
        return Err(CrawlError::new(
            "save_file filename must not contain path separators",
        ));
    }
    validate_relative_path("filename", filename)
}

fn validate_relative_path(field: &str, value: &str) -> Result<(), CrawlError> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() {
        return Ok(());
    }

    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(CrawlError::new(format!(
                    "save_file {field} must be a relative path without '.' or '..' components"
                )));
            }
        }
    }

    Ok(())
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
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
        .map_err(|e| ToolError(e.to_string()))?
        .save_file(&parsed.url, &path_str)
        .await
        .map_err(|e| ToolError(e.to_string()))?;

    Ok(ToolEffect::reply_json(&json!({
        "success": true,
        "path": saved_path,
        "url": parsed.url
    })))
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
    fn trailing_slash_url_uses_last_non_empty_segment() {
        let input = json!({"url": "https://example.com/files/"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.filename, "files");
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

    #[test]
    fn rejects_path_traversal() {
        let input = json!({"url": "https://example.com/file.txt", "filename": "../file.txt"});
        assert!(parse_input(&input).is_err());

        let input = json!({"url": "https://example.com/file.txt", "filename": "file.txt", "subdir": "../outside"});
        assert!(parse_input(&input).is_err());
    }
}
