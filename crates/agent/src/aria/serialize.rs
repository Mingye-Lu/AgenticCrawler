use std::fmt::Write as _;

use crate::aria::node::{AriaNode, AriaStates};

const COLLAPSE_HOMOGENEOUS_THRESHOLD: usize = 5;
const MAX_CHILDREN_PER_PARENT: usize = 50;
const MAX_TOTAL_NODES: usize = 2_000;
const DEGRADED_DEPTH: usize = 1;

#[must_use]
pub fn to_yaml(root: &AriaNode, depth: Option<usize>) -> String {
    let effective_depth = if count_emitted_nodes(root, depth, 0) > MAX_TOTAL_NODES {
        Some(DEGRADED_DEPTH)
    } else {
        depth
    };

    let mut lines = Vec::new();
    if root.role == "document" && root.ref_id.is_none() {
        for child in &root.children {
            serialize_node(child, 0, effective_depth, &mut lines);
        }
    } else {
        serialize_node(root, 0, effective_depth, &mut lines);
    }
    lines.join("\n")
}

fn serialize_node(
    node: &AriaNode,
    current_depth: usize,
    max_depth: Option<usize>,
    lines: &mut Vec<String>,
) {
    let indent = "  ".repeat(current_depth);

    if node.role == "text" {
        lines.push(format!(
            "{indent}- text: {}",
            escape_name(node.name.as_deref().unwrap_or(""))
        ));
        return;
    }

    let mut line = format!(
        "{indent}- {} {}",
        node.role,
        escape_name(node.name.as_deref().unwrap_or(""))
    );

    for state in render_states(&node.states) {
        line.push(' ');
        line.push_str(&state);
    }

    if let Some(ref_id) = &node.ref_id {
        let _ = write!(line, " [ref={ref_id}]");
    }

    line.push(':');
    lines.push(line);

    let child_indent = "  ".repeat(current_depth + 1);
    if let Some(url) = &node.url {
        lines.push(format!("{child_indent}/url: {url}"));
    }

    if let Some((_, count)) = should_collapse_children(&node.children) {
        let indent = "  ".repeat(current_depth + 1);
        if let Some(ref_id) = &node.ref_id {
            lines.push(format!(
                "{indent}- [{count} children collapsed — use read_content(@{ref_id}) to read]"
            ));
        } else {
            lines.push(format!("{indent}- [{count} children collapsed]"));
        }
        return;
    }

    if max_depth.is_some_and(|max| current_depth >= max) {
        push_omitted_marker(
            lines,
            current_depth + 1,
            node.children.len() + node.omitted_children,
        );
        return;
    }

    for child in node.children.iter().take(MAX_CHILDREN_PER_PARENT) {
        serialize_node(child, current_depth + 1, max_depth, lines);
    }

    let omitted_children =
        node.omitted_children + node.children.len().saturating_sub(MAX_CHILDREN_PER_PARENT);
    push_omitted_marker(lines, current_depth + 1, omitted_children);
}

const NON_INTERACTIVE_COLLAPSIBLE_ROLES: &[&str] = &[
    "generic",
    "text",
    "listitem",
    "paragraph",
    "none",
    "presentation",
    "cell",
    "row",
    "group",
];

fn should_collapse_children(children: &[AriaNode]) -> Option<(String, usize)> {
    if children.len() < COLLAPSE_HOMOGENEOUS_THRESHOLD {
        return None;
    }

    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for child in children {
        *counts.entry(child.role.as_str()).or_insert(0) += 1;
    }

    let (dominant_role, count) = counts.into_iter().max_by_key(|(_, count)| *count)?;

    if count * 5 < children.len() * 4 {
        return None;
    }

    if !NON_INTERACTIVE_COLLAPSIBLE_ROLES.contains(&dominant_role) {
        return None;
    }

    Some((dominant_role.to_string(), count))
}

