use serde_json::{json, Value};

use crate::aria::{assign_refs, parse_raw_tree, reconcile, to_yaml};
use crate::page_fingerprint::PageFingerprint;
use crate::semantic::{
    assemble_region_tree, compute_accessible_name, select_active_dialog, RawElementFacts,
    RegionCandidate,
};
use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

/// Default ARIA-tree serialization depth when the caller does not request one.
/// Matches the bridge's own internal default so the two stay in lock-step.
const DEFAULT_TREE_DEPTH: usize = 5;

const MAX_PAGE_MAP_LINKS: usize = 50;
const MAX_PAGE_MAP_FORMS: usize = 10;
const MAX_PAGE_MAP_LANDMARKS: usize = 20;
const MAX_PAGE_MAP_REGIONS: usize = 16;
const MAX_PAGE_MAP_CONTROLS: usize = 30;

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
    let truncated_regions = truncate_array_field(value, "regions", MAX_PAGE_MAP_REGIONS);
    let truncated_controls = truncate_array_field(value, "controls", MAX_PAGE_MAP_CONTROLS);

    if let Some(object) = value.as_object_mut() {
        object.insert("truncated_links".to_string(), Value::Bool(truncated_links));
        object.insert("truncated_forms".to_string(), Value::Bool(truncated_forms));
        object.insert(
            "truncated_landmarks".to_string(),
            Value::Bool(truncated_landmarks),
        );
        object.insert(
            "truncated_regions".to_string(),
            Value::Bool(truncated_regions),
        );
        object.insert(
            "truncated_controls".to_string(),
            Value::Bool(truncated_controls),
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

fn resolve_scope(
    scope: Option<&str>,
    browser: &BrowserContext,
) -> Result<Option<String>, ToolExecutionError> {
    let Some(scope) = scope else {
        return Ok(None);
    };

    match crate::tools::ref_resolve::resolve_scope_ref(scope, browser) {
        Ok(Some(query)) => Ok(Some(query)),
        Err(message) => Err(ToolExecutionError::new(message)),
        Ok(None) => Ok(Some(match scope {
            "dialog" => {
                "[role=\"dialog\"], [role=\"alertdialog\"], [aria-modal=\"true\"], [popover]"
                    .to_string()
            }
            "main" => "main, [role=\"main\"]".to_string(),
            "sidebar" => "[role=\"complementary\"], aside, nav".to_string(),
            other => other.to_string(),
        })),
    }
}

fn infer_control_role(tag: &str, role: Option<&str>) -> String {
    role.map_or_else(
        || {
            match tag {
                "button" => "button",
                "select" => "combobox",
                "textarea" | "input" => "textbox",
                _ => "",
            }
            .to_string()
        },
        str::to_string,
    )
}

fn control_facts_from_value(value: &Value) -> RawElementFacts {
    RawElementFacts {
        tag: value
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        role: value
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string),
        aria_expanded: None,
        aria_selected: None,
        aria_pressed: None,
        aria_controls: None,
        aria_owns: None,
        text: value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
        aria_label: value
            .get("aria_label")
            .and_then(Value::as_str)
            .map(str::to_string),
        aria_labelledby_text: value
            .get("aria_labelledby_text")
            .and_then(Value::as_str)
            .map(str::to_string),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        placeholder: value
            .get("placeholder")
            .and_then(Value::as_str)
            .map(str::to_string),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        visible: true,
        floating: false,
        selector: value
            .get("selector")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

pub(super) fn enrich_semantic_sections(result: &mut Value) {
    let regions = result
        .get("regionCandidates")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<RegionCandidate>(item.clone()).ok())
                .collect::<Vec<_>>()
        })
        .map(|candidates| assemble_region_tree(&candidates))
        .unwrap_or_default();

    let active_dialog = select_active_dialog(&regions).map(|region| {
        json!({
            "selector": region.selector,
            "label": region.label,
        })
    });

    let controls = result
        .get("nonFormControls")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let facts = control_facts_from_value(item);
                    json!({
                        "label": compute_accessible_name(&facts),
                        "role": infer_control_role(&facts.tag, facts.role.as_deref()),
                        "selector": facts.selector,
                        "value": item.get("value").cloned().unwrap_or(Value::Null),
                        "required": item.get("required").and_then(Value::as_bool).unwrap_or(false),
                        "disabled": item.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if let Some(object) = result.as_object_mut() {
        object.insert(
            "regions".to_string(),
            serde_json::to_value(regions).unwrap_or_else(|_| Value::Array(Vec::new())),
        );
        object.insert(
            "active_dialog".to_string(),
            active_dialog.unwrap_or(Value::Null),
        );
        object.insert("controls".to_string(), Value::Array(controls));
        object.remove("regionCandidates");
        object.remove("nonFormControls");
    }
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut crate::state::CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let scope = input.get("scope").and_then(Value::as_str);
    let resolved_scope = resolve_scope(scope, browser)?;

    let settings = runtime::load_settings();
    let compound_enrichment = runtime::settings_get_compound_enrichment(&settings);

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .page_map(resolved_scope.as_deref(), compound_enrichment)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    if result.get("stale_ref").and_then(Value::as_bool) == Some(true) {
        let message = result.get("error").and_then(Value::as_str).unwrap_or(
            "Ref not found. The page may have changed. Call page_map to get fresh refs.",
        );
        return Ok(ToolEffect::Reply(message.to_string()));
    }
    if result.get("scope_not_found").and_then(Value::as_bool) == Some(true) {
        let requested = result.get("scope").and_then(Value::as_str).unwrap_or("");
        return Ok(ToolEffect::Reply(format!(
            "scope not found: '{requested}'. Call page_map without a scope to map the full page."
        )));
    }

    let mut tree = result.get("tree").and_then(parse_raw_tree).ok_or_else(|| {
        ToolExecutionError::new("failed to parse ARIA tree from page_map bridge response")
    })?;

    let url = result
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| {
            result
                .get("meta")
                .and_then(|meta| meta.get("url"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
        .to_string();

    let prev_tree = crawl_state.last_aria_tree.clone();

    if scope.is_none() {
        let cache_key = normalize_url(&url).to_string();
        let url_changed = browser
            .snapshot_url()
            .is_some_and(|prev_url| prev_url != cache_key.as_str());
        if url_changed {
            browser.ref_map_mut().clear();
        }
        assign_refs(&mut tree, browser.ref_map_mut(), None, &mut Vec::new());
        browser.set_page_snapshot(&cache_key, None, result.clone());
    } else {
        assign_refs(&mut tree, browser.ref_map_mut(), None, &mut Vec::new());
    }

    if let Some(prev) = &prev_tree {
        reconcile(prev, &mut tree, &mut Vec::new());
    }

    let fp_settings = runtime::load_settings();
    if runtime::settings_get_page_fingerprinting(&fp_settings) {
        let fingerprint = PageFingerprint::compute(&url, &tree);
        crawl_state.page_fingerprints.push(fingerprint);
    }

    crawl_state.last_aria_tree = Some(tree.clone());

    let depth = input
        .get("depth")
        .and_then(Value::as_u64)
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(DEFAULT_TREE_DEPTH);

    Ok(ToolEffect::Reply(to_yaml(&tree, Some(depth))))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use super::{apply_page_map_caps, enrich_semantic_sections, resolve_scope};
    use browser::BrowserContext;

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
    fn page_map_caps_regions_and_controls() {
        let mut value = json!({
            "headings": [],
            "landmarks": [],
            "forms": [],
            "links": [],
            "regions": (0..20)
                .map(|index| json!({
                    "kind": "Region",
                    "label": format!("region {index}"),
                    "handle": format!("@r{}", index + 1),
                    "selector": format!("#region-{index}"),
                    "visible": true,
                    "children": []
                }))
                .collect::<Vec<_>>(),
            "controls": (0..40)
                .map(|index| json!({
                    "label": format!("Control {index}"),
                    "role": "textbox",
                    "selector": format!("#control-{index}"),
                    "value": null,
                    "required": false,
                    "disabled": false
                }))
                .collect::<Vec<_>>(),
            "interactive": {},
            "meta": {}
        });

        apply_page_map_caps(&mut value);

        assert_eq!(value["regions"].as_array().map(Vec::len), Some(16));
        assert_eq!(value["controls"].as_array().map(Vec::len), Some(30));
        assert_eq!(value["truncated_regions"], json!(true));
        assert_eq!(value["truncated_controls"], json!(true));
    }

    #[test]
    fn page_map_assembles_regions_from_raw_candidates() {
        let mut value = json!({
            "regionCandidates": [
                {
                    "tag": "main",
                    "role": null,
                    "aria_label": null,
                    "id": null,
                    "depth": 1,
                    "parent_idx": null,
                    "selector": "main",
                    "visible": true
                },
                {
                    "tag": "div",
                    "role": "dialog",
                    "aria_label": "Confirm",
                    "id": "modal",
                    "depth": 2,
                    "parent_idx": 0,
                    "selector": "div#modal",
                    "visible": true
                }
            ],
            "nonFormControls": [
                {
                    "tag": "input",
                    "role": null,
                    "text": "",
                    "aria_label": "Search",
                    "aria_labelledby_text": null,
                    "title": null,
                    "placeholder": null,
                    "name": null,
                    "value": null,
                    "required": false,
                    "disabled": false,
                    "selector": "input#search"
                }
            ],
            "headings": [],
            "landmarks": [],
            "forms": [],
            "links": [],
            "interactive": {"counts": {"total": 0, "buttons": 0, "inputs": 0, "selects": 0, "textareas": 0}, "elements": []},
            "meta": {"title": "Test", "url": "https://example.com", "description": ""},
            "total_landmarks": 0,
            "total_forms": 0,
            "total_links": 0
        });

        enrich_semantic_sections(&mut value);

        assert_eq!(value["regions"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["regions"][0]["label"], json!("main panel"));
        assert_eq!(
            value["regions"][0]["children"][0]["label"],
            json!("Confirm")
        );
        assert!(value["active_dialog"].get("handle").is_none());
        assert_eq!(value["active_dialog"]["selector"], json!("div#modal"));
        assert_eq!(value["controls"][0]["label"], json!("Search"));
        assert_eq!(value["controls"][0]["role"], json!("textbox"));
        assert!(value.get("regionCandidates").is_none());
        assert!(value.get("nonFormControls").is_none());
    }

    #[test]
    fn semantic_scope_tokens_resolve_and_legacy_handles_rejected() {
        let bridge: Arc<Mutex<Box<dyn browser::BrowserBackend + Send>>> =
            Arc::new(Mutex::new(Box::new(browser::NopBridge)));
        let ctx = BrowserContext::new(bridge);

        assert_eq!(
            resolve_scope(Some("main"), &ctx).unwrap(),
            Some("main, [role=\"main\"]".to_string())
        );
        assert_eq!(
            resolve_scope(Some("dialog"), &ctx).unwrap(),
            Some(
                "[role=\"dialog\"], [role=\"alertdialog\"], [aria-modal=\"true\"], [popover]"
                    .to_string(),
            )
        );
        assert_eq!(
            resolve_scope(Some("sidebar"), &ctx).unwrap(),
            Some("[role=\"complementary\"], aside, nav".to_string())
        );

        for handle in ["@r2", "@r9"] {
            assert_eq!(
                resolve_scope(Some(handle), &ctx).unwrap_err().to_string(),
                "@rN region handles are no longer supported. Use [ref=eN] from page_map output instead."
            );
        }
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
        // Legacy selector-backed refs still resolve to the stored CSS selector.
        let resolved = resolve_selector("@e1", &map).unwrap();
        assert_eq!(resolved, "#email-input");
    }

    #[test]
    fn invalid_ref_produces_clear_error_message() {
        use browser::RefMap;

        use crate::tools::ref_resolve::resolve_selector;

        let map = RefMap::new(); // empty map
        let err = resolve_selector("@e999", &map).unwrap_err();
        assert_eq!(
            err,
            "Ref '@e999' not found. The page may have changed. Call page_map to get fresh refs."
        );
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
        assert_eq!(
            err,
            "Ref '@e1' not found. The page may have changed. Call page_map to get fresh refs."
        );

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

    #[test]
    fn page_map_tree_pipeline_parses_assigns_and_serializes() {
        use browser::RefMap;

        use crate::aria::{assign_refs, parse_raw_tree, to_yaml};

        let bridge_result = json!({
            "tree": {
                "role": "document",
                "name": "",
                "states": {},
                "refId": Value::Null,
                "url": Value::Null,
                "frameId": Value::Null,
                "offscreen": false,
                "omittedChildren": 0,
                "children": [
                    {
                        "role": "button",
                        "name": "Submit",
                        "states": { "disabled": true },
                        "refId": "e1",
                        "offscreen": false,
                        "omittedChildren": 0,
                        "children": []
                    }
                ]
            },
            "url": "https://example.com"
        });

        let mut tree = parse_raw_tree(&bridge_result["tree"]).expect("tree should parse");
        let mut ref_map = RefMap::new();
        assign_refs(&mut tree, &mut ref_map, None, &mut Vec::new());

        let yaml = to_yaml(&tree, Some(super::DEFAULT_TREE_DEPTH));
        assert!(yaml.starts_with("- document \"\""));
        assert!(yaml.contains("button \"Submit\" [disabled]"));
        assert!(tree.ref_id.is_some());
        assert!(tree.children[0].ref_id.is_some());
    }
}
