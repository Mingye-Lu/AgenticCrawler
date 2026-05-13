use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::timeout;

use crate::browser::BrowserContext;

use super::page_map::apply_page_map_caps;

const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);

/// Build structured page state from a raw `page_map` value.
///
/// Applies caps, extracts url/title from meta, and strips `forms` and
/// `interactive` fields to keep interaction responses concise.
pub fn build_page_state_from_map(mut pm: Value) -> Value {
    apply_page_map_caps(&mut pm);

    let url = pm
        .get("meta")
        .and_then(|m| m.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let title = pm
        .get("meta")
        .and_then(|m| m.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    if let Some(obj) = pm.as_object_mut() {
        obj.remove("forms");
        obj.remove("interactive");
    }

    json!({
        "url": url,
        "title": title,
        "page_map": pm
    })
}

fn fallback_value() -> Value {
    json!({
        "url": "unknown",
        "title": "unknown",
        "page_map": null
    })
}

/// Best-effort post-action page state for interaction tool responses.
///
/// Calls the bridge `page_map` with a 3-second timeout. On success, returns
/// structured url + title + trimmed `page_map`. On any failure, returns a
/// fallback with null `page_map` — never propagates errors.
pub async fn post_action_page_state(browser: &mut BrowserContext) -> Value {
    let result = timeout(FEEDBACK_TIMEOUT, async {
        let mut bridge = browser.acquire_bridge().await?;
        bridge.page_map().await
    })
    .await;

    match result {
        Ok(Ok(pm)) => build_page_state_from_map(pm),
        _ => fallback_value(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{build_page_state_from_map, fallback_value};

    #[test]
    fn feedback_fallback_value_has_correct_shape() {
        let val = fallback_value();

        assert_eq!(val["url"], "unknown");
        assert_eq!(val["title"], "unknown");
        assert!(val["page_map"].is_null(), "page_map should be null");

        let obj = val.as_object().expect("fallback should be an object");
        assert!(obj.contains_key("url"));
        assert!(obj.contains_key("title"));
        assert!(obj.contains_key("page_map"));
        assert_eq!(obj.len(), 3, "fallback should have exactly 3 keys");
    }

    #[test]
    fn feedback_success_value_from_mock_page_map() {
        let mock_page_map = json!({
            "headings": [
                {
                    "level": 1,
                    "text": "Welcome",
                    "id": "welcome",
                    "selector": "#welcome",
                    "char_count": 100,
                    "preview": "Welcome to the site"
                }
            ],
            "landmarks": [
                {
                    "tag": "nav",
                    "role": "navigation",
                    "id": "nav",
                    "selector": "#nav",
                    "text_preview": "Main nav"
                }
            ],
            "forms": [
                {
                    "action": "/login",
                    "method": "post",
                    "id": "login-form",
                    "selector": "#login-form",
                    "fields": [{"name": "email", "type": "email"}]
                }
            ],
            "links": [
                {
                    "text": "Home",
                    "href": "https://example.com",
                    "selector": "a.home"
                }
            ],
            "interactive": {
                "buttons": 3,
                "inputs": 2,
                "selects": 1,
                "textareas": 0
            },
            "meta": {
                "title": "Example Page",
                "url": "https://example.com/page",
                "description": "A test page"
            }
        });

        let result = build_page_state_from_map(mock_page_map);

        assert_eq!(result["url"], "https://example.com/page");
        assert_eq!(result["title"], "Example Page");
        assert!(!result["page_map"].is_null(), "page_map should not be null");

        let pm = &result["page_map"];
        assert!(pm["headings"].is_array());
        assert!(pm["landmarks"].is_array());
        assert!(pm["links"].is_array());
        assert!(pm["meta"].is_object());

        assert!(
            pm.get("forms").is_none(),
            "forms should be removed from page_map"
        );
        assert!(
            pm.get("interactive").is_none(),
            "interactive should be removed from page_map"
        );

        assert!(pm.get("truncated_links").is_some());
        assert!(pm.get("truncated_forms").is_some());
        assert!(pm.get("truncated_landmarks").is_some());
    }
}
