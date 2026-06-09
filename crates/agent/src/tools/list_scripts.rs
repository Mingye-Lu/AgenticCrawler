use serde_json::{json, Value};
use std::fs;
use std::time::SystemTime;

use crate::{ToolEffect, ToolExecutionError};
use acrawl_core::config_home_dir;

pub fn execute(_input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let scripts_dir = config_home_dir().join("scripts");

    let mut scripts = Vec::new();

    if scripts_dir.exists() {
        let entries = fs::read_dir(&scripts_dir).map_err(|e| {
            ToolExecutionError::new(format!("failed to read scripts directory: {e}"))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                ToolExecutionError::new(format!("failed to read directory entry: {e}"))
            })?;
            let path = entry.path();

            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) {
                    let metadata = fs::metadata(&path).map_err(|e| {
                        ToolExecutionError::new(format!("failed to get file metadata: {e}"))
                    })?;

                    let size_bytes = metadata.len();
                    let modified_at = format_system_time(metadata.modified().ok());

                    scripts.push(json!({
                        "name": name,
                        "size_bytes": size_bytes,
                        "modified_at": modified_at
                    }));
                }
            }
        }
    }

    scripts.sort_by(|a, b| {
        let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        a_name.cmp(b_name)
    });

    let json_array = serde_json::to_string(&scripts)
        .map_err(|e| ToolExecutionError::new(format!("failed to serialize scripts list: {e}")))?;

    Ok(ToolEffect::Reply(json_array))
}

fn format_system_time(time: Option<SystemTime>) -> String {
    match time {
        Some(t) => t
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or_else(|_| "unknown".to_string(), |d| d.as_secs().to_string()),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_system_time_handles_none() {
        let result = format_system_time(None);
        assert_eq!(result, "unknown");
    }

    #[test]
    fn format_system_time_returns_unix_epoch_seconds() {
        use std::time::{Duration, UNIX_EPOCH};

        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let result = format_system_time(Some(t));
        assert_eq!(result, "1700000000");
    }
}
