//! Persistence layer for script storage on disk.
//!
//! Provides functions to save, load, and list scripts in a directory.
//! Scripts are stored as pretty-printed JSON files with `.json` extension.

use crate::grammar::ScriptDefinition;
use std::fs;
use std::path::Path;
use std::time::SystemTime;
use thiserror::Error;

/// Metadata about a stored script.
#[derive(Debug, Clone)]
pub struct ScriptMetadata {
    /// Script name (without `.json` extension).
    pub name: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last modification time.
    pub modified_at: SystemTime,
}

/// Errors that can occur during script persistence operations.
#[derive(Debug, Error)]
pub enum PersistenceError {
    /// I/O error (file not found, permission denied, etc.).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON parsing or serialization error.
    #[error("JSON error: {0}")]
    Parse(String),

    /// Invalid script name (contains path separators, null bytes, etc.).
    #[error("Invalid script name: {0}")]
    InvalidName(String),
}

/// Validates a script name.
///
/// Valid names contain only alphanumeric characters, underscores, and hyphens.
/// Names must not be empty, contain path separators, or null bytes.
///
/// # Arguments
/// * `name` - The script name to validate
///
/// # Returns
/// * `Ok(())` if the name is valid
/// * `Err(PersistenceError::InvalidName)` if the name is invalid
pub fn validate_script_name(name: &str) -> Result<(), PersistenceError> {
    if name.is_empty() {
        return Err(PersistenceError::InvalidName(
            "script name cannot be empty".to_string(),
        ));
    }

    if name.contains('\0') {
        return Err(PersistenceError::InvalidName(
            "script name cannot contain null bytes".to_string(),
        ));
    }

    if name.contains('/') || name.contains('\\') {
        return Err(PersistenceError::InvalidName(
            "script name cannot contain path separators".to_string(),
        ));
    }

    // Allow alphanumeric, underscore, and hyphen
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(PersistenceError::InvalidName(
            "script name must contain only alphanumeric characters, underscores, and hyphens"
                .to_string(),
        ));
    }

    Ok(())
}

