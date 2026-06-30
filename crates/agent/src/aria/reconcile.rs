use std::collections::HashMap;

use crate::aria::node::AriaNode;
use browser::ref_map::{RefMap, Resolution};

fn structural_path(ancestors: &[(&str, &str)]) -> String {
    ancestors
        .iter()
        .map(|(role, name)| format!("{role}:{}", name.replace('|', "\\|")))
        .collect::<Vec<_>>()
        .join("|")
}

fn node_name(node: &AriaNode) -> &str {
    node.name.as_deref().unwrap_or("")
}

fn ancestor_refs(ancestors: &[(String, String)]) -> Vec<(&str, &str)> {
    ancestors
        .iter()
        .map(|(role, name)| (role.as_str(), name.as_str()))
        .collect()
}

fn node_identity_key(node: &AriaNode, ancestors: &[(String, String)]) -> String {
    let refs = ancestor_refs(ancestors);
    identity_key(&node.role, node_name(node), &refs)
}

#[must_use]
pub fn identity_key(role: &str, name: &str, ancestors: &[(&str, &str)]) -> String {
    format!(
        "{role}|{}|{}",
        name.replace('|', "\\|"),
        structural_path(ancestors)
    )
}

pub fn assign_refs(
    node: &mut AriaNode,
    ref_map: &mut RefMap,
    frame_id: Option<&str>,
    ancestors: &mut Vec<(String, String)>,
) {
    let is_virtual = node.role == "document" && node.ref_id.is_none();
    if node.role == "text" || is_virtual {
        for child in &mut node.children {
            assign_refs(child, ref_map, frame_id, ancestors);
        }
        return;
    }

    let name = node_name(node).to_string();
    let refs = ancestor_refs(ancestors);
    let stable_key = identity_key(&node.role, &name, &refs);
    let ref_id = match node.ref_id.as_deref() {
        Some(existing) => ref_map.bind_existing(&stable_key, existing, &node.role, &name, frame_id),
        None => ref_map.assign_by_identity(
            &stable_key,
            &node.role,
            &name,
            frame_id,
            Resolution::Attr(String::new()),
        ),
    };
    node.ref_id = Some(ref_id);

    ancestors.push((node.role.clone(), name));
    let iframe_child_frame = node.frame_id.clone();
    for child in &mut node.children {
        let child_frame_id = if node.role == "iframe" {
            iframe_child_frame.as_deref()
        } else {
            frame_id
        };
        assign_refs(child, ref_map, child_frame_id, ancestors);
    }
    ancestors.pop();
}

