//! Parse the raw JS-emitted ARIA tree JSON (camelCase keys) into [`AriaNode`].
//!
//! The browser bridge's `page_map` handler (T8) emits a tree whose node shape is
//! `{ role, name, states, refId, url, frameId, offscreen, children, omittedChildren, crossOrigin? }`.
//! This module is the single seam that converts that wire JSON into the typed
//! [`AriaNode`] the reconcile/serialize pipeline consumes. The `crossOrigin`
//! flag is informational on the JS side and has no corresponding field on
//! [`AriaNode`], so it is intentionally dropped here.

use serde_json::Value;

use crate::aria::node::{AriaNode, AriaStates};

/// Parse a single raw JS tree node (and its descendants) into an [`AriaNode`].
///
/// Returns `None` when the node has no string `role` (the one required field).
/// Every other field defaults sensibly when absent. `children` is parsed
/// recursively; any child that fails to parse is silently skipped so a single
/// malformed node never discards an otherwise-valid subtree.
#[must_use]
pub fn parse_raw_tree(value: &Value) -> Option<AriaNode> {
    let role = value.get("role").and_then(Value::as_str)?.to_string();
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);

    let states = parse_states(value.get("states"));

    let ref_id = value
        .get("refId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let url = value.get("url").and_then(Value::as_str).map(str::to_string);
    let frame_id = value
        .get("frameId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let offscreen = value
        .get("offscreen")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let omitted_children = value
        .get("omittedChildren")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0);

    let children = value
        .get("children")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_raw_tree).collect())
        .unwrap_or_default();

    Some(AriaNode {
        role,
        name,
        states,
        ref_id,
        url,
        frame_id,
        offscreen,
        children,
        omitted_children,
    })
}

/// Map the JS `states` object onto the typed [`AriaStates`].
///
/// The JS handler only emits a key when the state is present, so absent keys
/// collapse to `false` (tristate flags collapse to `None`). `expanded` and
/// `pressed` are tristate: `None` means the attribute was absent, `Some(false)`
/// means it was explicitly `false`.
fn parse_states(states: Option<&Value>) -> AriaStates {
    let bool_at = |key: &str| {
        states
            .and_then(|s| s.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let opt_bool_at = |key: &str| states.and_then(|s| s.get(key)).and_then(Value::as_bool);

    AriaStates {
        disabled: bool_at("disabled"),
        checked: bool_at("checked"),
        expanded: opt_bool_at("expanded"),
        pressed: opt_bool_at("pressed"),
        selected: bool_at("selected"),
        level: states
            .and_then(|s| s.get("level"))
            .and_then(Value::as_u64)
            .and_then(|level| u8::try_from(level).ok()),
        active: bool_at("active"),
        invalid: bool_at("invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_full_node_with_camelcase_keys() {
        let value = json!({
            "role": "link",
            "name": "Docs",
            "states": {},
            "refId": "e7",
            "url": "https://example.com/docs",
            "frameId": "f2",
            "offscreen": true,
            "children": [],
            "omittedChildren": 3
        });

        let node = parse_raw_tree(&value).expect("node should parse");
        assert_eq!(node.role, "link");
        assert_eq!(node.name.as_deref(), Some("Docs"));
        assert_eq!(node.ref_id.as_deref(), Some("e7"));
        assert_eq!(node.url.as_deref(), Some("https://example.com/docs"));
        assert_eq!(node.frame_id.as_deref(), Some("f2"));
        assert!(node.offscreen);
        assert_eq!(node.omitted_children, 3);
        assert!(node.children.is_empty());
    }

    #[test]
    fn missing_role_returns_none() {
        let value = json!({ "name": "no role" });
        assert!(parse_raw_tree(&value).is_none());
    }

    #[test]
    fn defaults_apply_when_optional_fields_absent() {
        let value = json!({ "role": "main" });
        let node = parse_raw_tree(&value).expect("node should parse");
        assert_eq!(node.role, "main");
        assert_eq!(node.name, None);
        assert_eq!(node.ref_id, None);
        assert_eq!(node.url, None);
        assert_eq!(node.frame_id, None);
        assert!(!node.offscreen);
        assert_eq!(node.omitted_children, 0);
        assert!(node.children.is_empty());
        assert_eq!(node.states, AriaStates::default());
    }

    #[test]
    fn empty_name_is_preserved_as_some_empty_string() {
        let value = json!({ "role": "document", "name": "" });
        let node = parse_raw_tree(&value).expect("node should parse");
        assert_eq!(node.name.as_deref(), Some(""));
    }

    #[test]
    fn tristate_states_distinguish_absent_from_false() {
        let absent =
            parse_raw_tree(&json!({ "role": "button", "states": {} })).expect("node should parse");
        assert_eq!(absent.states.expanded, None);
        assert_eq!(absent.states.pressed, None);

        let explicit = parse_raw_tree(&json!({
            "role": "button",
            "states": { "expanded": false, "pressed": false }
        }))
        .expect("node should parse");
        assert_eq!(explicit.states.expanded, Some(false));
        assert_eq!(explicit.states.pressed, Some(false));
    }

    #[test]
    fn parses_all_boolean_and_level_states() {
        let value = json!({
            "role": "tab",
            "states": {
                "disabled": true,
                "checked": true,
                "expanded": true,
                "pressed": true,
                "selected": true,
                "level": 3,
                "active": true,
                "invalid": true
            }
        });

        let node = parse_raw_tree(&value).expect("node should parse");
        assert!(node.states.disabled);
        assert!(node.states.checked);
        assert_eq!(node.states.expanded, Some(true));
        assert_eq!(node.states.pressed, Some(true));
        assert!(node.states.selected);
        assert_eq!(node.states.level, Some(3));
        assert!(node.states.active);
        assert!(node.states.invalid);
    }

    #[test]
    fn parses_nested_children_recursively() {
        let value = json!({
            "role": "main",
            "name": "",
            "children": [
                {
                    "role": "navigation",
                    "name": "Primary",
                    "children": [
                        { "role": "link", "name": "Home", "refId": "e1", "children": [] }
                    ]
                },
                { "role": "button", "name": "Submit", "refId": "e2", "children": [] }
            ]
        });

        let node = parse_raw_tree(&value).expect("node should parse");
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.children[0].role, "navigation");
        assert_eq!(node.children[0].children.len(), 1);
        assert_eq!(node.children[0].children[0].name.as_deref(), Some("Home"));
        assert_eq!(node.children[1].ref_id.as_deref(), Some("e2"));
    }

    #[test]
    fn malformed_child_is_skipped_without_dropping_siblings() {
        let value = json!({
            "role": "main",
            "children": [
                { "name": "no role here" },
                { "role": "button", "name": "Keep me", "children": [] }
            ]
        });

        let node = parse_raw_tree(&value).expect("node should parse");
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].name.as_deref(), Some("Keep me"));
    }

    #[test]
    fn null_ref_id_maps_to_none() {
        let value = json!({ "role": "document", "refId": Value::Null });
        let node = parse_raw_tree(&value).expect("node should parse");
        assert_eq!(node.ref_id, None);
    }
}
