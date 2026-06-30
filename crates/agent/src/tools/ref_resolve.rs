use browser::{parse_ref, ref_map::Resolution, RefMap};

use crate::BrowserContext;

const STALE_REF_SUFFIX: &str =
    " not found. The page may have changed. Call page_map to get fresh refs.";
const CONTAINER_REF_SUFFIX: &str =
    " is a container node. Target a specific child element within it.";

fn canonical_ref(input: &str) -> String {
    parse_ref(input).map_or_else(|| input.to_string(), |ref_id| format!("@{ref_id}"))
}

fn stale_ref_error(input: &str) -> String {
    format!("Ref '{}'{}", canonical_ref(input), STALE_REF_SUFFIX)
}

fn container_ref_error(input: &str) -> String {
    format!("Ref '{}'{}", canonical_ref(input), CONTAINER_REF_SUFFIX)
}

fn is_actionable_role(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "textbox"
            | "checkbox"
            | "radio"
            | "combobox"
            | "slider"
            | "switch"
            | "tab"
            | "menuitem"
            | "option"
            | "treeitem"
    )
}

fn validate_actionable_ref(input: &str, ref_map: &RefMap, ref_id: &str) -> Result<(), String> {
    let entry = ref_map.get(ref_id).ok_or_else(|| stale_ref_error(input))?;
    if matches!(entry.resolution, Resolution::Attr(_)) && !is_actionable_role(&entry.role) {
        return Err(container_ref_error(input));
    }
    Ok(())
}

/// Resolve a ref string (e.g. "@e5" or "e5") to its owning frame and action query.
///
/// Returns `(None, selector)` for raw CSS inputs and `(frame_id, dom_query)` for refs.
///
/// `frame_id` is preserved for bridge-aware callers, but the current Playwright
/// bridge also falls back to searching descendant frames when given only the
/// DOM query, so existing callers may ignore it without losing functionality.
pub fn resolve_to_action_query(
    ref_input: &str,
    context: &BrowserContext,
) -> Result<(Option<String>, String), String> {
    if let Some(ref_id) = parse_ref(ref_input) {
        let ref_map = context.ref_map();
        validate_actionable_ref(ref_input, ref_map, &ref_id)?;
        return ref_map
            .resolve(&ref_id)
            .ok_or_else(|| stale_ref_error(ref_input));
    }

    Ok((None, ref_input.to_string()))
}

/// Resolve an @eN ref or bare eN ref to its DOM action query.
/// If input is a CSS selector (not a ref), returns it unchanged.
/// Returns Err if input looks like a ref but is not found in the map.
pub fn resolve_selector(input: &str, ref_map: &RefMap) -> Result<String, String> {
    if let Some(ref_id) = parse_ref(input) {
        validate_actionable_ref(input, ref_map, &ref_id)?;
        ref_map
            .resolve(&ref_id)
            .map(|(_, query)| query)
            .ok_or_else(|| stale_ref_error(input))
    } else {
        Ok(input.to_string())
    }
}

/// Migration error for callers still passing a legacy `@rN` region handle to a
/// `scope`/`region` parameter (the `@rN` namespace was retired in favor of `[ref=eN]`).
pub const STALE_REGION_HANDLE_MESSAGE: &str =
    "@rN region handles are no longer supported. Use [ref=eN] from page_map output instead.";

