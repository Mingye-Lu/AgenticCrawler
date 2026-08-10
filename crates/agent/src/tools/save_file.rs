use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde_json::{json, Value};

use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub struct SaveFileInput {
    pub url: String,
    pub filename: String,
    pub subdir: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
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

    let headers = match input.get("headers").and_then(|v| v.as_object()) {
        Some(obj) if !obj.is_empty() => {
            let mut map = BTreeMap::new();
            for (key, value) in obj {
                if let Some(value) = value.as_str() {
                    map.insert(key.clone(), value.to_string());
                }
            }
            if map.is_empty() {
                None
            } else {
                Some(map)
            }
        }
        _ => None,
    };

    Ok(SaveFileInput {
        url,
        filename,
        subdir,
        headers,
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

    if filename.contains(':') {
        return Err(CrawlError::new(
            "save_file filename must not contain ':' (Windows ADS not allowed)",
        ));
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

/// Walk up from `path` to the nearest ancestor that currently exists on
/// disk (inclusive of `path` itself).
///
/// `Path::canonicalize` fails outright for a path that doesn't exist yet,
/// which is the common case for the *first* write into a not-yet-created
/// subdirectory. Skipping the symlink-escape check whenever that happens
/// would fail open instead of closed. Since every component between the
/// nearest existing ancestor and the target has already been validated
/// (via `validate_relative_path`/`validate_filename`) to contain no `..`
/// or absolute/prefix components, checking that the nearest *existing*
/// ancestor resolves inside the allowed root is sufficient to catch a
/// symlink planted anywhere along the path.
fn nearest_existing_ancestor(path: &Path) -> Option<&Path> {
    let mut current = path;
    loop {
        if current.exists() {
            return Some(current);
        }
        current = current.parent()?;
    }
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let parsed = parse_input(input)?;

    let settings = runtime::load_settings();
    let override_dir = input.get("output_dir").and_then(|v| v.as_str());
    let output_base = runtime::resolve_output_dir(&settings, override_dir);
    let mut target = output_base.clone();
    if let Some(ref sub) = parsed.subdir {
        target.push(sub);
    }
    target.push(&parsed.filename);

    if let Some(parent) = target.parent() {
        if let Some(existing_ancestor) = nearest_existing_ancestor(parent) {
            if let Ok(canonical_ancestor) = existing_ancestor.canonicalize() {
                if let Ok(canonical_base) = output_base.canonicalize() {
                    if !canonical_ancestor.starts_with(&canonical_base) {
                        return Err(ToolExecutionError::new(
                            "resolved path escapes output directory (possible symlink attack)",
                        ));
                    }
                }
            }
        }
    }

    let path_str = target.to_string_lossy().to_string();

    let saved_path = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .save_file(&parsed.url, &path_str, parsed.headers.as_ref())
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(ToolEffect::reply_json(&json!({
        "success": true,
        "path": saved_path,
        "url": parsed.url
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::collections::BTreeMap;

    use crate::tools::test_support::{
        browser_with_save_file_header_recorder, take_recorded_save_file_headers,
    };

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
    fn parse_input_reads_headers() {
        let input = json!({
            "url": "https://example.com/file.mp4",
            "headers": { "Referer": "https://www.bilibili.com", "X-Test": "42" }
        });

        let parsed = parse_input(&input).unwrap();
        let headers = parsed.headers.unwrap();

        assert_eq!(
            headers.get("Referer").map(std::string::String::as_str),
            Some("https://www.bilibili.com")
        );
        assert_eq!(
            headers.get("X-Test").map(std::string::String::as_str),
            Some("42")
        );
    }

    #[test]
    fn parse_input_empty_headers_yields_none() {
        let input = json!({ "url": "https://example.com/file.mp4", "headers": {} });
        let parsed = parse_input(&input).unwrap();
        assert!(parsed.headers.is_none());
    }

    #[test]
    fn rejects_path_traversal() {
        let input = json!({"url": "https://example.com/file.txt", "filename": "../file.txt"});
        assert!(parse_input(&input).is_err());

        let input = json!({"url": "https://example.com/file.txt", "filename": "file.txt", "subdir": "../outside"});
        assert!(parse_input(&input).is_err());
    }

    #[tokio::test]
    async fn save_file_forwards_headers_to_backend() {
        let (mut browser, recorder) = browser_with_save_file_header_recorder(vec![]);

        execute(
            &json!({
                "url": "https://example.com/file.mp4",
                "headers": {
                    "Referer": "https://www.bilibili.com",
                    "X-Test": "42"
                }
            }),
            &mut browser,
        )
        .await
        .expect("save_file should succeed");

        let headers = take_recorded_save_file_headers(&recorder)
            .await
            .expect("backend should record forwarded headers");

        let expected = BTreeMap::from([
            (
                "Referer".to_string(),
                "https://www.bilibili.com".to_string(),
            ),
            ("X-Test".to_string(), "42".to_string()),
        ]);
        assert_eq!(headers, expected);
    }

    #[test]
    fn nearest_existing_ancestor_walks_up_past_not_yet_created_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let existing = tmp.path().join("existing_dir");
        std::fs::create_dir(&existing).expect("create existing_dir");

        // "not_yet/created" don't exist on disk yet — the common
        // first-write-into-a-new-subdirectory case.
        let target_parent = existing.join("not_yet").join("created");

        let found =
            nearest_existing_ancestor(&target_parent).expect("an existing ancestor must be found");
        assert_eq!(found, existing);
    }

    #[test]
    fn nearest_existing_ancestor_returns_path_itself_when_it_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let found = nearest_existing_ancestor(tmp.path()).expect("tempdir exists");
        assert_eq!(found, tmp.path());
    }

    // Unprivileged symlink creation requires no special OS setup on Unix
    // (unlike Windows, where it needs admin rights or Developer Mode), so
    // this exercises the full escape scenario only on Unix runners (CI runs
    // ubuntu-latest for `cargo test`).
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_subdir_escape_is_blocked_even_when_leaf_dir_is_new() {
        let allowed_root = tempfile::tempdir().expect("allowed_root tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");

        // allowed_root/evil -> outside (a symlink planted inside the
        // allowed output directory that resolves outside of it).
        let evil_link = allowed_root.path().join("evil");
        std::os::unix::fs::symlink(outside.path(), &evil_link).expect("create symlink");

        // "newsub" does not exist yet under the symlink target, so
        // target.parent() ("evil/newsub") fails to canonicalize directly —
        // the nearest *existing* ancestor is "evil" itself, which resolves
        // through the symlink to `outside`.
        let (mut browser, _recorder) = browser_with_save_file_header_recorder(vec![]);
        let result = execute(
            &json!({
                "url": "https://example.com/file.txt",
                "filename": "payload.txt",
                "subdir": "evil/newsub",
                "output_dir": allowed_root.path().to_str().unwrap(),
            }),
            &mut browser,
        )
        .await;

        let err = result.expect_err("escape via symlinked subdir must be rejected");
        assert!(
            err.to_string().contains("escapes output directory"),
            "unexpected error: {err}"
        );
    }
}
