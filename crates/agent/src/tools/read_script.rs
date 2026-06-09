use serde_json::Value;
use std::fs;

use crate::{CrawlError, ToolEffect, ToolExecutionError};
use acrawl_core::config_home_dir;

pub fn parse_input(input: &Value) -> Result<String, CrawlError> {
    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("read_script requires 'name' field"))?
        .to_string();

    script::persistence::validate_script_name(&name).map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(name)
}

pub fn execute(input: &Value) -> Result<ToolEffect, ToolExecutionError> {
    let name = parse_input(input)?;

    let script_path = config_home_dir()
        .join("scripts")
        .join(format!("{name}.json"));

    if !script_path.exists() {
        return Err(ToolExecutionError::new(format!(
            "script '{name}' not found",
        )));
    }

    let content = fs::read_to_string(&script_path)
        .map_err(|e| ToolExecutionError::new(format!("failed to read script file: {e}")))?;

    Ok(ToolEffect::Reply(content))
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
        let input = json!({});
        assert!(parse_input(&input).is_err());
    }
}
