use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use crate::aria::AriaNode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFingerprint {
    pub url: String,
    pub element_count: usize,
    pub text_hash: u64,
}

impl PageFingerprint {
    /// Compute a deterministic fingerprint from a URL and an ARIA tree snapshot.
    ///
    /// The hash mixes the URL, per-role node counts, landmark names, level-1
    /// heading names, and the `(role, name)` of every named node so relabeling
    /// an interactive control is detected. `element_count` is the total node
    /// count across the ARIA tree.
    #[must_use]
    pub fn compute(url: &str, tree: &AriaNode) -> Self {
        let mut role_counts: BTreeMap<&str, usize> = BTreeMap::new();
        count_roles(tree, &mut role_counts);
        let element_count: usize = role_counts.values().copied().sum();

        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        for (role, count) in &role_counts {
            role.hash(&mut hasher);
            count.hash(&mut hasher);
        }

        let mut landmark_names: Vec<&str> = Vec::new();
        collect_landmark_names(tree, &mut landmark_names);
        for name in &landmark_names {
            name.hash(&mut hasher);
        }

        let mut heading_names: Vec<&str> = Vec::new();
        collect_heading_names(tree, &mut heading_names);
        for name in &heading_names {
            name.hash(&mut hasher);
        }

        let mut named_nodes: Vec<(&str, &str)> = Vec::new();
        collect_named_nodes(tree, &mut named_nodes);
        for (role, name) in &named_nodes {
            role.hash(&mut hasher);
            name.hash(&mut hasher);
        }

        Self {
            url: url.to_string(),
            element_count,
            text_hash: hasher.finish(),
        }
    }

    #[must_use]
    pub fn pages_identical(a: &PageFingerprint, b: &PageFingerprint) -> bool {
        a == b
    }
}

const LANDMARK_ROLES: &[&str] = &[
    "main",
    "navigation",
    "banner",
    "contentinfo",
    "complementary",
    "region",
    "form",
    "search",
];

fn count_roles<'a>(node: &'a AriaNode, counts: &mut BTreeMap<&'a str, usize>) {
    *counts.entry(node.role.as_str()).or_insert(0) += 1;
    for child in &node.children {
        count_roles(child, counts);
    }
}

fn collect_landmark_names<'a>(node: &'a AriaNode, names: &mut Vec<&'a str>) {
    if LANDMARK_ROLES.contains(&node.role.as_str()) {
        if let Some(name) = &node.name {
            names.push(name.as_str());
        }
    }
    for child in &node.children {
        collect_landmark_names(child, names);
    }
}

fn collect_heading_names<'a>(node: &'a AriaNode, names: &mut Vec<&'a str>) {
    if node.role == "heading" && node.states.level == Some(1) {
        if let Some(name) = &node.name {
            names.push(name.as_str());
        }
    }
    for child in &node.children {
        collect_heading_names(child, names);
    }
}

fn collect_named_nodes<'a>(node: &'a AriaNode, out: &mut Vec<(&'a str, &'a str)>) {
    if let Some(name) = &node.name {
        out.push((node.role.as_str(), name.as_str()));
    }
    for child in &node.children {
        collect_named_nodes(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aria::node::{AriaNode, AriaStates};

    fn simple_tree() -> AriaNode {
        AriaNode {
            role: "main".to_string(),
            name: Some("Content".to_string()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![AriaNode {
                role: "button".to_string(),
                name: Some("Submit".to_string()),
                states: AriaStates::default(),
                ref_id: None,
                url: None,
                frame_id: None,
                offscreen: false,
                children: vec![],
                omitted_children: 0,
            }],
            omitted_children: 0,
        }
    }

    fn heading_node(name: &str) -> AriaNode {
        AriaNode {
            role: "heading".to_string(),
            name: Some(name.to_string()),
            states: AriaStates {
                level: Some(1),
                ..AriaStates::default()
            },
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![],
            omitted_children: 0,
        }
    }

    #[test]
    fn same_tree_same_fingerprint() {
        let fp1 = PageFingerprint::compute("https://example.com", &simple_tree());
        let fp2 = PageFingerprint::compute("https://example.com", &simple_tree());
        assert_eq!(fp1, fp2);
        assert!(PageFingerprint::pages_identical(&fp1, &fp2));
    }

    #[test]
    fn different_url_different_fingerprint() {
        let fp1 = PageFingerprint::compute("https://a.com", &simple_tree());
        let fp2 = PageFingerprint::compute("https://b.com", &simple_tree());
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn mutated_tree_different_fingerprint() {
        let t1 = simple_tree();
        let mut t2 = simple_tree();
        t2.children[0].name = Some("Delete".to_string());
        let fp1 = PageFingerprint::compute("https://example.com", &t1);
        let fp2 = PageFingerprint::compute("https://example.com", &t2);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn element_count_counts_all_nodes() {
        let fp = PageFingerprint::compute("https://example.com", &simple_tree());
        assert_eq!(fp.element_count, 2);
    }

    #[test]
    fn added_node_changes_fingerprint() {
        let base = simple_tree();
        let mut grown = simple_tree();
        grown.children.push(AriaNode {
            role: "link".to_string(),
            name: Some("Home".to_string()),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![],
            omitted_children: 0,
        });
        let fp_base = PageFingerprint::compute("https://example.com", &base);
        let fp_grown = PageFingerprint::compute("https://example.com", &grown);
        assert_ne!(fp_base, fp_grown);
        assert_eq!(fp_base.element_count, 2);
        assert_eq!(fp_grown.element_count, 3);
    }

    #[test]
    fn changed_heading_changes_fingerprint() {
        let mut t1 = simple_tree();
        t1.children.push(heading_node("Welcome"));
        let mut t2 = simple_tree();
        t2.children.push(heading_node("Goodbye"));
        let fp1 = PageFingerprint::compute("https://example.com", &t1);
        let fp2 = PageFingerprint::compute("https://example.com", &t2);
        assert_ne!(fp1, fp2);
    }
}
