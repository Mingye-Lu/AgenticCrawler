use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use serde_json::Value;

use crate::browser::BrowserContext;
use crate::{ToolEffect, ToolError};

fn validate_filename(filename: &str) -> Result<(), ToolError> {
    if filename.trim().is_empty() {
        return Err(ToolError::new("screenshot filename must not be empty"));
    }
    let path = Path::new(filename);
    if path.components().count() != 1 {
        return Err(ToolError::new(
            "screenshot filename must not contain path separators",
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(ToolError::new(
                    "screenshot filename must be a plain name without '.' or '..' components",
                ));
            }
        }
    }
    Ok(())
}

fn default_filename() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("screenshot_{ms}.png")
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let save = input
        .get("save")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let (screenshot_base64, size_bytes) = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolError::new(e.to_string()))?
        .screenshot()
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;

    if !save {
        return Ok(ToolEffect::reply_json(&serde_json::json!({
            "screenshot_base64": screenshot_base64,
            "size_bytes": size_bytes
        })));
    }

    let filename = match input.get("filename").and_then(|v| v.as_str()) {
        Some(name) => {
            validate_filename(name)?;
            name.to_string()
        }
        None => default_filename(),
    };

    let settings = runtime::load_settings();
    let output_dir = runtime::settings_get_output_dir(&settings).to_string();
    let target = PathBuf::from(&output_dir).join(&filename);

    tokio::fs::create_dir_all(&output_dir)
        .await
        .map_err(|e| ToolError::new(format!("failed to create output directory: {e}")))?;

    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(&screenshot_base64)
        .map_err(|e| ToolError::new(format!("failed to decode screenshot: {e}")))?;

    tokio::fs::write(&target, &png_bytes)
        .await
        .map_err(|e| ToolError::new(format!("failed to write screenshot: {e}")))?;

    let saved_path = target.to_string_lossy().to_string();
    Ok(ToolEffect::reply_json(&serde_json::json!({
        "saved_path": saved_path,
        "size_bytes": size_bytes
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let name = default_filename();
        assert!(name.starts_with("screenshot_"));
        assert!(name.ends_with(".png"));
    }
}
