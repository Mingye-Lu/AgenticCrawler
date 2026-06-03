use std::path::{Component, Path};

use base64::Engine as _;
use serde_json::Value;

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

fn validate_filename(filename: &str) -> Result<(), ToolExecutionError> {
    if filename.trim().is_empty() {
        return Err(ToolExecutionError::new(
            "screenshot filename must not be empty",
        ));
    }
    let path = Path::new(filename);
    if path.components().count() != 1 {
        return Err(ToolExecutionError::new(
            "screenshot filename must not contain path separators",
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(ToolExecutionError::new(
                    "screenshot filename must be a plain name without '.' or '..' components",
                ));
            }
        }
    }
    Ok(())
}

fn default_filename(format: &str) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let ext = match format {
        "jpeg" => "jpg",
        "webp" => "webp",
        _ => "png",
    };
    format!("screenshot_{ms}.{ext}")
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let save = input.get("save").and_then(Value::as_bool).unwrap_or(false);
    let selector = input.get("selector").and_then(Value::as_str);
    let format = input.get("format").and_then(Value::as_str);
    let quality = input
        .get("quality")
        .and_then(Value::as_u64)
        .map(|q| q.min(100) as u32);
    let full_page = input
        .get("full_page")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let opts = crate::ScreenshotOptions {
        selector,
        format,
        quality,
        full_page,
    };

    let (screenshot_base64, size_bytes) = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .screenshot(&opts)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    if !save {
        let mut result = serde_json::json!({
            "screenshot_base64": screenshot_base64,
            "size_bytes": size_bytes
        });
        if let Some(fmt) = format {
            result["format"] = serde_json::json!(fmt);
        }
        return Ok(ToolEffect::reply_json(&result));
    }

    let filename = match input.get("filename").and_then(|v| v.as_str()) {
        Some(name) => {
            validate_filename(name)?;
            name.to_string()
        }
        None => default_filename(format.unwrap_or("png")),
    };

    let settings = runtime::load_settings();
    let override_dir = input.get("output_dir").and_then(|v| v.as_str());
    let output_dir = runtime::resolve_output_dir(&settings, override_dir);
    let target = output_dir.join(&filename);

    tokio::fs::create_dir_all(&output_dir)
        .await
        .map_err(|e| ToolExecutionError::new(format!("failed to create output directory: {e}")))?;

    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(&screenshot_base64)
        .map_err(|e| ToolExecutionError::new(format!("failed to decode screenshot: {e}")))?;

    tokio::fs::write(&target, &png_bytes)
        .await
        .map_err(|e| ToolExecutionError::new(format!("failed to write screenshot: {e}")))?;

    let saved_path = target.to_string_lossy().to_string();
    Ok(ToolEffect::reply_json(&serde_json::json!({
        "saved_path": saved_path,
        "size_bytes": size_bytes
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use async_trait::async_trait;

    use crate::{
        BridgeError, BrowserBackend, BrowserState, PageInfo, ScreenshotOptions, SharedBridge,
    };

    static ENV_LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();

    fn setup_temp_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "acrawl_screenshot_test_{}_{suffix}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn valid_png_base64() -> String {
        use base64::Engine as _;
        let png_bytes: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
            0x77, 0x53, 0xDE, // end IHDR
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
            0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21,
            0xBC, 0x33, // end IDAT
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND chunk
            0xAE, 0x42, 0x60, 0x82,
        ];
        base64::engine::general_purpose::STANDARD.encode(png_bytes)
    }

    #[derive(Debug)]
    struct MockBackend {
        screenshot_result: Result<(String, usize), BridgeError>,
    }

    impl MockBackend {
        fn with_valid_screenshot() -> Self {
            let b64 = valid_png_base64();
            let size = b64.len();
            Self {
                screenshot_result: Ok((b64, size)),
            }
        }

        fn with_invalid_base64() -> Self {
            Self {
                screenshot_result: Ok(("!!!not-valid-base64!!!".to_string(), 22)),
            }
        }
    }

    #[async_trait]
    impl BrowserBackend for MockBackend {
        async fn navigate(&mut self, _url: &str) -> Result<PageInfo, BridgeError> {
            unimplemented!()
        }
        async fn new_page(&mut self, _url: Option<&str>) -> Result<usize, BridgeError> {
            Ok(0)
        }
        async fn close_page(&mut self, _: usize) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn scroll(&mut self, _: &str, _: i64) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn page_map(&mut self, _scope: Option<&str>) -> Result<serde_json::Value, BridgeError> {
            Ok(serde_json::json!({}))
        }
        async fn read_content(
            &mut self,
            _: Option<&str>,
            _: Option<&str>,
            _: usize,
            _: usize,
        ) -> Result<serde_json::Value, BridgeError> {
            Ok(serde_json::json!({}))
        }
        async fn wait_for_selector(&mut self, _: &str, _: u64, _: Option<&str>) -> Result<bool, BridgeError> {
            Ok(true)
        }
        async fn select_option(&mut self, _: &str, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn evaluate(&mut self, _: &str) -> Result<serde_json::Value, BridgeError> {
            Ok(serde_json::json!(null))
        }
        async fn hover(&mut self, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn press_key(&mut self, _: &str, _: Option<&str>) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn switch_tab(&mut self, _: i64) -> Result<serde_json::Value, BridgeError> {
            Ok(serde_json::json!({}))
        }
        async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
            unimplemented!()
        }
        async fn import_cookies(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn import_cookies_only(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn import_local_storage(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn list_resources(&mut self) -> Result<serde_json::Value, BridgeError> {
            Ok(serde_json::json!([]))
        }
        async fn save_file(&mut self, _: &str, _: &str) -> Result<String, BridgeError> {
            Ok(String::new())
        }
        async fn click(&mut self, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn click_at(&mut self, _: f64, _: f64) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn fill(&mut self, _: &str, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn screenshot(
            &mut self,
            _options: &ScreenshotOptions<'_>,
        ) -> Result<(String, usize), BridgeError> {
            match &self.screenshot_result {
                Ok((b64, size)) => Ok((b64.clone(), *size)),
                Err(_) => Err(BridgeError::Protocol("mock screenshot error".to_string())),
            }
        }
        async fn go_back(&mut self) -> Result<String, BridgeError> {
            Ok(String::new())
        }
    }

    fn make_browser(backend: MockBackend) -> BrowserContext {
        let bridge: SharedBridge = std::sync::Arc::new(tokio::sync::Mutex::new(
            Box::new(backend) as Box<dyn BrowserBackend + Send>
        ));
        BrowserContext::new(bridge)
    }

    #[test]
    fn validate_filename_accepts_plain_name() {
        assert!(validate_filename("shot.png").is_ok());
        assert!(validate_filename("my_screenshot.png").is_ok());
    }

    #[test]
    fn validate_filename_rejects_empty() {
        assert!(validate_filename("").is_err());
        assert!(validate_filename("   ").is_err());
    }

    #[test]
    fn validate_filename_rejects_path_separators() {
        assert!(validate_filename("sub/shot.png").is_err());
    }

    #[test]
    fn validate_filename_rejects_traversal() {
        assert!(validate_filename("../shot.png").is_err());
    }

    #[test]
    fn default_filename_has_png_extension() {
        let name = default_filename("png");
        assert!(name.starts_with("screenshot_"));
        assert!(Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png")));
    }

    #[tokio::test]
    async fn execute_save_false_returns_base64() {
        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({});

        let effect = execute(&input, &mut browser).await.unwrap();
        let json_str = format!("{effect:?}");
        assert!(json_str.contains("screenshot_base64"));
        assert!(!json_str.contains("saved_path"));
    }

    #[tokio::test]
    async fn execute_save_false_explicit() {
        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({"save": false});

        let effect = execute(&input, &mut browser).await.unwrap();
        let json_str = format!("{effect:?}");
        assert!(json_str.contains("screenshot_base64"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn execute_save_true_writes_file_default_filename() {
        let _lock = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let temp_dir = setup_temp_dir("default_fn");
        let output_dir = temp_dir.join("output");

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        std::fs::write(
            temp_dir.join("settings.json"),
            format!(
                r#"{{"output_dir": "{}"}}"#,
                output_dir.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({"save": true});

        let effect = execute(&input, &mut browser).await.unwrap();
        let json_str = format!("{effect:?}");
        assert!(json_str.contains("saved_path"));
        assert!(!json_str.contains("screenshot_base64"));

        let entries: Vec<_> = std::fs::read_dir(&output_dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(entries.len(), 1);
        let filename = entries[0].file_name().to_string_lossy().to_string();
        assert!(filename.starts_with("screenshot_"));
        assert!(Path::new(&filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png")));

        let written = std::fs::read(entries[0].path()).unwrap();
        assert_eq!(&written[..4], &[0x89, 0x50, 0x4E, 0x47]); // PNG magic

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn execute_save_true_custom_filename() {
        let _lock = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let temp_dir = setup_temp_dir("custom_fn");
        let output_dir = temp_dir.join("screenshots");

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        std::fs::write(
            temp_dir.join("settings.json"),
            format!(
                r#"{{"output_dir": "{}"}}"#,
                output_dir.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({"save": true, "filename": "capture.png"});

        let effect = execute(&input, &mut browser).await.unwrap();
        let json_str = format!("{effect:?}");
        assert!(json_str.contains("saved_path"));
        assert!(json_str.contains("capture.png"));

        let target = output_dir.join("capture.png");
        assert!(target.exists());
        let written = std::fs::read(&target).unwrap();
        assert_eq!(&written[..4], &[0x89, 0x50, 0x4E, 0x47]);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn execute_save_true_invalid_filename_rejected() {
        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({"save": true, "filename": "../evil.png"});

        let result = execute(&input, &mut browser).await;
        assert!(result.is_err());

        let input = serde_json::json!({"save": true, "filename": "sub/dir.png"});
        let result = execute(&input, &mut browser).await;
        assert!(result.is_err());

        let input = serde_json::json!({"save": true, "filename": ""});
        let result = execute(&input, &mut browser).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn execute_save_true_invalid_base64_errors() {
        let _lock = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let temp_dir = setup_temp_dir("bad_b64");
        let output_dir = temp_dir.join("output");

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        std::fs::write(
            temp_dir.join("settings.json"),
            format!(
                r#"{{"output_dir": "{}"}}"#,
                output_dir.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let mut browser = make_browser(MockBackend::with_invalid_base64());
        let input = serde_json::json!({"save": true});

        let result = execute(&input, &mut browser).await;
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("decode"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn execute_save_true_write_error_on_invalid_dir() {
        let _lock = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let temp_dir = setup_temp_dir("write_err");
        // Point output_dir at a FILE, so create_dir_all will fail
        let blocker = temp_dir.join("blocked");
        std::fs::write(&blocker, "not a directory").unwrap();
        let output_dir = blocker.join("subdir");

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        std::fs::write(
            temp_dir.join("settings.json"),
            format!(
                r#"{{"output_dir": "{}"}}"#,
                output_dir.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({"save": true});

        let result = execute(&input, &mut browser).await;
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("output directory") || err_msg.contains("write"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn default_filename_uses_format_extension() {
        let jpeg_name = default_filename("jpeg");
        assert!(Path::new(&jpeg_name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg")));

        let webp_name = default_filename("webp");
        assert!(Path::new(&webp_name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("webp")));

        let png_name = default_filename("png");
        assert!(Path::new(&png_name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png")));
    }

    #[tokio::test]
    async fn execute_returns_format_in_response() {
        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({"format": "jpeg"});

        let effect = execute(&input, &mut browser).await.unwrap();
        let json_str = format!("{effect:?}");
        assert!(json_str.contains("jpeg"));
    }

    #[tokio::test]
    async fn execute_options_are_parsed_correctly() {
        let mut browser = make_browser(MockBackend::with_valid_screenshot());
        let input = serde_json::json!({
            "selector": "#main",
            "format": "webp",
            "quality": 85,
            "full_page": true
        });

        let effect = execute(&input, &mut browser).await.unwrap();
        let json_str = format!("{effect:?}");
        assert!(json_str.contains("screenshot_base64"));
        assert!(json_str.contains("webp"));
    }
}
