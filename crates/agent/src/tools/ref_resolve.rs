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

/// Validate that a ref is actionable or try to auto-descend into children.
///
/// Returns:
/// - `Ok(None)`: ref is directly actionable — use it as-is.
/// - `Ok(Some(child_ref_id))`: container with exactly one actionable
///   descendant — auto-descend to this child ref.
/// - `Err(msg)`: container with 0 or 2+ actionable children, or stale ref.
fn validate_or_auto_descend(
    input: &str,
    ref_map: &RefMap,
    ref_id: &str,
    skip_auto_descend: bool,
) -> Result<Option<String>, String> {
    let entry = ref_map.get(ref_id).ok_or_else(|| stale_ref_error(input))?;

    // Directly actionable ref — no auto-descend needed
    if !matches!(entry.resolution, Resolution::Attr(_)) || is_actionable_role(&entry.role) {
        return Ok(None);
    }

    // Container ref — skip auto-descend when caller explicitly opts out
    // (e.g. fill_form's form_selector must stay on the <form> element)
    if skip_auto_descend {
        return Err(container_ref_error(input));
    }

    // Truncated container — children may have been omitted by depth/child-count
    // limits in the ARIA walk; the single visible child may not be the only one
    if entry.truncated {
        return Err(container_ref_error(input));
    }

    // Container ref — try to find actionable descendants
    let descendants = ref_map.descendant_refs(ref_id);
    let actionable: Vec<String> = descendants
        .into_iter()
        .filter(|d_id| {
            ref_map
                .get(d_id)
                .is_some_and(|e| is_actionable_role(&e.role))
        })
        .collect();

    match actionable.len() {
        0 => Err(container_ref_error(input)),
        1 => {
            let child_ref = &actionable[0];
            Ok(Some(format!("@{child_ref}")))
        }
        _ => {
            let child_list = actionable
                .iter()
                .map(|id| format!("@{id}"))
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "Ref '{}' is a container with {} clickable children ({}). Target one of them directly.",
                canonical_ref(input),
                actionable.len(),
                child_list
            ))
        }
    }
}

/// Resolve a ref string (e.g. "@e5" or "e5") to its action query.
///
/// Returns `(None, selector)` for raw CSS inputs and `(None, dom_query)` for refs.
/// The frame slot is retained in the return type for compatibility; `RefMap`
/// intentionally does not persist the walker's unstable `f1`/`f2` frame labels.
pub fn resolve_to_action_query(
    ref_input: &str,
    context: &BrowserContext,
) -> Result<(Option<String>, String), String> {
    if let Some(ref_id) = parse_ref(ref_input) {
        let ref_map = context.ref_map();
        let resolved_ref = validate_or_auto_descend(ref_input, ref_map, &ref_id, false)?;

        let target_id = match resolved_ref {
            Some(child_ref) => {
                // parse_ref strips the @ before the ref id
                parse_ref(&child_ref).unwrap_or(child_ref)
            }
            None => ref_id,
        };

        return ref_map
            .resolve(&target_id)
            .map(|(_, query)| (None, query))
            .ok_or_else(|| stale_ref_error(ref_input));
    }

    Ok((None, ref_input.to_string()))
}