fn is_legacy_region_handle(input: &str) -> bool {
    input
        .strip_prefix("@r")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

fn parse_scope_ref(input: &str) -> Option<String> {
    let inner = input
        .strip_prefix("[ref=")
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(input);
    parse_ref(inner)
}

/// Resolve a `scope`/`region` token that may reference a specific element.
///
/// - `@rN` legacy region handle → `Err(STALE_REGION_HANDLE_MESSAGE)`.
/// - `[ref=eN]` / `@eN` / `eN` → `Ok(Some(query))` via `RefMap::resolve` (a
///   stale ref yields `Err`); container nodes are allowed, unlike `resolve_selector`.
/// - anything else (semantic token or raw CSS) → `Ok(None)` for the caller to handle.
pub fn resolve_scope_ref(input: &str, context: &BrowserContext) -> Result<Option<String>, String> {
    let trimmed = input.trim();

    if is_legacy_region_handle(trimmed) {
        return Err(STALE_REGION_HANDLE_MESSAGE.to_string());
    }

    if let Some(ref_id) = parse_scope_ref(trimmed) {
        return context
            .ref_map()
            .resolve(&ref_id)
            .map(|(_frame_id, query)| Some(query))
            .ok_or_else(|| format!("Ref '@{ref_id}'{STALE_REF_SUFFIX}"));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use browser::RefMap;
    use tokio::sync::Mutex;

    use super::*;

    fn test_context() -> crate::BrowserContext {
        let bridge: Arc<Mutex<Box<dyn browser::BrowserBackend + Send>>> =
            Arc::new(Mutex::new(Box::new(browser::NopBridge)));
        crate::BrowserContext::new(bridge)
    }

    #[test]
    fn css_selector_passes_through() {
        let map = RefMap::new();
        let result = resolve_selector("#my-button", &map).unwrap();
        assert_eq!(result, "#my-button");
    }

    #[test]
    fn valid_ref_resolves_to_selector() {
        let mut map = RefMap::new();
        map.assign_or_reuse("button.submit", "button", "Submit");
        let result = resolve_selector("@e1", &map).unwrap();
        assert_eq!(result, "button.submit");
    }

    #[test]
    fn bare_ref_resolves() {
        let mut map = RefMap::new();
        map.assign_or_reuse("input#email", "textbox", "Email");
        let result = resolve_selector("e1", &map).unwrap();
        assert_eq!(result, "input#email");
    }

    #[test]
    fn unknown_ref_returns_error() {
        let map = RefMap::new();
        let err = resolve_selector("@e999", &map).unwrap_err();
        assert_eq!(
            err,
            "Ref '@e999' not found. The page may have changed. Call page_map to get fresh refs."
        );
    }

    #[test]
    fn dot_selector_passes_through() {
        let map = RefMap::new();
        let result = resolve_selector(".btn-primary", &map).unwrap();
        assert_eq!(result, ".btn-primary");
    }

    #[test]
    fn empty_string_passes_through() {
        let map = RefMap::new();
        let result = resolve_selector("", &map).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn stamped_ref_resolves_to_attr_query() {
        let mut map = RefMap::new();
        let ref_id = map.assign_by_identity(
            "button|Submit|",
            "button",
            "Submit",
            Some("f1"),
            Resolution::Attr(String::new()),
        );

        let result = resolve_selector(&format!("@{ref_id}"), &map).unwrap();
        assert_eq!(result, format!("[data-acrawl-ref='{ref_id}']"));
    }

    #[test]
    fn container_ref_returns_guidance_error() {
        let mut map = RefMap::new();
        let ref_id = map.assign_by_identity(
            "navigation|Primary|",
            "navigation",
            "Primary",
            None,
            Resolution::Attr(String::new()),
        );

        let err = resolve_selector(&format!("@{ref_id}"), &map).unwrap_err();
        assert_eq!(
            err,
            format!(
                "Ref '@{ref_id}' is a container node. Target a specific child element within it."
            )
        );
    }

    #[test]
    fn action_query_returns_frame_and_query() {
        let mut context = test_context();
        let ref_id = context.ref_map_mut().assign_by_identity(
            "button|Submit|",
            "button",
            "Submit",
            Some("f7"),
            Resolution::Attr(String::new()),
        );

        let (frame_id, query) = resolve_to_action_query(&format!("@{ref_id}"), &context).unwrap();
        assert_eq!(frame_id, Some("f7".to_string()));
        assert_eq!(query, format!("[data-acrawl-ref='{ref_id}']"));
    }

    #[test]
    fn action_query_unknown_ref_returns_exact_stale_string() {
        let context = test_context();
        let err = resolve_to_action_query("@e999", &context).unwrap_err();
        assert_eq!(
            err,
            "Ref '@e999' not found. The page may have changed. Call page_map to get fresh refs."
        );
    }

    #[test]
    fn action_query_raw_css_selector_passes_through() {
        let context = test_context();
        let (frame_id, query) = resolve_to_action_query("button.submit", &context).unwrap();
        assert_eq!(frame_id, None);
        assert_eq!(query, "button.submit");
    }

    #[test]
    fn scope_ref_legacy_region_handle_returns_migration_error() {
        let context = test_context();
        let err = resolve_scope_ref("@r1", &context).unwrap_err();
        assert_eq!(err, STALE_REGION_HANDLE_MESSAGE);
        assert_eq!(
            err,
            "@rN region handles are no longer supported. Use [ref=eN] from page_map output instead."
        );
    }

    #[test]
    fn scope_ref_semantic_token_and_css_pass_through_as_none() {
        let context = test_context();
        assert_eq!(resolve_scope_ref("main", &context).unwrap(), None);
        assert_eq!(resolve_scope_ref("#custom-css", &context).unwrap(), None);
    }

    #[test]
    fn scope_ref_element_ref_resolves_to_query() {
        let mut context = test_context();
        let ref_id = context.ref_map_mut().assign_by_identity(
            "dialog|Confirm|",
            "dialog",
            "Confirm",
            Some("f1"),
            Resolution::Attr(String::new()),
        );

        let expected = Some(format!("[data-acrawl-ref='{ref_id}']"));
        assert_eq!(
            resolve_scope_ref(&format!("[ref={ref_id}]"), &context).unwrap(),
            expected
        );
        assert_eq!(
            resolve_scope_ref(&format!("@{ref_id}"), &context).unwrap(),
            expected
        );
        assert_eq!(resolve_scope_ref(&ref_id, &context).unwrap(), expected);
    }

    #[test]
    fn scope_ref_unknown_element_ref_returns_stale_error() {
        let context = test_context();
        let err = resolve_scope_ref("@e999", &context).unwrap_err();
        assert_eq!(
            err,
            "Ref '@e999' not found. The page may have changed. Call page_map to get fresh refs."
        );
    }
}
