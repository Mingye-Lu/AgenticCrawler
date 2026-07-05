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
    parent_ref_id: Option<&str>,
) {
    let is_virtual = node.role == "document" && node.ref_id.is_none();
    if node.role == "text" || is_virtual {
        for child in &mut node.children {
            assign_refs(child, ref_map, frame_id, ancestors, parent_ref_id);
        }
        return;
    }

    let name = node_name(node).to_string();
    let refs = ancestor_refs(ancestors);
    let stable_key = identity_key(&node.role, &name, &refs);
    let ref_id = match node.ref_id.as_deref() {
        Some(existing) => ref_map.bind_existing(
            &stable_key,
            existing,
            &node.role,
            &name,
            frame_id,
            parent_ref_id,
        ),
        None => ref_map.assign_by_identity(
            &stable_key,
            &node.role,
            &name,
            frame_id,
            Resolution::Attr(String::new()),
            parent_ref_id,
        ),
    };
    node.ref_id = Some(ref_id.clone());

    ancestors.push((node.role.clone(), name));
    let iframe_child_frame = node.frame_id.clone();
    for child in &mut node.children {
        let child_frame_id = if node.role == "iframe" {
            iframe_child_frame.as_deref()
        } else {
            frame_id
        };
        assign_refs(child, ref_map, child_frame_id, ancestors, Some(&ref_id));
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
        assign_refs(tree, &mut ref_map, None, &mut vec![], None);
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

    /// Regression test for the reviewed desync bug: when a new duplicate-named
    /// sibling is inserted in front of existing stamped elements, every node
    /// must keep the ref its DOM stamp carries — no identity-based reshuffling
    /// that would point the displayed ref at a different element's stamp.
    #[test]
    fn duplicate_sibling_insert_keeps_stamped_refs() {
        // Walk 2 arrives with a fresh "New Action" stamped e4, inserted before
        // the previously stamped Action e2 / Action e3.
        let mut tree = main_with(
            vec![
                with_ref(btn("Action"), "e4"),
                with_ref(btn("Action"), "e2"),
                with_ref(btn("Action"), "e3"),
            ],
            "e1",
        );

        assign_with_fresh_map(&mut tree);

        assert_eq!(tree.children[0].ref_id.as_deref(), Some("e4"));
        assert_eq!(tree.children[1].ref_id.as_deref(), Some("e2"));
        assert_eq!(tree.children[2].ref_id.as_deref(), Some("e3"));
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
