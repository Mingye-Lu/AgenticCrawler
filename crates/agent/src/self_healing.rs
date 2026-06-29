use serde_json::Value;

/// Extract the first element ref (for example `@e5`) from tool input JSON.
#[must_use]
pub fn extract_element_ref(input: &Value) -> Option<String> {
    if let Some(sel) = input.get("selector").and_then(Value::as_str) {
        if sel.starts_with('@') {
            return Some(sel.to_string());
        }
    }

    if let Some(sel) = input.get("form_selector").and_then(Value::as_str) {
        if sel.starts_with('@') {
            return Some(sel.to_string());
        }
    }

    if let Some(fields) = input.get("fields").and_then(Value::as_object) {
        if let Some((key, _)) = fields.iter().find(|(key, _)| key.starts_with('@')) {
            return Some(key.clone());
        }
    }

    if let Some(s) = input.as_str() {
        if s.starts_with('@') {
            return Some(s.to_string());
        }
    }

    None
}

fn normalized_hint(hint: Option<&str>) -> Option<String> {
    let normalized = hint?.trim().to_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn text_matches(candidate: &str, hint: &str) -> bool {
    let candidate = candidate.trim().to_lowercase();
    !candidate.is_empty() && (candidate.contains(hint) || hint.contains(&candidate))
}

/// Attempt to find a replacement selector from a fresh `page_map`.
#[must_use]
pub fn find_healed_selector(
    original_ref: &str,
    page_map: &Value,
    original_text_hint: Option<&str>,
) -> Option<String> {
    let interactive = page_map
        .get("interactive")
        .and_then(|i| i.get("elements"))
        .and_then(Value::as_array)?;
    let hint = normalized_hint(original_text_hint)?;

    for elem in interactive {
        for field in ["name", "text"] {
            if let Some(text) = elem.get(field).and_then(Value::as_str) {
                if text_matches(text, &hint) {
                    if let Some(new_ref) = elem.get("ref").and_then(Value::as_str) {
                        if new_ref != original_ref {
                            return Some(new_ref.to_string());
                        }
                    }
                    if let Some(sel) = elem.get("selector").and_then(Value::as_str) {
                        return Some(sel.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Build a patched input with the healed selector.
#[must_use]
pub fn patch_selector(input: &Value, old_selector: &str, new_selector: &str) -> Value {
    let mut patched = input.clone();
    let Some(obj) = patched.as_object_mut() else {
        return patched;
    };

    if obj
        .get("selector")
        .and_then(Value::as_str)
        .is_some_and(|sel| sel == old_selector)
    {
        obj.insert(
            "selector".to_string(),
            Value::String(new_selector.to_string()),
        );
    }

    if obj
        .get("form_selector")
        .and_then(Value::as_str)
        .is_some_and(|sel| sel == old_selector)
    {
        obj.insert(
            "form_selector".to_string(),
            Value::String(new_selector.to_string()),
        );
    }

    if let Some(fields) = obj.get_mut("fields").and_then(Value::as_object_mut) {
        if let Some(value) = fields.remove(old_selector) {
            fields.insert(new_selector.to_string(), value);
        }
    }

    patched
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_element_ref_from_selector_field() {
        let input = json!({"selector": "@e5", "other": "data"});
        assert_eq!(extract_element_ref(&input), Some("@e5".to_string()));
    }

    #[test]
    fn extract_element_ref_from_fields_key() {
        let input = json!({"fields": {"@e5": "hello"}, "submit": false});
        assert_eq!(extract_element_ref(&input), Some("@e5".to_string()));
    }

    #[test]
    fn extract_none_for_css_selector() {
        let input = json!({"selector": "button.submit"});
        assert_eq!(extract_element_ref(&input), None);
    }

    #[test]
    fn find_healed_selector_by_name_match() {
        let page_map = json!({
            "interactive": {
                "elements": [
                    {"ref": "@e7", "name": "Submit", "text": "Submit", "tag": "button"},
                    {"ref": "@e8", "name": "Cancel", "text": "Cancel", "tag": "button"}
                ]
            }
        });
        let healed = find_healed_selector("@e5", &page_map, Some("submit"));
        assert_eq!(healed, Some("@e7".to_string()));
    }

    #[test]
    fn find_healed_selector_no_match_returns_none() {
        let page_map = json!({
            "interactive": {
                "elements": [
                    {"ref": "@e7", "text": "Login", "tag": "button"}
                ]
            }
        });
        let healed = find_healed_selector("@e5", &page_map, Some("submit"));
        assert!(healed.is_none());
    }

    #[test]
    fn patch_selector_replaces_top_level_field() {
        let input = json!({"selector": "@e5", "other": "data"});
        let patched = patch_selector(&input, "@e5", "@e7");
        assert_eq!(patched.get("selector").and_then(Value::as_str), Some("@e7"));
        assert_eq!(patched.get("other").and_then(Value::as_str), Some("data"));
    }

    #[test]
    fn patch_selector_rewrites_fill_form_field_key() {
        let input = json!({"fields": {"@e5": "john@example.com", "#name": "John"}});
        let patched = patch_selector(&input, "@e5", "@e9");
        let fields = patched.get("fields").and_then(Value::as_object).unwrap();
        assert_eq!(
            fields.get("@e9").and_then(Value::as_str),
            Some("john@example.com")
        );
        assert_eq!(fields.get("#name").and_then(Value::as_str), Some("John"));
        assert!(!fields.contains_key("@e5"));
    }
}