/// Saves a script to disk.
///
/// Creates the scripts directory if it doesn't exist.
/// The script is serialized to pretty-printed JSON and written to `{scripts_dir}/{name}.json`.
///
/// # Arguments
/// * `scripts_dir` - Directory where scripts are stored
/// * `name` - Script name (validated before saving)
/// * `script` - The script definition to save
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(PersistenceError)` if validation fails, directory creation fails, or writing fails
pub fn save_script_to_disk(
    scripts_dir: &Path,
    name: &str,
    script: &ScriptDefinition,
) -> Result<(), PersistenceError> {
    validate_script_name(name)?;

    // Create directory if it doesn't exist
    fs::create_dir_all(scripts_dir)?;

    // Serialize to pretty JSON
    let json_str =
        serde_json::to_string_pretty(script).map_err(|e| PersistenceError::Parse(e.to_string()))?;

    // Write to file
    let file_path = scripts_dir.join(format!("{name}.json"));
    fs::write(file_path, json_str)?;

    Ok(())
}

/// Loads a script from disk.
///
/// Reads the JSON file at `{scripts_dir}/{name}.json` and deserializes it.
///
/// # Arguments
/// * `scripts_dir` - Directory where scripts are stored
/// * `name` - Script name (validated before loading)
///
/// # Returns
/// * `Ok(ScriptDefinition)` on success
/// * `Err(PersistenceError)` if validation fails, file not found, or parsing fails
pub fn load_script_from_disk(
    scripts_dir: &Path,
    name: &str,
) -> Result<ScriptDefinition, PersistenceError> {
    validate_script_name(name)?;

    let file_path = scripts_dir.join(format!("{name}.json"));
    let json_str = fs::read_to_string(file_path)?;

    let script = serde_json::from_str::<ScriptDefinition>(&json_str)
        .map_err(|e| PersistenceError::Parse(e.to_string()))?;

    Ok(script)
}

/// Lists all scripts in a directory.
///
/// Scans the directory for `.json` files, extracts metadata for each,
/// and returns them sorted by name.
///
/// # Arguments
/// * `scripts_dir` - Directory where scripts are stored
///
/// # Returns
/// * `Ok(Vec<ScriptMetadata>)` containing all scripts, sorted by name
/// * `Err(PersistenceError)` if the directory cannot be read
pub fn list_scripts_on_disk(scripts_dir: &Path) -> Result<Vec<ScriptMetadata>, PersistenceError> {
    let mut scripts = Vec::new();

    // Return empty list if directory doesn't exist
    if !scripts_dir.exists() {
        return Ok(scripts);
    }

    for entry in fs::read_dir(scripts_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Only process `.json` files
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        // Extract script name (filename without `.json`)
        if let Some(file_name) = path.file_stem().and_then(|s| s.to_str()) {
            let metadata = entry.metadata()?;
            let size_bytes = metadata.len();
            let modified_at = metadata.modified()?;

            scripts.push(ScriptMetadata {
                name: file_name.to_string(),
                size_bytes,
                modified_at,
            });
        }
    }

    // Sort by name
    scripts.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(scripts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_validate_script_name_valid() {
        assert!(validate_script_name("my_script").is_ok());
        assert!(validate_script_name("my-script").is_ok());
        assert!(validate_script_name("MyScript123").is_ok());
        assert!(validate_script_name("_script").is_ok());
        assert!(validate_script_name("script_123-test").is_ok());
    }

    #[test]
    fn test_validate_script_name_invalid() {
        assert!(validate_script_name("").is_err());
        assert!(validate_script_name("my/script").is_err());
        assert!(validate_script_name("my\\script").is_err());
        assert!(validate_script_name("my script").is_err());
        assert!(validate_script_name("my.script").is_err());
        assert!(validate_script_name("my@script").is_err());
    }

    #[test]
    fn test_save_and_load_script() {
        let temp_dir = TempDir::new().unwrap();
        let scripts_dir = temp_dir.path();

        let script = ScriptDefinition {
            schema_version: 1,
            name: Some("test_script".to_string()),
            steps: vec![],
        };

        // Save
        assert!(save_script_to_disk(scripts_dir, "test_script", &script).is_ok());

        // Verify file exists
        let file_path = scripts_dir.join("test_script.json");
        assert!(file_path.exists());

        // Load
        let loaded = load_script_from_disk(scripts_dir, "test_script").unwrap();
        assert_eq!(loaded.schema_version, 1);
        assert_eq!(loaded.name, Some("test_script".to_string()));
    }

    #[test]
    fn test_list_scripts() {
        let temp_dir = TempDir::new().unwrap();
        let scripts_dir = temp_dir.path();

        let first_script_def = ScriptDefinition {
            schema_version: 1,
            name: Some("script_a".to_string()),
            steps: vec![],
        };

        let second_script_def = ScriptDefinition {
            schema_version: 1,
            name: Some("script_b".to_string()),
            steps: vec![],
        };

        save_script_to_disk(scripts_dir, "script_a", &first_script_def).unwrap();
        save_script_to_disk(scripts_dir, "script_b", &second_script_def).unwrap();

        let scripts = list_scripts_on_disk(scripts_dir).unwrap();
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].name, "script_a");
        assert_eq!(scripts[1].name, "script_b");
    }

    #[test]
    fn test_list_scripts_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let scripts_dir = temp_dir.path();

        let scripts = list_scripts_on_disk(scripts_dir).unwrap();
        assert_eq!(scripts.len(), 0);
    }

    #[test]
    fn test_list_scripts_nonexistent_dir() {
        let nonexistent = Path::new("/nonexistent/path/to/scripts");
        let scripts = list_scripts_on_disk(nonexistent).unwrap();
        assert_eq!(scripts.len(), 0);
    }

    #[test]
    fn test_load_nonexistent_script() {
        let temp_dir = TempDir::new().unwrap();
        let scripts_dir = temp_dir.path();

        let result = load_script_from_disk(scripts_dir, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_invalid_name() {
        let temp_dir = TempDir::new().unwrap();
        let scripts_dir = temp_dir.path();

        let script = ScriptDefinition {
            schema_version: 1,
            name: None,
            steps: vec![],
        };

        assert!(save_script_to_disk(scripts_dir, "invalid/name", &script).is_err());
        assert!(save_script_to_disk(scripts_dir, "", &script).is_err());
    }
}
