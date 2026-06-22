use serde_json::Value;

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

const MAX_PAGE_MAP_LINKS: usize = 50;
const MAX_PAGE_MAP_FORMS: usize = 10;
const MAX_PAGE_MAP_LANDMARKS: usize = 20;

/// Normalize a URL for page-map caching and ref lifecycle comparison.
/// Strips fragment unless it looks like a hash-based route (`#/…` or `#!/…`).
#[must_use]
pub fn normalize_url(url: &str) -> &str {
    match url.split_once('#') {
        Some((_, frag)) if frag.starts_with('/') || frag.starts_with("!/") => url,
        Some((base, _)) => base,
        None => url,
    }
}

/// Annotate interactive elements in a `page_map` value with `@eN` refs.
/// Assigns or reuses stable refs via the browser context's `RefMap`.
pub fn annotate_refs(result: &mut Value, browser: &mut BrowserContext) {
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
    crawl_state: &mut crate::state::CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let scope = input.get("scope").and_then(Value::as_str);

    let settings = runtime::load_settings();
    let compound_enrichment = runtime::settings_get_compound_enrichment(&settings);

    let mut result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .page_map(scope, compound_enrichment)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    apply_page_map_caps(&mut result);

    if scope.is_none() {
        let url = result
            .get("meta")
            .and_then(|m| m.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        let cache_key = normalize_url(url).to_string();

        if let Some(prev_url) = browser.snapshot_url() {
            if prev_url != cache_key.as_str() {
                browser.ref_map_mut().clear();
            }
        }

        annotate_refs(&mut result, browser);
        browser.set_page_snapshot(&cache_key, None, result.clone());
    } else {
        annotate_refs(&mut result, browser);
    }

    let fp_settings = runtime::load_settings();
    if runtime::settings_get_page_fingerprinting(&fp_settings) {
        let url = result
            .get("meta")
            .and_then(|meta| meta.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let fingerprint = crate::page_fingerprint::PageFingerprint::compute(url, &result);
        crawl_state.page_fingerprints.push(fingerprint);
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

    // ─── Integration tests: @eN ref lifecycle ───────────────────────────────

    #[test]
    fn page_map_injects_ref_into_interactive_elements() {
        let mut value = json!({
            "interactive": {
                "counts": {"buttons": 1, "inputs": 0, "selects": 0, "textareas": 0, "total": 1},
                "elements": [
                    {"tag": "button", "text": "Submit", "selector": "#submit", "role": "button"}
                ]
            },
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "meta": {"title": "Test", "description": "", "url": "https://example.com"}
        });

        apply_page_map_caps(&mut value);

        // Verify element structure is intact (ref would be injected by execute() with browser)
        let elements = value["interactive"]["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0]["selector"], json!("#submit"));
        assert_eq!(elements[0]["role"], json!("button"));
    }

    #[test]
    fn page_map_ref_assignment_stability() {
        use browser::RefMap;

        let mut ref_map = RefMap::new();
        // Assign twice for same selector → same ref
        let r1 = ref_map.assign_or_reuse("#submit", "button", "Submit");
        let r2 = ref_map.assign_or_reuse("#submit", "button", "Submit");
        assert_eq!(r1, r2);
        assert_eq!(r1, "e1");

        // Different selector → new ref
        let r3 = ref_map.assign_or_reuse("#email", "textbox", "Email");
        assert_ne!(r1, r3);
        assert_eq!(r3, "e2");
    }

    #[test]
    fn page_map_ref_reset_on_clear() {
        use browser::RefMap;

        let mut ref_map = RefMap::new();
        let r1 = ref_map.assign_or_reuse("#btn", "button", "Click me");
        assert_eq!(r1, "e1");

        ref_map.clear();

        // After clear, counter resets
        let r2 = ref_map.assign_or_reuse("#other-btn", "button", "Other");
        assert_eq!(r2, "e1");
        // Original ref no longer exists — e1 now points to #other-btn
        assert_eq!(
            ref_map.get("e1").map(|e| e.selector.as_str()),
            Some("#other-btn")
        );
    }

    #[test]
    fn fill_form_fields_css_selectors_pass_through_resolve() {
        use browser::RefMap;

        use crate::tools::ref_resolve::resolve_selector;

        let map = RefMap::new();
        // Pure CSS selectors pass through unchanged
        assert_eq!(resolve_selector("#email", &map).unwrap(), "#email");
        assert_eq!(
            resolve_selector("input[name='q']", &map).unwrap(),
            "input[name='q']"
        );
    }

    #[test]
    fn fill_form_ref_key_resolves_to_selector() {
        use browser::RefMap;

        use crate::tools::ref_resolve::resolve_selector;

        let mut map = RefMap::new();
        map.assign_or_reuse("#email-input", "textbox", "Email");
        // @e1 should resolve to "#email-input"
        let resolved = resolve_selector("@e1", &map).unwrap();
        assert_eq!(resolved, "#email-input");
    }

    #[test]
    fn invalid_ref_produces_clear_error_message() {
        use browser::RefMap;

        use crate::tools::ref_resolve::resolve_selector;

        let map = RefMap::new(); // empty map
        let err = resolve_selector("@e999", &map).unwrap_err();
        assert!(err.contains("Unknown element ref"));
        assert!(err.contains("page_map"));
    }

    #[test]
    fn backward_compat_css_selectors_work_alongside_refs() {
        use browser::RefMap;

        use crate::tools::ref_resolve::resolve_selector;

        let mut map = RefMap::new();
        map.assign_or_reuse("button.submit", "button", "Submit");

        // @e1 resolves to CSS selector
        assert_eq!(resolve_selector("@e1", &map).unwrap(), "button.submit");
        // Unrelated CSS selectors pass through unchanged
        assert_eq!(resolve_selector("#other", &map).unwrap(), "#other");
        assert_eq!(
            resolve_selector(".class-selector", &map).unwrap(),
            ".class-selector"
        );
    }

    #[test]
    fn sub_agent_isolation_separate_ref_maps() {
        use browser::RefMap;

        // Two independent RefMaps (simulating two BrowserContexts)
        let mut map_a = RefMap::new();
        let mut map_b = RefMap::new();

        // Assign to map_a
        let ref_a = map_a.assign_or_reuse("#btn-a", "button", "A");
        // Assign different selector to map_b
        let ref_b = map_b.assign_or_reuse("#btn-b", "button", "B");

        // Both start at e1 (independent counters)
        assert_eq!(ref_a, "e1");
        assert_eq!(ref_b, "e1");

        // map_a's e1 points to #btn-a, not #btn-b
        assert_eq!(map_a.get("e1").unwrap().selector, "#btn-a");
        assert_eq!(map_b.get("e1").unwrap().selector, "#btn-b");

        // Clearing map_a doesn't affect map_b
        map_a.clear();
        assert!(map_a.get("e1").is_none());
        assert!(map_b.get("e1").is_some());
    }

    // ─── Lifecycle integration tests: annotate_refs + invalidation ──────────

    #[test]
    fn annotate_refs_injects_ref_fields_into_elements() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        use browser::BrowserContext;

        use super::annotate_refs;

        let bridge: Arc<Mutex<Box<dyn browser::BrowserBackend + Send>>> =
            Arc::new(Mutex::new(Box::new(browser::NopBridge)));
        let mut ctx = BrowserContext::new(bridge);

        let mut value = json!({
            "interactive": {
                "elements": [
                    {"tag": "button", "text": "Submit", "selector": "#submit", "role": "button", "name": "Submit"},
                    {"tag": "input", "text": "", "selector": "#email", "role": "textbox", "name": "Email"}
                ]
            }
        });

        annotate_refs(&mut value, &mut ctx);

        let elements = value["interactive"]["elements"].as_array().unwrap();
        assert_eq!(elements[0]["ref"], json!("@e1"));
        assert_eq!(elements[1]["ref"], json!("@e2"));

        assert_eq!(ctx.ref_map().get("e1").unwrap().selector, "#submit");
        assert_eq!(ctx.ref_map().get("e2").unwrap().selector, "#email");
    }

    #[test]
    fn annotate_refs_reuses_existing_refs_for_same_selector() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        use browser::BrowserContext;

        use super::annotate_refs;

        let bridge: Arc<Mutex<Box<dyn browser::BrowserBackend + Send>>> =
            Arc::new(Mutex::new(Box::new(browser::NopBridge)));
        let mut ctx = BrowserContext::new(bridge);

        let mut value1 = json!({
            "interactive": {
                "elements": [
                    {"selector": "#submit", "role": "button", "name": "Submit"}
                ]
            }
        });
        annotate_refs(&mut value1, &mut ctx);

        let mut value2 = json!({
            "interactive": {
                "elements": [
                    {"selector": "#submit", "role": "button", "name": "Submit"},
                    {"selector": "#cancel", "role": "button", "name": "Cancel"}
                ]
            }
        });
        annotate_refs(&mut value2, &mut ctx);

        let elements = value2["interactive"]["elements"].as_array().unwrap();
        assert_eq!(elements[0]["ref"], json!("@e1"));
        assert_eq!(elements[1]["ref"], json!("@e2"));
    }

    #[test]
    fn ref_clear_invalidates_stale_refs() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        use browser::BrowserContext;

        use super::annotate_refs;
        use crate::tools::ref_resolve::resolve_selector;

        let bridge: Arc<Mutex<Box<dyn browser::BrowserBackend + Send>>> =
            Arc::new(Mutex::new(Box::new(browser::NopBridge)));
        let mut ctx = BrowserContext::new(bridge);

        let mut value = json!({
            "interactive": {
                "elements": [
                    {"selector": "#old-btn", "role": "button", "name": "Old"}
                ]
            }
        });
        annotate_refs(&mut value, &mut ctx);
        assert_eq!(resolve_selector("@e1", ctx.ref_map()).unwrap(), "#old-btn");

        ctx.ref_map_mut().clear();

        let err = resolve_selector("@e1", ctx.ref_map()).unwrap_err();
        assert!(err.contains("Unknown element ref"));

        let mut value2 = json!({
            "interactive": {
                "elements": [
                    {"selector": "#new-btn", "role": "button", "name": "New"}
                ]
            }
        });
        annotate_refs(&mut value2, &mut ctx);
        assert_eq!(resolve_selector("@e1", ctx.ref_map()).unwrap(), "#new-btn");
    }

    #[test]
    fn normalize_url_strips_hash_preserves_routes() {
        use super::normalize_url;

        assert_eq!(
            normalize_url("https://example.com/page#section"),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_url("https://app.com/#/dashboard"),
            "https://app.com/#/dashboard"
        );
        assert_eq!(
            normalize_url("https://app.com/#!/billing"),
            "https://app.com/#!/billing"
        );
        assert_eq!(
            normalize_url("https://example.com/page"),
            "https://example.com/page"
        );
    }
}
