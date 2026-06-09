use serde_json::Value;
use std::fs;

use crate::{CrawlError, ToolEffect, ToolExecutionError};
use acrawl_core::config_home_dir;

pub fn parse_input(input: &Value) -> Result<(String, Value), CrawlError> {
    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("save_script requires 'name' field"))?
        .to_string();

    let script = input
        .get("script")
        .ok_or_else(|| CrawlError::new("save_script requires 'script' field"))?
        .clone();

    script::persistence::validate_script_name(&name).map_err(|e| CrawlError::new(e.to_string()))?;

    Ok((name, script))
}

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let (name, script) = parse_input(input)?;

    // Validate script JSON via parser
    script::parser::parse_script(&script)
        .map_err(|e| ToolExecutionError::new(format!("invalid script: {e}")))?;

    // Create scripts directory
    let scripts_dir = config_home_dir().join("scripts");
    fs::create_dir_all(&scripts_dir)
        .map_err(|e| ToolExecutionError::new(format!("failed to create scripts directory: {e}")))?;

    // Write script file
    let script_path = scripts_dir.join(format!("{name}.json"));
    let script_json = serde_json::to_string_pretty(&script)
        .map_err(|e| ToolExecutionError::new(format!("failed to serialize script: {e}")))?;

    fs::write(&script_path, script_json)
        .map_err(|e| ToolExecutionError::new(format!("failed to write script file: {e}")))?;

    Ok(ToolEffect::Reply(format!("Script '{name}' saved")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_script_name_rejects_slashes() {
        assert!(script::persistence::validate_script_name("my/script").is_err());
        assert!(script::persistence::validate_script_name("my\\script").is_err());
    }

    #[test]
    fn validates_script_name_rejects_dots() {
        assert!(script::persistence::validate_script_name("my.script").is_err());
        assert!(script::persistence::validate_script_name("..").is_err());
    }

    #[test]
    fn validates_script_name_rejects_leading_dash() {
        assert!(script::persistence::validate_script_name("-script").is_err());
    }

    #[test]
    fn validates_script_name_rejects_null_bytes() {
        assert!(script::persistence::validate_script_name("my\0script").is_err());
    }

    #[test]
    fn validates_script_name_accepts_valid_names() {
        assert!(script::persistence::validate_script_name("my_script").is_ok());
        assert!(script::persistence::validate_script_name("my-script").is_ok());
        assert!(script::persistence::validate_script_name("MyScript123").is_ok());
    }

    #[test]
    fn parses_input_requires_name() {
        let input = json!({"script": {}});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn parses_input_requires_script() {
        let input = json!({"name": "test"});
        assert!(parse_input(&input).is_err());
    }
}