fn render_states(states: &AriaStates) -> Vec<String> {
    let mut rendered = Vec::new();

    if states.active {
        rendered.push("[active]".to_string());
    }
    if states.checked {
        rendered.push("[checked]".to_string());
    }
    if states.disabled {
        rendered.push("[disabled]".to_string());
    }
    match states.expanded {
        Some(true) => rendered.push("[expanded]".to_string()),
        Some(false) => rendered.push("[expanded=false]".to_string()),
        None => {}
    }
    if states.invalid {
        rendered.push("[invalid]".to_string());
    }
    if let Some(level) = states.level {
        rendered.push(format!("[level={level}]"));
    }
    if states.pressed == Some(true) {
        rendered.push("[pressed=true]".to_string());
    }
    if states.selected {
        rendered.push("[selected]".to_string());
    }

    rendered
}

fn push_omitted_marker(lines: &mut Vec<String>, depth: usize, omitted_children: usize) {
    if omitted_children == 0 {
        return;
    }

    let indent = "  ".repeat(depth);
    lines.push(format!("{indent}- {omitted_children} children omitted"));
}

fn count_emitted_nodes(node: &AriaNode, max_depth: Option<usize>, current_depth: usize) -> usize {
    let mut count = 1;

    if node.role == "text" {
        return count;
    }

    if max_depth.is_some_and(|max| current_depth >= max) {
        if node.children.len() + node.omitted_children > 0 {
            count += 1;
        }
        return count;
    }

    for child in node.children.iter().take(MAX_CHILDREN_PER_PARENT) {
        count += count_emitted_nodes(child, max_depth, current_depth + 1);
        if count > MAX_TOTAL_NODES {
            return count;
        }
    }

    if node.omitted_children + node.children.len().saturating_sub(MAX_CHILDREN_PER_PARENT) > 0 {
        count += 1;
    }

    count
}

fn escape_name(raw: &str) -> String {
    let truncated = truncate_name(raw);
    let mut escaped = String::with_capacity(truncated.len() + 2);
    escaped.push('"');

    for ch in truncated.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }

    escaped.push('"');
    escaped
}

