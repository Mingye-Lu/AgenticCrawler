use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::timeout;

use crate::BrowserContext;

use super::page_map::apply_page_map_caps;

const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);

/// Build structured page state from a raw `page_map` value (full, no diff).
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

fn extract_url(pm: &Value) -> &str {
    pm.get("meta")
        .and_then(|m| m.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn url_without_hash(url: &str) -> &str {
    url.split_once('#').map_or(url, |(base, _)| base)
}

fn extract_title(pm: &Value) -> &str {
    pm.get("meta")
        .and_then(|m| m.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn heading_key(h: &Value) -> Option<String> {
    let text = h.get("text")?.as_str()?;
    let level = h.get("level")?.as_u64()?;
    Some(format!("{level}:{text}"))
}

fn link_key(l: &Value) -> Option<String> {
    let href = l.get("href")?.as_str()?;
    let text = l.get("text").and_then(Value::as_str).unwrap_or("");
    Some(format!("{text}\x00{href}"))
}

fn landmark_key(lm: &Value) -> Option<String> {
    let tag = lm.get("tag")?.as_str()?;
    let role = lm.get("role").and_then(Value::as_str).unwrap_or("");
    let id = lm.get("id").and_then(Value::as_str).unwrap_or("");
    Some(format!("{tag}\x00{role}\x00{id}"))
}

fn collect_keys(items: &[Value], key_fn: fn(&Value) -> Option<String>) -> HashSet<String> {
    items.iter().filter_map(key_fn).collect()
}

fn diff_array(
    prev_items: &[Value],
    curr_items: &[Value],
    key_fn: fn(&Value) -> Option<String>,
) -> (Vec<Value>, Vec<Value>) {
    let prev_keys = collect_keys(prev_items, key_fn);
    let curr_keys = collect_keys(curr_items, key_fn);

    let added: Vec<Value> = curr_items
        .iter()
        .filter(|item| {
            key_fn(item)
                .as_ref()
                .is_some_and(|k| !prev_keys.contains(k))
        })
        .cloned()
        .collect();

    let removed: Vec<Value> = prev_items
        .iter()
        .filter(|item| {
            key_fn(item)
                .as_ref()
                .is_some_and(|k| !curr_keys.contains(k))
        })
        .cloned()
        .collect();

    (added, removed)
}

fn get_array<'a>(pm: &'a Value, key: &str) -> &'a [Value] {
    pm.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

pub fn build_diff_page_state(prev: &Value, current: &mut Value) -> Value {
    apply_page_map_caps(current);

    let url = extract_url(current).to_string();
    let title = extract_title(current).to_string();

    let (added_headings, removed_headings) =
        diff_array(get_array(prev, "headings"), get_array(current, "headings"), heading_key);
    let (added_links, removed_links) =
        diff_array(get_array(prev, "links"), get_array(current, "links"), link_key);
    let (added_landmarks, removed_landmarks) =
        diff_array(get_array(prev, "landmarks"), get_array(current, "landmarks"), landmark_key);

    let has_changes = !added_headings.is_empty()
        || !removed_headings.is_empty()
        || !added_links.is_empty()
        || !removed_links.is_empty()
        || !added_landmarks.is_empty()
        || !removed_landmarks.is_empty();

    if !has_changes {
        return json!({
            "url": url,
            "title": title,
            "changed": false
        });
    }

    let total_prev = get_array(prev, "headings").len()
        + get_array(prev, "links").len()
        + get_array(prev, "landmarks").len();
    let total_changed = added_headings.len()
        + removed_headings.len()
        + added_links.len()
        + removed_links.len()
        + added_landmarks.len()
        + removed_landmarks.len();

    if total_prev > 0 && total_changed > total_prev {
        return build_page_state_from_map(current.clone());
    }

    let mut changes = serde_json::Map::new();
    if !added_headings.is_empty() {
        changes.insert("added_headings".into(), Value::Array(added_headings));
    }
    if !removed_headings.is_empty() {
        changes.insert("removed_headings".into(), Value::Array(removed_headings));
    }
    if !added_links.is_empty() {
        changes.insert("added_links".into(), Value::Array(added_links));
    }
    if !removed_links.is_empty() {
        changes.insert("removed_links".into(), Value::Array(removed_links));
    }
    if !added_landmarks.is_empty() {
        changes.insert("added_landmarks".into(), Value::Array(added_landmarks));
    }
    if !removed_landmarks.is_empty() {
        changes.insert("removed_landmarks".into(), Value::Array(removed_landmarks));
    }

    json!({
        "url": url,
        "title": title,
        "changed": true,
        "changes": changes
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
/// Calls `page_map_feedback` on the bridge (which may return a pre-computed
/// diff from the extension, or a full `page_map` from Playwright). If the
/// response is already a diff, passes it through. Otherwise applies
/// Rust-side differential comparison against the cached snapshot.
pub async fn post_action_page_state(browser: &mut BrowserContext) -> Value {
    let result = timeout(FEEDBACK_TIMEOUT, async {
        let mut bridge = browser.acquire_bridge().await?;
        bridge.page_map_feedback().await
    })
    .await;

    match result {
        Ok(Ok(pm)) => {
            if pm.get("changed").is_some() {
                return pm;
            }

            let mut pm = pm;
            let full_url = extract_url(&pm).to_string();
            let cache_key = url_without_hash(&full_url).to_string();

            let response = match browser.page_snapshot_for_url(&cache_key) {
                Some(prev) => build_diff_page_state(prev, &mut pm),
                None => build_page_state_from_map(pm.clone()),
            };

            browser.set_page_snapshot(cache_key, pm);
            response
        }
        _ => fallback_value(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{build_diff_page_state, build_page_state_from_map, fallback_value};

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

    fn base_page_map() -> serde_json::Value {
        json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"},
                {"text": "About", "href": "https://example.com/about", "selector": "a.about"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        })
    }

    #[test]
    fn diff_no_changes_returns_changed_false() {
        let prev = base_page_map();
        let mut current = base_page_map();

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], false);
        assert_eq!(result["url"], "https://example.com/page");
        assert_eq!(result["title"], "Test Page");
        assert!(result.get("changes").is_none());
    }

    #[test]
    fn diff_modal_added_shows_only_new_elements() {
        let prev = base_page_map();
        let mut current = json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"},
                {"level": 2, "text": "Sign Up", "selector": "div.modal > h2", "char_count": 50, "preview": "Create account"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"},
                {"tag": "dialog", "role": "dialog", "id": "signup", "selector": "#signup", "text_preview": "Sign up form"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"},
                {"text": "About", "href": "https://example.com/about", "selector": "a.about"},
                {"text": "Login instead", "href": "https://example.com/login", "selector": "div.modal a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        assert_eq!(changes["added_headings"].as_array().unwrap().len(), 1);
        assert_eq!(changes["added_headings"][0]["text"], "Sign Up");
        assert_eq!(changes["added_links"].as_array().unwrap().len(), 1);
        assert_eq!(changes["added_links"][0]["text"], "Login instead");
        assert_eq!(changes["added_landmarks"].as_array().unwrap().len(), 1);
        assert_eq!(changes["added_landmarks"][0]["tag"], "dialog");
        assert!(changes.get("removed_headings").is_none());
        assert!(changes.get("removed_links").is_none());
        assert!(changes.get("removed_landmarks").is_none());
    }

    #[test]
    fn diff_modal_closed_shows_removed_elements() {
        let prev = json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"},
                {"level": 2, "text": "Sign Up", "selector": "div.modal > h2", "char_count": 50, "preview": "Create"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"},
                {"text": "Login instead", "href": "https://example.com/login", "selector": "div.modal a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        });

        let mut current = json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        assert_eq!(changes["removed_headings"].as_array().unwrap().len(), 1);
        assert_eq!(changes["removed_headings"][0]["text"], "Sign Up");
        assert_eq!(changes["removed_links"].as_array().unwrap().len(), 1);
        assert_eq!(changes["removed_links"][0]["href"], "https://example.com/login");
        assert!(changes.get("added_headings").is_none());
    }

    #[test]
    fn diff_ignores_selector_changes_for_same_content() {
        let prev = json!({
            "headings": [
                {"level": 1, "text": "Title", "selector": "div:nth-of-type(1) > h1", "char_count": 10, "preview": "T"}
            ],
            "landmarks": [],
            "links": [
                {"text": "Link", "href": "https://example.com/page", "selector": "div:nth-of-type(1) > a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": "https://example.com/page", "description": ""}
        });

        let mut current = json!({
            "headings": [
                {"level": 1, "text": "Title", "selector": "div:nth-of-type(2) > h1", "char_count": 10, "preview": "T"}
            ],
            "landmarks": [],
            "links": [
                {"text": "Link", "href": "https://example.com/page", "selector": "div:nth-of-type(2) > a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": "https://example.com/page", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], false);
    }

    #[test]
    fn diff_too_large_falls_back_to_full_page_map() {
        let prev = json!({
            "headings": [
                {"level": 1, "text": "Home", "selector": "h1", "char_count": 10, "preview": "Home"},
                {"level": 2, "text": "Features", "selector": "h2", "char_count": 10, "preview": "Feat"}
            ],
            "landmarks": [
                {"tag": "main", "role": "main", "id": "content", "selector": "#content", "text_preview": "Main"}
            ],
            "links": [
                {"text": "Link A", "href": "https://example.com/a", "selector": "a"},
                {"text": "Link B", "href": "https://example.com/b", "selector": "a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Home", "url": "https://example.com/", "description": ""}
        });

        let mut current = json!({
            "headings": [
                {"level": 1, "text": "About Us", "selector": "h1", "char_count": 20, "preview": "About"},
                {"level": 2, "text": "Team", "selector": "h2.team", "char_count": 15, "preview": "Team"},
                {"level": 2, "text": "Mission", "selector": "h2.mission", "char_count": 15, "preview": "Mission"}
            ],
            "landmarks": [
                {"tag": "main", "role": "main", "id": "about", "selector": "#about", "text_preview": "About"}
            ],
            "links": [
                {"text": "Contact", "href": "https://example.com/contact", "selector": "a"},
                {"text": "Join Us", "href": "https://example.com/careers", "selector": "a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "About", "url": "https://example.com/", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert!(
            result.get("page_map").is_some(),
            "should fall back to full page_map when diff is too large"
        );
        assert_eq!(result["url"], "https://example.com/");
    }

    #[test]
    fn url_without_hash_strips_fragment() {
        use super::url_without_hash;

        assert_eq!(url_without_hash("https://example.com/page#section"), "https://example.com/page");
        assert_eq!(url_without_hash("https://example.com/page"), "https://example.com/page");
        assert_eq!(url_without_hash("https://app.com/#/dashboard"), "https://app.com/");
        assert_eq!(url_without_hash("unknown"), "unknown");
    }
}
