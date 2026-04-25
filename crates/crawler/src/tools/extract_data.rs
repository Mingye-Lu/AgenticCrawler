use serde_json::Value;

use crate::browser::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolError};

#[derive(Debug)]
struct ExtractDataInput {
    instruction: String,
    data: Value,
}

fn parse_input(input: &Value) -> Result<ExtractDataInput, CrawlError> {
    let instruction = input
        .get("instruction")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: instruction"))?;

    let data = input
        .get("data")
        .ok_or_else(|| CrawlError::new("missing required field: data"))?
        .clone();

    Ok(ExtractDataInput {
        instruction: instruction.to_string(),
        data,
    })
}

#[allow(clippy::unused_async)]
pub async fn execute(
    input: &Value,
    _browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolError> {
    let params = parse_input(input)?;

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "instruction": params.instruction,
        "data": params.data
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_input() {
        let input = json!({
            "instruction": "Extract all product names",
            "data": ["Product A", "Product B"]
        });
        let result = parse_input(&input).unwrap();
        assert_eq!(result.instruction, "Extract all product names");
        assert_eq!(result.data, json!(["Product A", "Product B"]));
    }

    #[test]
    fn parse_missing_instruction_returns_error() {
        let input = json!({"data": []});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("instruction"));
    }

    #[test]
    fn parse_missing_data_returns_error() {
        let input = json!({"instruction": "extract stuff"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("data"));
    }

    #[test]
    fn parse_complex_data_structure() {
        let input = json!({
            "instruction": "Extract products",
            "data": {
                "products": [
                    {"name": "A", "price": 10},
                    {"name": "B", "price": 20}
                ]
            }
        });
        let result = parse_input(&input).unwrap();
        assert!(result.data.is_object());
        assert_eq!(result.data["products"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parse_null_data_is_accepted() {
        let input = json!({"instruction": "nothing", "data": null});
        let result = parse_input(&input).unwrap();
        assert!(result.data.is_null());
    }

    #[test]
    fn parse_non_string_instruction_returns_error() {
        let input = json!({"instruction": 42, "data": {}});
        assert!(parse_input(&input).is_err());
    }
}