fn truncate_name(raw: &str) -> String {
    if raw.chars().count() < 200 {
        return raw.to_string();
    }

    let prefix = raw
        .char_indices()
        .nth(199)
        .map_or(raw, |(idx, _)| &raw[..idx]);
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_button(name: &str, ref_id: &str) -> AriaNode {
        AriaNode {
            role: "button".to_string(),
            name: Some(name.to_string()),
            states: AriaStates::default(),
            ref_id: Some(ref_id.to_string()),
            url: None,
            frame_id: None,
            offscreen: false,
            children: Vec::new(),
            omitted_children: 0,
        }
    }

    fn node(role: &str, name: Option<&str>, children: Vec<AriaNode>) -> AriaNode {
        AriaNode {
            role: role.to_string(),
            name: name.map(str::to_string),
            states: AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children,
            omitted_children: 0,
        }
    }

    #[test]
    fn test_basic_node() {
        let yaml = to_yaml(&make_button("Save", "e7"), None);
        assert_eq!(yaml, "- button \"Save\" [ref=e7]:");
    }

    #[test]
    fn test_empty_name() {
        let yaml = to_yaml(
            &AriaNode {
                role: "main".to_string(),
                name: None,
                states: AriaStates::default(),
                ref_id: Some("e1".to_string()),
                url: None,
                frame_id: None,
                offscreen: false,
                children: Vec::new(),
                omitted_children: 0,
            },
            None,
        );

        assert_eq!(yaml, "- main \"\" [ref=e1]:");
    }

    #[test]
    fn test_all_states() {
        let yaml = to_yaml(
            &AriaNode {
                role: "tab".to_string(),
                name: Some("Overview".to_string()),
                states: AriaStates {
                    disabled: true,
                    checked: true,
                    expanded: Some(false),
                    pressed: Some(true),
                    selected: true,
                    level: Some(3),
                    active: true,
                    invalid: true,
                },
                ref_id: Some("e9".to_string()),
                url: None,
                frame_id: None,
                offscreen: false,
                children: Vec::new(),
                omitted_children: 0,
            },
            None,
        );

        assert_eq!(
            yaml,
            "- tab \"Overview\" [active] [checked] [disabled] [expanded=false] [invalid] [level=3] [pressed=true] [selected] [ref=e9]:"
        );
    }

    #[test]
    fn test_link_with_url() {
        let yaml = to_yaml(
            &AriaNode {
                role: "link".to_string(),
                name: Some("Docs".to_string()),
                states: AriaStates::default(),
                ref_id: Some("e3".to_string()),
                url: Some("https://example.com/docs".to_string()),
                frame_id: None,
                offscreen: false,
                children: Vec::new(),
                omitted_children: 0,
            },
            None,
        );

        assert_eq!(
            yaml,
            "- link \"Docs\" [ref=e3]:\n  /url: https://example.com/docs"
        );
    }

    #[test]
    fn test_escape_double_quote() {
        let yaml = to_yaml(&make_button("Say \"hello\" world", "e1"), None);
        assert_eq!(yaml, "- button \"Say \\\"hello\\\" world\" [ref=e1]:");
    }

    #[test]
    fn test_escape_backslash() {
        let yaml = to_yaml(&make_button(r"C:\Temp\file.txt", "e1"), None);
        assert_eq!(yaml, "- button \"C:\\\\Temp\\\\file.txt\" [ref=e1]:");
    }

    #[test]
    fn test_escape_newline() {
        let yaml = to_yaml(&make_button("Line one\nLine two", "e1"), None);
        assert_eq!(yaml, "- button \"Line one\\nLine two\" [ref=e1]:");
    }

    #[test]
    fn test_emoji_passthrough() {
        let yaml = to_yaml(&make_button("Hello 🌍 World", "e1"), None);
        assert_eq!(yaml, "- button \"Hello 🌍 World\" [ref=e1]:");
    }

    #[test]
    fn test_truncate_200_chars() {
        let input = "a".repeat(250);
        let expected = format!("- button \"{}…\" [ref=e1]:", "a".repeat(199));
        assert_eq!(to_yaml(&make_button(&input, "e1"), None), expected);
    }

    #[test]
    fn test_no_truncate_199_chars() {
        let input = "a".repeat(199);
        let expected = format!("- button \"{input}\" [ref=e1]:");
        assert_eq!(to_yaml(&make_button(&input, "e1"), None), expected);
    }

    #[test]
    fn test_truncate_200_exact() {
        let input = "a".repeat(200);
        let expected = format!("- button \"{}…\" [ref=e1]:", "a".repeat(199));
        assert_eq!(to_yaml(&make_button(&input, "e1"), None), expected);
    }

    #[test]
    fn test_depth_truncation() {
        let tree = node(
            "main",
            Some(""),
            vec![node(
                "region",
                Some("Level 1"),
                vec![node(
                    "region",
                    Some("Level 2"),
                    vec![node("button", Some("Deep"), Vec::new())],
                )],
            )],
        );

        let yaml = to_yaml(&tree, Some(2));
        assert_eq!(
            yaml,
            "- main \"\":\n  - region \"Level 1\":\n    - region \"Level 2\":\n      - 1 children omitted"
        );
    }

    #[test]
    fn test_child_cap_50() {
        let children = (1..=55)
            .map(|idx| make_button(&format!("Button {idx}"), &format!("e{idx}")))
            .collect();
        let tree = node("main", Some(""), children);

        let yaml = to_yaml(&tree, None);
        let lines = yaml.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 52);
        assert_eq!(lines[0], "- main \"\":");
        assert_eq!(lines[50], "  - button \"Button 50\" [ref=e50]:");
        assert_eq!(lines[51], "  - 5 children omitted");
    }

    #[test]
    fn test_text_node() {
        let text = AriaNode {
            role: "text".to_string(),
            name: Some("Fast browser automation".to_string()),
            states: AriaStates::default(),
            ref_id: Some("e99".to_string()),
            url: Some("https://example.com/ignored".to_string()),
            frame_id: None,
            offscreen: false,
            children: vec![make_button("Ignored", "e2")],
            omitted_children: 3,
        };

        assert_eq!(to_yaml(&text, None), "- text: \"Fast browser automation\"");
    }

    #[test]
    fn test_nested_indentation() {
        let tree = node(
            "main",
            Some(""),
            vec![node(
                "navigation",
                Some("Primary"),
                vec![AriaNode {
                    role: "link".to_string(),
                    name: Some("Pricing".to_string()),
                    states: AriaStates::default(),
                    ref_id: Some("e3".to_string()),
                    url: Some("https://example.com/pricing".to_string()),
                    frame_id: None,
                    offscreen: false,
                    children: vec![AriaNode {
                        role: "text".to_string(),
                        name: Some("Fast browser automation".to_string()),
                        states: AriaStates::default(),
                        ref_id: None,
                        url: None,
                        frame_id: None,
                        offscreen: false,
                        children: Vec::new(),
                        omitted_children: 0,
                    }],
                    omitted_children: 0,
                }],
            )],
        );

        assert_eq!(
            to_yaml(&tree, None),
            "- main \"\":\n  - navigation \"Primary\":\n    - link \"Pricing\" [ref=e3]:\n      /url: https://example.com/pricing\n      - text: \"Fast browser automation\""
        );
    }

    fn ref_el(role: &str, name: &str, ref_id: &str, children: Vec<AriaNode>) -> AriaNode {
        AriaNode {
            role: role.to_string(),
            name: Some(name.to_string()),
            states: AriaStates::default(),
            ref_id: Some(ref_id.to_string()),
            url: None,
            frame_id: None,
            offscreen: false,
            children,
            omitted_children: 0,
        }
    }

    fn ref_link(name: &str, ref_id: &str, url: &str) -> AriaNode {
        AriaNode {
            url: Some(url.to_string()),
            ..ref_el("link", name, ref_id, Vec::new())
        }
    }

    fn ref_heading(name: &str, ref_id: &str, level: u8) -> AriaNode {
        AriaNode {
            states: AriaStates {
                level: Some(level),
                ..AriaStates::default()
            },
            ..ref_el("heading", name, ref_id, Vec::new())
        }
    }

    #[test]
    fn test_golden_nested_regions_fixture() {
        let tree = ref_el(
            "document",
            "",
            "e1",
            vec![
                ref_el(
                    "banner",
                    "",
                    "e2",
                    vec![ref_heading("My Site Title", "e3", 1)],
                ),
                ref_el(
                    "navigation",
                    "Primary",
                    "e4",
                    vec![
                        ref_link("Home", "e5", "/home"),
                        ref_link("About", "e6", "/about"),
                        ref_link("Contact", "e7", "/contact"),
                    ],
                ),
                ref_el(
                    "main",
                    "",
                    "e8",
                    vec![
                        ref_el(
                            "navigation",
                            "Breadcrumb",
                            "e9",
                            vec![
                                ref_link("Root", "e10", "/"),
                                ref_link("Section", "e11", "/section"),
                            ],
                        ),
                        ref_el(
                            "region",
                            "Content",
                            "e12",
                            vec![ref_el(
                                "article",
                                "",
                                "e13",
                                vec![ref_heading("Article Heading", "e14", 2)],
                            )],
                        ),
                    ],
                ),
                ref_el("contentinfo", "", "e15", Vec::new()),
            ],
        );

        let expected = [
            "- document \"\" [ref=e1]:",
            "  - banner \"\" [ref=e2]:",
            "    - heading \"My Site Title\" [level=1] [ref=e3]:",
            "  - navigation \"Primary\" [ref=e4]:",
            "    - link \"Home\" [ref=e5]:",
            "      /url: /home",
            "    - link \"About\" [ref=e6]:",
            "      /url: /about",
            "    - link \"Contact\" [ref=e7]:",
            "      /url: /contact",
            "  - main \"\" [ref=e8]:",
            "    - navigation \"Breadcrumb\" [ref=e9]:",
            "      - link \"Root\" [ref=e10]:",
            "        /url: /",
            "      - link \"Section\" [ref=e11]:",
            "        /url: /section",
            "    - region \"Content\" [ref=e12]:",
            "      - article \"\" [ref=e13]:",
            "        - heading \"Article Heading\" [level=2] [ref=e14]:",
            "  - contentinfo \"\" [ref=e15]:",
        ]
        .join("\n");

        assert_eq!(to_yaml(&tree, None), expected);
    }

    fn leaf(role: &str, ref_id: Option<&str>) -> AriaNode {
        AriaNode {
            role: role.to_string(),
            name: None,
            states: AriaStates::default(),
            ref_id: ref_id.map(str::to_string),
            url: None,
            frame_id: None,
            offscreen: false,
            children: Vec::new(),
            omitted_children: 0,
        }
    }

    #[test]
    fn test_collapse_homogeneous_generic_children() {
        let children = (0..10).map(|_| leaf("generic", None)).collect();
        let tree = ref_el("region", "Log", "e1", children);

        let yaml = to_yaml(&tree, None);

        assert_eq!(
            yaml,
            "- region \"Log\" [ref=e1]:\n  - [10 children collapsed — use read_content(@e1) to read]"
        );
    }

    #[test]
    fn test_collapse_homogeneous_listitem_children() {
        let children = (0..8).map(|_| leaf("listitem", None)).collect();
        let tree = ref_el("list", "Items", "e2", children);

        let yaml = to_yaml(&tree, None);

        assert_eq!(
            yaml,
            "- list \"Items\" [ref=e2]:\n  - [8 children collapsed — use read_content(@e2) to read]"
        );
    }

    #[test]
    fn test_no_collapse_below_threshold() {
        let children = (0..3).map(|_| leaf("generic", None)).collect();
        let tree = node("region", Some("Log"), children);

        let yaml = to_yaml(&tree, None);

        assert!(!yaml.contains("children collapsed"));
        assert_eq!(yaml.lines().count(), 4);
    }

    #[test]
    fn test_no_collapse_mixed_roles() {
        let mut children: Vec<AriaNode> = (0..4).map(|_| leaf("generic", None)).collect();
        children.extend((0..2).map(|_| leaf("button", None)));
        children.extend((0..3).map(|_| leaf("link", None)));
        let tree = node("region", Some("Mixed"), children);

        let yaml = to_yaml(&tree, None);

        assert!(!yaml.contains("children collapsed"));
        assert_eq!(yaml.lines().count(), 10);
    }

    #[test]
    fn test_collapse_without_parent_ref_id() {
        let children = (0..20).map(|_| leaf("generic", None)).collect();
        let tree = node("region", Some("Log"), children);

        let yaml = to_yaml(&tree, None);

        assert_eq!(yaml, "- region \"Log\":\n  - [20 children collapsed]");
    }

    #[test]
    fn test_collapse_preserves_non_collapsible_children() {
        let mut children: Vec<AriaNode> = (0..2)
            .map(|idx| make_button(&format!("Button {idx}"), &format!("e{idx}")))
            .collect();
        children
            .extend((0..2).map(|idx| ref_link(&format!("Link {idx}"), &format!("l{idx}"), "/x")));
        let tree = node("toolbar", Some("Actions"), children);

        let yaml = to_yaml(&tree, None);

        assert!(!yaml.contains("children collapsed"));
        assert!(yaml.contains("button \"Button 0\""));
        assert!(yaml.contains("link \"Link 0\""));
    }
}
