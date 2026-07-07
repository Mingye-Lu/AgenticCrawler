use std::path::Path;

use acrawl_processing::error::ProcessingError;
use acrawl_processing::transcribe::{self, TranscribeOptions};
use serde_json::{json, Value};

use crate::{BrowserContext, ToolEffect, ToolExecutionError};

const MODEL_FILENAMES: &[(&str, &str)] = &[
    ("tiny", "ggml-tiny.en.bin"),
    ("small", "ggml-small.en.bin"),
    ("large-turbo", "ggml-large-v3-turbo-q5_0.bin"),
];

/// Most-capable-first order, used to pick among multiple downloaded models
/// when none is explicitly requested.
const CAPABILITY_ORDER: &[&str] = &["large-turbo", "small", "tiny"];

const DEFAULT_MODEL_FILENAME: &str = "ggml-tiny.en.bin";

#[allow(clippy::unused_async)]
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
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let requested_model = input.get("model").and_then(|v| v.as_str());

    let models_dir = acrawl_core::config_home_dir().join("models");
    let filename =
        resolve_model_filename(&models_dir, requested_model).map_err(ToolExecutionError::new)?;
    let model_path = models_dir.join(filename);

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
        Err(ProcessingError::FeatureDisabled(feature)) => Ok(ToolEffect::reply_json(&json!({
            "error": "transcription feature not enabled",
            "details": format!("Build acrawl with --features {feature} to enable transcription"),
            "note": "This feature requires cmake and C++ build tools"
        }))),
        Err(ProcessingError::ModelNotFound(path)) => {
            let suggested_model = requested_model.unwrap_or("tiny");
            Ok(ToolEffect::reply_json(&json!({
                "error": "Whisper model not found",
                "model_path": path.to_string_lossy(),
                "fix": format!("Run `acrawl model download {suggested_model}` to download the model")
            })))
        }
        Err(e) => Err(ToolExecutionError::new(e.to_string())),
    }
}

/// Resolves which model filename to use inside `models_dir`.
///
/// If `requested` names a known model, its filename is used (or an error if unknown).
/// Otherwise the directory is scanned for `.bin` files: a single match is used as-is,
/// multiple matches are resolved by [`CAPABILITY_ORDER`], and no matches fall back to
/// the default tiny model filename (letting the caller's `ModelNotFound` path report it).
fn resolve_model_filename(models_dir: &Path, requested: Option<&str>) -> Result<String, String> {
    if let Some(name) = requested {
        return MODEL_FILENAMES
            .iter()
            .find(|(model_name, _)| *model_name == name)
            .map(|(_, filename)| (*filename).to_string())
            .ok_or_else(|| {
                let valid_names: Vec<&str> = MODEL_FILENAMES.iter().map(|(n, _)| *n).collect();
                format!(
                    "Unknown model '{name}'. Valid options: {}",
                    valid_names.join(", ")
                )
            });
    }

    let found_bins = list_bin_files(models_dir);

    match found_bins.len() {
        0 => Ok(DEFAULT_MODEL_FILENAME.to_string()),
        1 => Ok(found_bins.into_iter().next().expect("len checked above")),
        _ => {
            for model_name in CAPABILITY_ORDER {
                if let Some((_, filename)) = MODEL_FILENAMES.iter().find(|(n, _)| n == model_name) {
                    if found_bins.iter().any(|f| f == filename) {
                        return Ok((*filename).to_string());
                    }
                }
            }
            Ok(DEFAULT_MODEL_FILENAME.to_string())
        }
    }
}

fn list_bin_files(models_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(models_dir) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            std::path::Path::new(&file_name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
                .then_some(file_name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{resolve_model_filename, DEFAULT_MODEL_FILENAME};
    use std::fs;

    #[test]
    fn known_model_names_map_to_expected_filenames() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_model_filename(dir.path(), Some("tiny")).unwrap(),
            "ggml-tiny.en.bin"
        );
        assert_eq!(
            resolve_model_filename(dir.path(), Some("small")).unwrap(),
            "ggml-small.en.bin"
        );
        assert_eq!(
            resolve_model_filename(dir.path(), Some("large-turbo")).unwrap(),
            "ggml-large-v3-turbo-q5_0.bin"
        );
    }

    #[test]
    fn unknown_model_name_errors_with_valid_options() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_model_filename(dir.path(), Some("medium")).unwrap_err();
        assert!(err.contains("medium"));
        assert!(err.contains("tiny"));
        assert!(err.contains("small"));
        assert!(err.contains("large-turbo"));
    }

    #[test]
    fn no_model_specified_and_empty_dir_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_model_filename(dir.path(), None).unwrap(),
            DEFAULT_MODEL_FILENAME
        );
    }

    #[test]
    fn no_model_specified_and_missing_dir_falls_back_to_default() {
        let missing = std::path::Path::new("/nonexistent/models/dir/for/test");
        assert_eq!(
            resolve_model_filename(missing, None).unwrap(),
            DEFAULT_MODEL_FILENAME
        );
    }

    #[test]
    fn no_model_specified_and_single_bin_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ggml-small.en.bin"), b"stub").unwrap();
        assert_eq!(
            resolve_model_filename(dir.path(), None).unwrap(),
            "ggml-small.en.bin"
        );
    }

    #[test]
    fn no_model_specified_and_multiple_bins_picks_most_capable() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ggml-tiny.en.bin"), b"stub").unwrap();
        fs::write(dir.path().join("ggml-small.en.bin"), b"stub").unwrap();
        assert_eq!(
            resolve_model_filename(dir.path(), None).unwrap(),
            "ggml-small.en.bin"
        );

        fs::write(dir.path().join("ggml-large-v3-turbo-q5_0.bin"), b"stub").unwrap();
        assert_eq!(
            resolve_model_filename(dir.path(), None).unwrap(),
            "ggml-large-v3-turbo-q5_0.bin"
        );
    }
}
