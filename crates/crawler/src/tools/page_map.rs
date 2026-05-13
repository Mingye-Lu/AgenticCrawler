use serde_json::Value;

use crate::browser::BrowserContext;
use crate::{ToolEffect, ToolError};

const MAX_PAGE_MAP_LINKS: usize = 50;
const MAX_PAGE_MAP_FORMS: usize = 10;
const MAX_PAGE_MAP_LANDMARKS: usize = 20;

pub fn apply_page_map_caps(value: &mut Value) {
    let truncated_links = truncate_array_field(value, "links", MAX_PAGE_MAP_LINKS);
    let truncated_forms = truncate_array_field(value, "forms", MAX_PAGE_MAP_FORMS);
    let truncated_landmarks = truncate_array_field(value, "landmarks", MAX_PAGE_MAP_LANDMARKS);

    if let Some(object) = value.as_object_mut() {
        object.insert("truncated_links".to_string(), Value::Bool(truncated_links));
        object.insert("truncated_forms".to_string(), Value::Bool(truncated_forms));
        object.insert(
            "truncated_landmarks".to_string(),
            Value::Bool(truncated_landmarks),
        );
    }
}

fn truncate_array_field(value: &mut Value, key: &str, max_len: usize) -> bool {
    value.get_mut(key).and_then(Value::as_array_mut).is_some_and(|items| {
        let was_truncated = items.len() > max_len;
        if was_truncated {
            items.truncate(max_len);
        }
        was_truncated
    })
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let _ = input;

    let mut result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolError(e.to_string()))?
        .page_map()
        .await
        .map_err(|e| ToolError(e.to_string()))?;

    apply_page_map_caps(&mut result);

    Ok(ToolEffect::reply_json(&result))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::apply_page_map_caps;

    #[test]
    fn page_map_response_structure_has_all_sections() {
        let mut value = json!({
            "headings": [
                {
                    "level": 1,
                    "text": "Title",
                    "id": "title",
                    "selector": "#title",
                    "char_count": 42,
                    "preview": "Preview"
                }
            ],
            "landmarks": [
                {
                    "tag": "main",
                    "role": "main",
                    "id": "content",
                    "selector": "#content",
                    "text_preview": "Main content"
                }
            ],
            "forms": [
                {
                    "action": "/submit",
                    "method": "post",
                    "id": "contact",
                    "selector": "#contact",
                    "fields": [
                        {
                            "name": "email",
                            "type": "email",
                            "label": "Email",
                            "required": true
                        }
                    ]
                }
            ],
            "links": [
                {
                    "text": "Docs",
                    "href": "https://example.com/docs",
                    "selector": "a"
                }
            ],
            "interactive": {
                "buttons": 1,
                "inputs": 2,
                "selects": 3,
                "textareas": 4
            },
            "meta": {
                "title": "Example",
                "description": "Description",
                "url": "https://example.com"
            }
        });

        apply_page_map_caps(&mut value);

        let object = value.as_object().expect("page_map payload should be an object");
        for key in ["headings", "landmarks", "forms", "links", "interactive", "meta"] {
            assert!(object.contains_key(key), "missing key: {key}");
        }
    }

    #[test]
    fn page_map_headings_format_matches_spec() {
        let mut value = json!({
            "headings": [
                {
                    "level": 2,
                    "text": "Overview",
                    "id": "overview",
                    "selector": "#overview",
                    "char_count": 12,
                    "preview": "Quick summary"
                }
            ],
            "landmarks": [],
            "forms": [],
            "links": [],
            "interactive": {},
            "meta": {}
        });

        apply_page_map_caps(&mut value);

        let heading = value
            .get("headings")
            .and_then(Value::as_array)
            .and_then(|headings| headings.first())
            .and_then(Value::as_object)
            .expect("expected a heading object");

        for key in ["level", "text", "id", "selector"] {
            assert!(heading.contains_key(key), "missing heading field: {key}");
        }
    }

    #[test]
    fn page_map_caps_links_at_50() {
        let mut value = json!({
            "headings": [],
            "landmarks": [],
            "forms": [],
            "links": (0..100)
                .map(|index| json!({
                    "text": format!("Link {index}"),
                    "href": format!("https://example.com/{index}"),
                    "selector": format!("a:nth-of-type({})", index + 1)
                }))
                .collect::<Vec<_>>(),
            "interactive": {},
            "meta": {}
        });

        apply_page_map_caps(&mut value);

        assert_eq!(value["links"].as_array().map(Vec::len), Some(50));
        assert_eq!(value["truncated_links"], json!(true));
    }

    #[test]
    fn page_map_caps_forms_at_10() {
        let mut value = json!({
            "headings": [],
            "landmarks": [],
            "forms": (0..20)
                .map(|index| json!({
                    "action": format!("/submit/{index}"),
                    "method": "post",
                    "id": format!("form-{index}"),
                    "selector": format!("#form-{index}"),
                    "fields": []
                }))
                .collect::<Vec<_>>(),
            "links": [],
            "interactive": {},
            "meta": {}
        });

        apply_page_map_caps(&mut value);

        assert_eq!(value["forms"].as_array().map(Vec::len), Some(10));
        assert_eq!(value["truncated_forms"], json!(true));
    }

    #[test]
    fn page_map_caps_landmarks_at_20() {
        let mut value = json!({
            "headings": [],
            "landmarks": (0..30)
                .map(|index| json!({
                    "tag": "section",
                    "role": "navigation",
                    "id": format!("landmark-{index}"),
                    "selector": format!("#landmark-{index}"),
                    "text_preview": format!("Landmark {index}")
                }))
                .collect::<Vec<_>>(),
            "forms": [],
            "links": [],
            "interactive": {},
            "meta": {}
        });

        apply_page_map_caps(&mut value);

        assert_eq!(value["landmarks"].as_array().map(Vec::len), Some(20));
        assert_eq!(value["truncated_landmarks"], json!(true));
    }

    #[test]
    fn page_map_no_truncation_when_under_cap() {
        let mut value = json!({
            "headings": [],
            "landmarks": [],
            "forms": [],
            "links": (0..5)
                .map(|index| json!({
                    "text": format!("Link {index}"),
                    "href": format!("https://example.com/{index}"),
                    "selector": format!("a:nth-of-type({})", index + 1)
                }))
                .collect::<Vec<_>>(),
            "interactive": {},
            "meta": {}
        });

        apply_page_map_caps(&mut value);

        assert_eq!(value["links"].as_array().map(Vec::len), Some(5));
        assert_eq!(value["truncated_links"], json!(false));
    }
}