/// Resolve an @eN ref or bare eN ref to its DOM action query.
/// If input is a CSS selector (not a ref), returns it unchanged.
/// Returns Err if input looks like a ref but is not found in the map.
///
/// When `skip_auto_descend` is true, container refs always produce an error
/// instead of trying to auto-descend to a child. Use this for selectors that
/// must stay on the container element (e.g. `fill_form`'s `form_selector`,
/// which needs the actual `<form>` element for submission).
pub fn resolve_selector(
    input: &str,
    ref_map: &RefMap,
    skip_auto_descend: bool,
) -> Result<String, String> {
    if let Some(ref_id) = parse_ref(input) {
        let resolved_ref = validate_or_auto_descend(input, ref_map, &ref_id, skip_auto_descend)?;

        let target_id = match resolved_ref {
            Some(child_ref) => parse_ref(&child_ref).unwrap_or(child_ref),
            None => ref_id,
        };

        ref_map
            .resolve(&target_id)
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

/// Resolve a `page_map` scope token without converting refs into DOM selectors.
///
/// The in-page ARIA walker understands `[ref=eN]` and searches same-origin
/// descendant frames. If Rust pre-resolves the ref into `[data-acrawl-ref=...]`,
/// the browser receives a generic CSS selector and can miss iframe-contained
/// nodes. This function only validates that a ref is known, then returns a
/// canonical `[ref=eN]` token for the walker.
pub fn resolve_page_map_scope_ref(
    input: &str,
    context: &BrowserContext,
) -> Result<Option<String>, String> {
    let trimmed = input.trim();

    if is_legacy_region_handle(trimmed) {
        return Err(STALE_REGION_HANDLE_MESSAGE.to_string());
    }

    if let Some(ref_id) = parse_scope_ref(trimmed) {
        if context.ref_map().get(&ref_id).is_some() {
            return Ok(Some(format!("[ref={ref_id}]")));
        }
        return Err(format!("Ref '@{ref_id}'{STALE_REF_SUFFIX}"));
    }

    Ok(None)
}

/// CSS selector fallback for a well-known semantic region token (`"dialog"`,
/// `"main"`, `"sidebar"`) when it doesn't resolve to a ref via
/// [`resolve_scope_ref`]/[`resolve_page_map_scope_ref`].
///
/// Shared by `click`'s `region` param and `page_map`'s `scope` param so the
/// two tools agree on what e.g. `region="dialog"` means — previously each
/// hardcoded its own copy of these selectors and they drifted apart.
/// Any other token (a raw CSS selector) is returned unchanged.
#[must_use]
pub fn region_scope_selector(token: &str) -> String {
    match token {
        "dialog" => {
            "dialog, [role=\"dialog\"], [role=\"alertdialog\"], [aria-modal=\"true\"], [popover]:popover-open"
                .to_string()
        }
        "main" => "main, [role=\"main\"]".to_string(),
        "sidebar" => "[role=\"complementary\"], aside".to_string(),
        other => other.to_string(),
    }
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
        let result = resolve_selector("#my-button", &map, false).unwrap();
        assert_eq!(result, "#my-button");
    }

    #[test]
    fn region_scope_selector_dialog_includes_popover() {
        // click(region="dialog") and page_map(scope="dialog") must agree on
        // what counts as a dialog, including `[popover]`-based ones.
        let selector = region_scope_selector("dialog");
        assert!(
            selector.contains("[popover]:popover-open"),
            "selector: {selector}"
        );
        assert!(
            selector.contains("[role=\"dialog\"]"),
            "selector: {selector}"
        );
    }

    #[test]
    fn region_scope_selector_sidebar_excludes_nav() {
        let selector = region_scope_selector("sidebar");
        assert!(!selector.contains("nav"), "selector: {selector}");
        assert!(
            selector.contains("[role=\"complementary\"]"),
            "selector: {selector}"
        );
    }

    #[test]
    fn region_scope_selector_passes_through_unknown_token() {
        assert_eq!(region_scope_selector("#custom-css"), "#custom-css");
    }

    #[test]
    fn valid_ref_resolves_to_selector() {
        let mut map = RefMap::new();
        map.assign_or_reuse("button.submit", "button", "Submit", None);
        let result = resolve_selector("@e1", &map, false).unwrap();
        assert_eq!(result, "button.submit");
    }

    #[test]
    fn bare_ref_resolves() {
        let mut map = RefMap::new();
        map.assign_or_reuse("input#email", "textbox", "Email", None);
        let result = resolve_selector("e1", &map, false).unwrap();
        assert_eq!(result, "input#email");
    }

    #[test]
    fn unknown_ref_returns_error() {
        let map = RefMap::new();
        let err = resolve_selector("@e999", &map, false).unwrap_err();
        assert_eq!(
            err,
            "Ref '@e999' not found. The page may have changed. Call page_map to get fresh refs."
        );
    }

    #[test]
    fn dot_selector_passes_through() {
        let map = RefMap::new();
        let result = resolve_selector(".btn-primary", &map, false).unwrap();
        assert_eq!(result, ".btn-primary");
    }

    #[test]
    fn empty_string_passes_through() {
        let map = RefMap::new();
        let result = resolve_selector("", &map, false).unwrap();
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
            None,
        );

        let result = resolve_selector(&format!("@{ref_id}"), &map, false).unwrap();
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
            None,
        );

        let err = resolve_selector(&format!("@{ref_id}"), &map, false).unwrap_err();
        assert_eq!(
            err,
            format!(
                "Ref '@{ref_id}' is a container node. Target a specific child element within it."
            )
        );
    }

    #[test]
    fn container_ref_auto_descends_to_single_actionable_child() {
        let mut map = RefMap::new();

        // Container (generic div wrapping a button)
        let container_id = map.assign_by_identity(
            "generic|Diagramma|",
            "generic",
            "Diagramma",
            None,
            Resolution::Attr(String::new()),
            None,
        );

        // Single actionable child (button)
        map.assign_by_identity(
            "button|Settings|generic:Diagramma|",
            "button",
            "Settings",
            None,
            Resolution::Attr(String::new()),
            Some(&container_id),
        );

        // Resolving the container should auto-descend to the button
        let result = resolve_selector(&format!("@{container_id}"), &map, false).unwrap();
        // Should resolve to the button's attr query
        assert!(result.contains("data-acrawl-ref='e2'"));
    }

    #[test]
    fn container_ref_with_multiple_actionable_children_returns_hint() {
        let mut map = RefMap::new();

        let container_id = map.assign_by_identity(
            "generic|Root|",
            "generic",
            "Root",
            None,
            Resolution::Attr(String::new()),
            None,
        );

        let child1 = map.assign_by_identity(
            "button|A|generic:Root|",
            "button",
            "A",
            None,
            Resolution::Attr(String::new()),
            Some(&container_id),
        );

        let child2 = map.assign_by_identity(
            "button|B|generic:Root|",
            "button",
            "B",
            None,
            Resolution::Attr(String::new()),
            Some(&container_id),
        );

        let err = resolve_selector(&format!("@{container_id}"), &map, false).unwrap_err();
        assert!(
            err.contains(&format!("@{child1}")),
            "error should mention child1: {err}"
        );
        assert!(
            err.contains(&format!("@{child2}")),
            "error should mention child2: {err}"
        );
        assert!(
            err.contains("2 clickable children"),
            "error should state count: {err}"
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
            None,
        );

        let (frame_id, query) = resolve_to_action_query(&format!("@{ref_id}"), &context).unwrap();
        assert_eq!(frame_id, None);
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
            None,
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
    fn page_map_scope_ref_keeps_ref_token_for_browser_frame_lookup() {
        let mut context = test_context();
        let ref_id = context.ref_map_mut().assign_by_identity(
            "dialog|Confirm|",
            "dialog",
            "Confirm",
            Some("f1"),
            Resolution::Attr(String::new()),
            None,
        );

        let expected = Some(format!("[ref={ref_id}]"));
        assert_eq!(
            resolve_page_map_scope_ref(&format!("[ref={ref_id}]"), &context).unwrap(),
            expected
        );
        assert_eq!(
            resolve_page_map_scope_ref(&format!("@{ref_id}"), &context).unwrap(),
            expected
        );
        assert_eq!(
            resolve_page_map_scope_ref(&ref_id, &context).unwrap(),
            expected
        );
    }

    #[test]
    fn page_map_scope_ref_unknown_element_ref_returns_stale_error() {
        let context = test_context();
        let err = resolve_page_map_scope_ref("[ref=e999]", &context).unwrap_err();
        assert_eq!(
            err,
            "Ref '@e999' not found. The page may have changed. Call page_map to get fresh refs."
        );
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