pub fn reconcile(prev: &AriaNode, current: &mut AriaNode, ancestors: &mut Vec<(String, String)>) {
    let refs = ancestor_refs(ancestors);
    let prev_key = identity_key(&prev.role, node_name(prev), &refs);
    let curr_key = identity_key(&current.role, node_name(current), &refs);

    if prev_key != curr_key {
        return;
    }

    if let Some(ref_id) = &prev.ref_id {
        current.ref_id = Some(ref_id.clone());
    }

    let current_name = node_name(current).to_string();
    ancestors.push((current.role.clone(), current_name));

    let mut prev_children_by_key: HashMap<String, Vec<&AriaNode>> = HashMap::new();
    for prev_child in &prev.children {
        let key = node_identity_key(prev_child, ancestors);
        prev_children_by_key
            .entry(key)
            .or_default()
            .push(prev_child);
    }

    let mut seen_per_key: HashMap<String, usize> = HashMap::new();
    for current_child in &mut current.children {
        let key = node_identity_key(current_child, ancestors);
        let occurrence = seen_per_key.entry(key.clone()).or_insert(0);
        if let Some(prev_matches) = prev_children_by_key.get(&key) {
            if let Some(prev_child) = prev_matches.get(*occurrence) {
                reconcile(prev_child, current_child, ancestors);
            }
        }
        *occurrence += 1;
    }

    ancestors.pop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aria::node::{AriaNode, AriaStates};

    fn btn(name: &str) -> AriaNode {
        AriaNode {
            role: "button".to_string(),
            name: Some(name.to_string()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![],
            omitted_children: 0,
        }
    }

    fn assign_with_fresh_map(tree: &mut AriaNode) {
        let mut ref_map = RefMap::new();
        assign_refs(tree, &mut ref_map, None, &mut vec![]);
    }

    fn with_ref(mut node: AriaNode, ref_id: &str) -> AriaNode {
        node.ref_id = Some(ref_id.to_string());
        node
    }

    fn main_with(children: Vec<AriaNode>, ref_id: &str) -> AriaNode {
        with_ref(
            AriaNode {
                role: "main".to_string(),
                name: Some(String::new()),
                states: AriaStates::default(),
                ref_id: None,
                url: None,
                frame_id: None,
                offscreen: false,
                children,
                omitted_children: 0,
            },
            ref_id,
        )
    }

    #[test]
    fn same_node_same_key() {
        let k1 = identity_key("button", "Submit", &[]);
        let k2 = identity_key("button", "Submit", &[]);
        assert_eq!(k1, k2);
    }

    #[test]
    fn different_name_different_key() {
        let k1 = identity_key("button", "Submit", &[]);
        let k2 = identity_key("button", "Cancel", &[]);
        assert_ne!(k1, k2);
    }

    #[test]
    fn different_path_different_key() {
        let k1 = identity_key("button", "Submit", &[("main", "")]);
        let k2 = identity_key("button", "Submit", &[("dialog", "")]);
        assert_ne!(k1, k2);
    }

    #[test]
    fn assign_refs_stamps_ref_id() {
        let mut tree = AriaNode {
            role: "main".to_string(),
            name: Some(String::new()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![btn("Submit")],
            omitted_children: 0,
        };
        assign_with_fresh_map(&mut tree);
        assert!(tree.ref_id.is_some());
        assert!(tree.children[0].ref_id.is_some());
    }

    #[test]
    fn assign_refs_skips_virtual_document_root() {
        let mut tree = AriaNode {
            role: "document".to_string(),
            name: None,
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![AriaNode {
                role: "generic".to_string(),
                name: Some(String::new()),
                states: AriaStates::default(),
                ref_id: None,
                url: None,
                frame_id: None,
                offscreen: false,
                children: vec![btn("Learn more")],
                omitted_children: 0,
            }],
            omitted_children: 0,
        };

        assign_with_fresh_map(&mut tree);

        assert_eq!(tree.ref_id, None);
        assert_eq!(tree.children[0].ref_id.as_deref(), Some("e1"));
        assert_eq!(tree.children[0].children[0].ref_id.as_deref(), Some("e2"));
    }

    #[test]
    fn reconcile_preserves_ref_across_snapshot() {
        let mut tree1 = AriaNode {
            role: "main".to_string(),
            name: Some(String::new()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![btn("Submit"), btn("Cancel")],
            omitted_children: 0,
        };
        assign_with_fresh_map(&mut tree1);
        let submit_ref = tree1.children[0].ref_id.clone().unwrap();

        let mut tree2 = AriaNode {
            role: "main".to_string(),
            name: Some(String::new()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![btn("Submit"), btn("Cancel")],
            omitted_children: 0,
        };
        assign_with_fresh_map(&mut tree2);

        reconcile(&tree1, &mut tree2, &mut vec![]);
        assert_eq!(
            tree2.children[0].ref_id.as_deref(),
            Some(submit_ref.as_str())
        );
    }

    #[test]
    fn insert_sibling_does_not_churn_unrelated() {
        let mut tree1 = AriaNode {
            role: "main".to_string(),
            name: Some(String::new()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![btn("Submit"), btn("Cancel")],
            omitted_children: 0,
        };
        assign_with_fresh_map(&mut tree1);
        let cancel_ref = tree1.children[1].ref_id.clone().unwrap();

        let mut tree2 = AriaNode {
            role: "main".to_string(),
            name: Some(String::new()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![btn("New Button"), btn("Submit"), btn("Cancel")],
            omitted_children: 0,
        };
        assign_with_fresh_map(&mut tree2);
        reconcile(&tree1, &mut tree2, &mut vec![]);

        let cancel_node = tree2
            .children
            .iter()
            .find(|node| node.name.as_deref() == Some("Cancel"));
        assert_eq!(
            cancel_node.and_then(|node| node.ref_id.as_deref()),
            Some(cancel_ref.as_str())
        );
    }

    #[test]
    fn duplicate_siblings_preserve_occurrence_order() {
        let mut tree1 = AriaNode {
            role: "main".to_string(),
            name: Some(String::new()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![btn("Action"), btn("Action")],
            omitted_children: 0,
        };
        assign_with_fresh_map(&mut tree1);
        let first_ref = tree1.children[0].ref_id.clone().unwrap();
        let second_ref = tree1.children[1].ref_id.clone().unwrap();

        let mut tree2 = AriaNode {
            role: "main".to_string(),
            name: Some(String::new()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![btn("Action"), btn("Action")],
            omitted_children: 0,
        };
        assign_with_fresh_map(&mut tree2);

        reconcile(&tree1, &mut tree2, &mut vec![]);

        assert_eq!(
            tree2.children[0].ref_id.as_deref(),
            Some(first_ref.as_str())
        );
        assert_eq!(
            tree2.children[1].ref_id.as_deref(),
            Some(second_ref.as_str())
        );
    }

    #[test]
    fn assign_refs_preserves_distinct_dom_refs_for_duplicate_siblings() {
        let mut tree = main_with(
            vec![
                with_ref(btn("Action"), "e2"),
                with_ref(btn("Action"), "e3"),
                with_ref(btn("Submit"), "e4"),
            ],
            "e1",
        );

        assign_with_fresh_map(&mut tree);

        assert_eq!(tree.ref_id.as_deref(), Some("e1"));
        assert_eq!(tree.children[0].ref_id.as_deref(), Some("e2"));
        assert_eq!(tree.children[1].ref_id.as_deref(), Some("e3"));
        assert_eq!(tree.children[2].ref_id.as_deref(), Some("e4"));
        assert_ne!(tree.children[0].ref_id, tree.children[1].ref_id);
    }

    #[test]
    fn assign_refs_mints_past_stamped_ids_without_collision() {
        let mut tree = main_with(vec![with_ref(btn("Saved"), "e5"), btn("Synthetic")], "e1");

        assign_with_fresh_map(&mut tree);

        assert_eq!(tree.children[0].ref_id.as_deref(), Some("e5"));
        let minted = tree.children[1].ref_id.as_deref().unwrap();
        assert_ne!(minted, "e5");
        assert_eq!(minted, "e6");
    }
}
