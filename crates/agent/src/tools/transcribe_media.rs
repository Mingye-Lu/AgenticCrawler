use std::path::Path;

use acrawl_processing::error::ProcessingError;
use acrawl_processing::transcribe::{self, TranscribeOptions};
use serde_json::{json, Value};

use crate::{BrowserContext, ToolEffect, ToolExecutionError};

pub async fn execute(
    input: &Value,
    _browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let path_str = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::new("transcribe_media requires 'path' field"))?;

    let language = input
        .get("language")
        .and_then(|v| v.as_str())
        .map(String::from);
    let timestamps = input
        .get("timestamps")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let model_path = acrawl_core::config_home_dir()
        .join("models")
        .join("ggml-tiny.en.bin");

    let opts = TranscribeOptions {
        language,
        timestamps,
        model_path: model_path.clone(),
    };

    match transcribe::transcribe(Path::new(path_str), opts) {
        Ok(output) => Ok(ToolEffect::reply_json(&json!({
            "transcript": output.transcript,
            "duration_seconds": output.duration_seconds,
            "segments": output.segments,
            "metadata": output.metadata
        }))),
        Err(ProcessingError::FeatureDisabled(feature)) => {
            Ok(ToolEffect::reply_json(&json!({
                "error": "transcription feature not enabled",
                "details": format!("Build acrawl with --features {feature} to enable transcription"),
                "note": "This feature requires cmake and C++ build tools"
            })))
        }
        Err(ProcessingError::ModelNotFound(path)) => {
            Ok(ToolEffect::reply_json(&json!({
                "error": "Whisper model not found",
                "model_path": path.to_string_lossy(),
                "fix": "Run `acrawl model download tiny` to download the model (~75MB)"
            })))
        }
        Err(e) => Err(ToolExecutionError::new(e.to_string())),
    }
}
