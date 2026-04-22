use std::collections::BTreeMap;

use serde_json::Value;

use crate::browser::BrowserContext;
use crate::CrawlError;

#[derive(Debug)]
struct FillFormInput {
    fields: BTreeMap<String, String>,
    submit: bool,
    form_selector: String,
}

fn parse_input(input: &Value) -> Result<FillFormInput, CrawlError> {
    let fields_value = input
        .get("fields")
        .ok_or_else(|| CrawlError::new("missing required field: fields"))?;

    let fields_obj = fields_value
        .as_object()
        .ok_or_else(|| CrawlError::new("fields must be an object"))?;

    if fields_obj.is_empty() {
        return Err(CrawlError::new("fields must not be empty"));
    }

    let mut fields = BTreeMap::new();
    for (key, value) in fields_obj {
        let val_str = value
            .as_str()
            .ok_or_else(|| CrawlError::new(format!("field value for '{key}' must be a string")))?;
        fields.insert(key.clone(), val_str.to_string());
    }

    let submit = input
        .get("submit")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let form_selector = input
        .get("form_selector")
        .and_then(Value::as_str)
        .unwrap_or("form")
        .to_string();

    Ok(FillFormInput {
        fields,
        submit,
        form_selector,
    })
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let params = parse_input(input)?;

    for (selector, value) in &params.fields {
        browser
            .acquire_bridge()
            .await
            .fill(selector, value)
            .await
            .map_err(|e| CrawlError::new(format!("failed to fill '{selector}': {e}")))?;
    }

    if params.submit {
        let js = format!(
            "document.querySelector('{}').submit()",
            params.form_selector.replace('\'', "\\'")
        );
        browser
            .acquire_bridge()
            .await
            .evaluate(&js)
            .await
            .map_err(|e| CrawlError::new(format!("failed to submit form: {e}")))?;
    }

    let field_count = params.fields.len();
    Ok(serde_json::json!({
        "success": true,
        "message": format!(
            "Filled {field_count} field(s){}",
            if params.submit { " and submitted form" } else { "" }
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_fields() {
        let input = json!({
            "fields": {"#name": "John", "#email": "john@example.com"}
        });
        let result = parse_input(&input).unwrap();
        assert_eq!(result.fields.len(), 2);
        assert_eq!(result.fields["#name"], "John");
        assert_eq!(result.fields["#email"], "john@example.com");
        assert!(!result.submit);
        assert_eq!(result.form_selector, "form");
    }

    #[test]
    fn parse_with_submit_and_form_selector() {
        let input = json!({
            "fields": {"#q": "rust"},
            "submit": true,
            "form_selector": "#search-form"
        });
        let result = parse_input(&input).unwrap();
        assert!(result.submit);
        assert_eq!(result.form_selector, "#search-form");
    }

    #[test]
    fn parse_missing_fields_returns_error() {
        let input = json!({});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("fields"));
    }

    #[test]
    fn parse_empty_fields_returns_error() {
        let input = json!({"fields": {}});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn parse_non_object_fields_returns_error() {
        let input = json!({"fields": "not an object"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("object"));
    }

    #[test]
    fn parse_non_string_field_value_returns_error() {
        let input = json!({"fields": {"#name": 42}});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("string"));
    }

    #[test]
    fn parse_defaults_submit_false_and_form_selector() {
        let input = json!({"fields": {"#x": "y"}});
        let result = parse_input(&input).unwrap();
        assert!(!result.submit);
        assert_eq!(result.form_selector, "form");
    }
}
