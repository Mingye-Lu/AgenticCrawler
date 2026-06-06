use serde_json::Value;

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

const MAX_PAGE_MAP_LINKS: usize = 50;
const MAX_PAGE_MAP_FORMS: usize = 10;
const MAX_PAGE_MAP_LANDMARKS: usize = 20;

fn normalized_page_map_url(url: &str) -> String {
    match url.split_once('#') {
        Some((_, frag)) if frag.starts_with('/') || frag.starts_with("!/") => url.to_string(),
        Some((base, _)) => base.to_string(),
        None => url.to_string(),
    }
}

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
    value
        .get_mut(key)
        .and_then(Value::as_array_mut)
        .is_some_and(|items| {
            let was_truncated = items.len() > max_len;
            if was_truncated {
                items.truncate(max_len);
            }
            was_truncated
        })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let scope = input.get("scope").and_then(Value::as_str);

    let mut result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .page_map(scope)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    apply_page_map_caps(&mut result);

    if scope.is_none() {
        let url = result
            .get("meta")
            .and_then(|m| m.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        let cache_key = normalized_page_map_url(url);

        if let Some(prev_url) = browser.snapshot_url() {
            if prev_url != cache_key.as_str() {
                browser.ref_map_mut().clear();
            }
        }

        if let Some(elements) = result
            .get_mut("interactive")
            .and_then(|interactive| interactive.get_mut("elements"))
            .and_then(Value::as_array_mut)
        {
            for el in elements.iter_mut() {
                let selector = el.get("selector").and_then(Value::as_str).map(String::from);
                if let Some(selector) = selector {
                    let role = el
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = el
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let ref_id = browser
                        .ref_map_mut()
                        .assign_or_reuse(&selector, &role, &name);
                    if let Some(obj) = el.as_object_mut() {
                        obj.insert("ref".to_string(), Value::String(format!("@{ref_id}")));
                    }
                }
            }
        }

        browser.set_page_snapshot(cache_key, result.clone());
    }

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
                "counts": { "buttons": 1, "inputs": 2, "selects": 3, "textareas": 4, "total": 10 },
                "elements": []
            },
            "meta": {
                "title": "Example",
                "description": "Description",
                "url": "https://example.com"
            }
        });

        apply_page_map_caps(&mut value);

        let object = value
            .as_object()
            .expect("page_map payload should be an object");
        for key in [
            "headings",
            "landmarks",
            "forms",
            "links",
            "interactive",
            "meta",
        ] {
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

    #[test]
    fn page_map_interactive_backward_compat_shape() {
        let value = json!({
            "interactive": {
                "counts": { "buttons": 3, "inputs": 2, "selects": 1, "textareas": 0, "total": 6 },
                "elements": [
                    {"tag": "button", "text": "Submit", "selector": "#submit"}
                ]
            }
        });

        let interactive = &value["interactive"];

        assert_eq!(interactive["counts"]["buttons"], json!(3));
        assert_eq!(interactive["counts"]["inputs"], json!(2));
        assert_eq!(interactive["counts"]["selects"], json!(1));
        assert_eq!(interactive["counts"]["textareas"], json!(0));
        assert_eq!(interactive["counts"]["total"], json!(6));
        assert!(interactive["elements"].is_array());
        assert_eq!(interactive["elements"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn page_map_interactive_elements_get_ref_field() {
        let value = json!({
            "interactive": {
                "counts": {"buttons": 1, "inputs": 0, "selects": 0, "textareas": 0, "total": 1},
                "elements": [
                    {"tag": "button", "text": "Submit", "selector": "#submit", "role": "button"}
                ]
            },
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "meta": {"title": "T", "description": "", "url": "https://example.com"}
        });

        assert!(value["interactive"]["elements"][0].get("ref").is_none());
    }

    #[test]
    fn page_map_scope_not_found_response() {
        let value = json!({
            "scope_not_found": true,
            "scope": "[role='dialog']",
            "headings": [],
            "landmarks": [],
            "forms": [],
            "links": [],
            "interactive": {
                "counts": { "buttons": 0, "inputs": 0, "selects": 0, "textareas": 0, "total": 0 },
                "elements": []
            },
            "meta": { "title": "Test", "description": "", "url": "https://example.com" },
            "total_landmarks": 0,
            "total_forms": 0,
            "total_links": 0
        });

        assert_eq!(value["scope_not_found"], json!(true));
        assert_eq!(value["scope"], json!("[role='dialog']"));
        assert!(value["headings"].as_array().unwrap().is_empty());
    }
}
