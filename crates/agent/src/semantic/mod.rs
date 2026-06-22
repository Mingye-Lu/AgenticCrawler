use std::collections::HashSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawElementFacts {
    pub tag: String,
    pub role: Option<String>,
    pub aria_expanded: Option<String>,
    pub aria_selected: Option<String>,
    pub aria_pressed: Option<String>,
    pub aria_controls: Option<String>,
    pub aria_owns: Option<String>,
    pub text: Option<String>,
    pub aria_label: Option<String>,
    pub aria_labelledby_text: Option<String>,
    pub title: Option<String>,
    pub placeholder: Option<String>,
    pub name: Option<String>,
    pub visible: bool,
    pub floating: bool,
    pub selector: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionKind {
    Sidebar,
    Main,
    Dialog,
    Banner,
    Nav,
    Complementary,
    Form,
    Region,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionCandidate {
    pub tag: String,
    pub role: Option<String>,
    pub aria_label: Option<String>,
    pub id: Option<String>,
    pub depth: usize,
    pub parent_idx: Option<usize>,
    pub selector: String,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionNode {
    pub kind: RegionKind,
    pub label: String,
    pub handle: String,
    pub selector: String,
    pub visible: bool,
    pub children: Vec<RegionNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionCandidate {
    pub name: String,
    pub selector: String,
    pub role: Option<String>,
    pub aria_selected: Option<String>,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlInfo {
    pub label: String,
    pub role: String,
    pub selector: String,
    pub region_handle: Option<String>,
    pub value: Option<String>,
    pub required: bool,
    pub disabled: bool,
}

#[must_use]
pub fn compute_accessible_name(facts: &RawElementFacts) -> String {
    trimmed_name(facts.aria_label.as_deref())
        .or_else(|| trimmed_name(facts.aria_labelledby_text.as_deref()))
        .or_else(|| trimmed_name(facts.text.as_deref()))
        .or_else(|| raw_name(facts.placeholder.as_deref()))
        .or_else(|| raw_name(facts.title.as_deref()))
        .or_else(|| raw_name(facts.name.as_deref()))
        .unwrap_or_default()
}

#[must_use]
pub fn region_kind(tag: &str, role: Option<&str>) -> RegionKind {
    if has_role(role, &["dialog", "alertdialog"]) {
        RegionKind::Dialog
    } else if has_role(role, &["navigation"]) || tag.eq_ignore_ascii_case("nav") {
        RegionKind::Nav
    } else if has_role(role, &["main"]) || tag.eq_ignore_ascii_case("main") {
        RegionKind::Main
    } else if has_role(role, &["complementary"]) || tag.eq_ignore_ascii_case("aside") {
        RegionKind::Complementary
    } else if has_role(role, &["banner"]) || tag.eq_ignore_ascii_case("header") {
        RegionKind::Banner
    } else if has_role(role, &["contentinfo"]) || tag.eq_ignore_ascii_case("footer") {
        RegionKind::Complementary
    } else if has_role(role, &["form"]) || tag.eq_ignore_ascii_case("form") {
        RegionKind::Form
    } else if has_role(role, &["region"]) || tag.eq_ignore_ascii_case("section") {
        RegionKind::Region
    } else {
        RegionKind::Other
    }
}

#[must_use]
pub fn region_label(kind: &RegionKind, aria_label: Option<&str>, idx: usize) -> String {
    if let Some(label) = aria_label.map(str::trim).filter(|label| !label.is_empty()) {
        return label.to_string();
    }

    match kind {
        RegionKind::Dialog => "modal dialog".to_string(),
        RegionKind::Sidebar => "sidebar".to_string(),
        RegionKind::Main => "main panel".to_string(),
        RegionKind::Nav => "navigation".to_string(),
        RegionKind::Banner => "banner".to_string(),
        RegionKind::Complementary => "aside".to_string(),
        RegionKind::Form => "form".to_string(),
        RegionKind::Region => format!("region {}", idx + 1),
        RegionKind::Other => format!("section {}", idx + 1),
    }
}

#[must_use]
pub fn assemble_region_tree(candidates: &[RegionCandidate]) -> Vec<RegionNode> {
    let handles = candidates
        .iter()
        .enumerate()
        .map(|(idx, _)| format!("@r{}", idx + 1))
        .collect::<Vec<_>>();
    let included = candidates
        .iter()
        .map(|candidate| candidate.depth <= 3)
        .collect::<Vec<_>>();
    let mut roots = Vec::new();
    let mut children_by_parent = vec![Vec::new(); candidates.len()];

    for (idx, candidate) in candidates.iter().enumerate() {
        if !included[idx] {
            continue;
        }

        if let Some(parent_idx) = candidate
            .parent_idx
            .filter(|&parent_idx| parent_idx < candidates.len() && included[parent_idx])
        {
            children_by_parent[parent_idx].push(idx);
        } else {
            roots.push(idx);
        }
    }

    roots
        .into_iter()
        .map(|idx| build_region_node(idx, candidates, &handles, &children_by_parent))
        .collect()
}

#[must_use]
pub fn match_text(
    query: &str,
    candidates: &[(String, String)],
    _role_filter: Option<&str>,
) -> Option<(String, Vec<String>)> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return None;
    }

    match_same_tier(candidates, |name| name == query)
        .or_else(|| match_same_tier(candidates, |name| name.eq_ignore_ascii_case(query)))
        .or_else(|| match_same_tier(candidates, |name| name.trim() == trimmed_query))
        .or_else(|| {
            let lower_query = trimmed_query.to_lowercase();
            match_same_tier(candidates, |name| {
                name.to_lowercase().contains(&lower_query)
            })
        })
        .or_else(|| match_token_overlap(trimmed_query, candidates))
}

#[must_use]
pub fn select_active_dialog(regions: &[RegionNode]) -> Option<&RegionNode> {
    let mut active_dialog = None;
    collect_last_visible_dialog(regions, &mut active_dialog);
    active_dialog
}

fn build_region_node(
    idx: usize,
    candidates: &[RegionCandidate],
    handles: &[String],
    children_by_parent: &[Vec<usize>],
) -> RegionNode {
    let candidate = &candidates[idx];
    let kind = region_kind(&candidate.tag, candidate.role.as_deref());
    let label = region_label(&kind, candidate.aria_label.as_deref(), idx);
    let children = children_by_parent[idx]
        .iter()
        .copied()
        .map(|child_idx| build_region_node(child_idx, candidates, handles, children_by_parent))
        .collect();

    RegionNode {
        kind,
        label,
        handle: handles[idx].clone(),
        selector: candidate.selector.clone(),
        visible: candidate.visible,
        children,
    }
}

fn collect_last_visible_dialog<'a>(
    regions: &'a [RegionNode],
    active_dialog: &mut Option<&'a RegionNode>,
) {
    for region in regions {
        if region.kind == RegionKind::Dialog && region.visible {
            *active_dialog = Some(region);
        }
        collect_last_visible_dialog(&region.children, active_dialog);
    }
}

fn has_role(role: Option<&str>, expected: &[&str]) -> bool {
    role.is_some_and(|role| {
        expected
            .iter()
            .any(|value| role.eq_ignore_ascii_case(value))
    })
}

fn match_same_tier<F>(
    candidates: &[(String, String)],
    predicate: F,
) -> Option<(String, Vec<String>)>
where
    F: Fn(&str) -> bool,
{
    let matches = candidates
        .iter()
        .filter(|(name, _)| predicate(name))
        .map(|(_, selector)| selector.clone())
        .collect::<Vec<_>>();
    split_best_and_alternatives(matches)
}

fn match_token_overlap(
    query: &str,
    candidates: &[(String, String)],
) -> Option<(String, Vec<String>)> {
    let query_tokens = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<HashSet<_>>();
    let mut best_overlap = 0;
    let mut matches = Vec::new();

    for (name, selector) in candidates {
        let overlap = name
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|token| query_tokens.contains(token))
            .collect::<HashSet<_>>()
            .len();

        if overlap == 0 {
            continue;
        }

        if overlap > best_overlap {
            best_overlap = overlap;
            matches.clear();
        }

        if overlap == best_overlap {
            matches.push(selector.clone());
        }
    }

    split_best_and_alternatives(matches)
}

fn raw_name(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| !value.is_empty())
        .map(truncate_to_sixty_chars)
}

fn split_best_and_alternatives(mut matches: Vec<String>) -> Option<(String, Vec<String>)> {
    if matches.is_empty() {
        None
    } else {
        let best = matches.remove(0);
        Some((best, matches))
    }
}

fn trimmed_name(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(truncate_to_sixty_chars)
}

fn truncate_to_sixty_chars(value: &str) -> String {
    value.chars().take(60).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_facts() -> RawElementFacts {
        RawElementFacts {
            tag: "button".to_string(),
            role: None,
            aria_expanded: None,
            aria_selected: None,
            aria_pressed: None,
            aria_controls: None,
            aria_owns: None,
            text: None,
            aria_label: None,
            aria_labelledby_text: None,
            title: None,
            placeholder: None,
            name: None,
            visible: true,
            floating: false,
            selector: "button.primary".to_string(),
        }
    }

    fn region_candidate(
        tag: &str,
        role: Option<&str>,
        aria_label: Option<&str>,
        depth: usize,
        parent_idx: Option<usize>,
        selector: &str,
        visible: bool,
    ) -> RegionCandidate {
        RegionCandidate {
            tag: tag.to_string(),
            role: role.map(str::to_string),
            aria_label: aria_label.map(str::to_string),
            id: None,
            depth,
            parent_idx,
            selector: selector.to_string(),
            visible,
        }
    }

    fn region_node(
        kind: RegionKind,
        handle: &str,
        selector: &str,
        visible: bool,
        children: Vec<RegionNode>,
    ) -> RegionNode {
        RegionNode {
            kind,
            label: handle.to_string(),
            handle: handle.to_string(),
            selector: selector.to_string(),
            visible,
            children,
        }
    }

    #[test]
    fn accessible_name_prefers_aria_label() {
        let mut facts = raw_facts();
        facts.aria_label = Some("Primary action".to_string());
        facts.aria_labelledby_text = Some("Secondary action".to_string());
        facts.text = Some("Click me".to_string());

        assert_eq!(compute_accessible_name(&facts), "Primary action");
    }

    #[test]
    fn accessible_name_uses_labelledby_when_label_missing() {
        let mut facts = raw_facts();
        facts.aria_labelledby_text = Some("  External label  ".to_string());

        assert_eq!(compute_accessible_name(&facts), "External label");
    }

    #[test]
    fn accessible_name_uses_text_when_aria_names_missing() {
        let mut facts = raw_facts();
        facts.text = Some("  Visible text  ".to_string());

        assert_eq!(compute_accessible_name(&facts), "Visible text");
    }

    #[test]
    fn raw_element_facts_deserializes_from_dom_snapshot_json() {
        let json = serde_json::json!({
            "elements": [{
                "tag": "button",
                "role": "combobox",
                "aria_expanded": "false",
                "aria_selected": null,
                "aria_pressed": null,
                "aria_controls": "menu-1",
                "aria_owns": null,
                "text": "Choose",
                "aria_label": null,
                "aria_labelledby_text": null,
                "title": null,
                "placeholder": null,
                "name": null,
                "visible": true,
                "floating": false,
                "selector": "button.cmb"
            }]
        });

        let elements = json["elements"].as_array().unwrap();
        let el = &elements[0];
        assert_eq!(el["tag"].as_str().unwrap(), "button");
        assert_eq!(el["role"].as_str().unwrap(), "combobox");
        assert!(el["visible"].as_bool().unwrap());

        let facts: Vec<RawElementFacts> = serde_json::from_value(json["elements"].clone()).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].tag, "button");
        assert_eq!(facts[0].role.as_deref(), Some("combobox"));
        assert!(facts[0].visible);
    }

    #[test]
    fn accessible_name_uses_placeholder_when_text_missing() {
        let mut facts = raw_facts();
        facts.placeholder = Some("Search docs".to_string());

        assert_eq!(compute_accessible_name(&facts), "Search docs");
    }

    #[test]
    fn accessible_name_uses_title_when_placeholder_missing() {
        let mut facts = raw_facts();
        facts.title = Some("Tooltip name".to_string());

        assert_eq!(compute_accessible_name(&facts), "Tooltip name");
    }

    #[test]
    fn accessible_name_uses_name_attr_last() {
        let mut facts = raw_facts();
        facts.name = Some("email".to_string());

        assert_eq!(compute_accessible_name(&facts), "email");
    }

    #[test]
    fn accessible_name_returns_empty_when_all_inputs_missing() {
        assert_eq!(compute_accessible_name(&raw_facts()), "");
    }

    #[test]
    fn accessible_name_truncates_to_sixty_chars() {
        let mut facts = raw_facts();
        facts.aria_label = Some("x".repeat(75));

        assert_eq!(compute_accessible_name(&facts), "x".repeat(60));
    }

    #[test]
    fn region_kind_maps_dialog_roles() {
        assert_eq!(region_kind("div", Some("dialog")), RegionKind::Dialog);
        assert_eq!(region_kind("div", Some("alertdialog")), RegionKind::Dialog);
    }

    #[test]
    fn region_kind_maps_navigation() {
        assert_eq!(region_kind("nav", None), RegionKind::Nav);
        assert_eq!(region_kind("div", Some("navigation")), RegionKind::Nav);
    }

    #[test]
    fn region_kind_maps_main() {
        assert_eq!(region_kind("main", None), RegionKind::Main);
        assert_eq!(region_kind("div", Some("main")), RegionKind::Main);
    }

    #[test]
    fn region_kind_maps_complementary_variants() {
        assert_eq!(region_kind("aside", None), RegionKind::Complementary);
        assert_eq!(
            region_kind("div", Some("complementary")),
            RegionKind::Complementary
        );
        assert_eq!(region_kind("footer", None), RegionKind::Complementary);
        assert_eq!(
            region_kind("div", Some("contentinfo")),
            RegionKind::Complementary
        );
    }

    #[test]
    fn region_kind_maps_banner() {
        assert_eq!(region_kind("header", None), RegionKind::Banner);
        assert_eq!(region_kind("div", Some("banner")), RegionKind::Banner);
    }

    #[test]
    fn region_kind_maps_form() {
        assert_eq!(region_kind("form", None), RegionKind::Form);
        assert_eq!(region_kind("div", Some("form")), RegionKind::Form);
    }

    #[test]
    fn region_kind_maps_region_and_other() {
        assert_eq!(region_kind("section", None), RegionKind::Region);
        assert_eq!(region_kind("div", Some("region")), RegionKind::Region);
        assert_eq!(region_kind("article", None), RegionKind::Other);
    }

    #[test]
    fn region_label_prefers_aria_label() {
        assert_eq!(
            region_label(&RegionKind::Dialog, Some("Admin modal"), 0),
            "Admin modal"
        );
    }

    #[test]
    fn region_label_uses_kind_defaults() {
        assert_eq!(region_label(&RegionKind::Dialog, None, 0), "modal dialog");
        assert_eq!(region_label(&RegionKind::Sidebar, None, 0), "sidebar");
        assert_eq!(region_label(&RegionKind::Main, None, 0), "main panel");
        assert_eq!(region_label(&RegionKind::Nav, None, 0), "navigation");
        assert_eq!(region_label(&RegionKind::Banner, None, 0), "banner");
        assert_eq!(region_label(&RegionKind::Complementary, None, 0), "aside");
        assert_eq!(region_label(&RegionKind::Form, None, 0), "form");
        assert_eq!(region_label(&RegionKind::Region, None, 1), "region 2");
        assert_eq!(region_label(&RegionKind::Other, None, 2), "section 3");
    }

    #[test]
    fn assemble_region_tree_builds_flat_roots() {
        let candidates = vec![
            region_candidate("main", None, None, 0, None, "main", true),
            region_candidate("nav", None, None, 0, None, "nav", true),
        ];

        let regions = assemble_region_tree(&candidates);

        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].handle, "@r1");
        assert_eq!(regions[1].handle, "@r2");
        assert!(regions.iter().all(|region| region.children.is_empty()));
    }

    #[test]
    fn assemble_region_tree_builds_nested_children() {
        let candidates = vec![
            region_candidate("main", None, Some("Workspace"), 0, None, "main", true),
            region_candidate("aside", None, Some("Filters"), 1, Some(0), "aside", true),
            region_candidate(
                "section",
                None,
                Some("Subpanel"),
                2,
                Some(1),
                "section",
                true,
            ),
        ];

        let regions = assemble_region_tree(&candidates);

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].handle, "@r1");
        assert_eq!(regions[0].children[0].handle, "@r2");
        assert_eq!(regions[0].children[0].children[0].handle, "@r3");
    }

    #[test]
    fn assemble_region_tree_caps_depth_at_three() {
        let candidates = vec![
            region_candidate("main", None, None, 0, None, "main", true),
            region_candidate("section", None, None, 1, Some(0), "section.one", true),
            region_candidate("section", None, None, 2, Some(1), "section.two", true),
            region_candidate("section", None, None, 3, Some(2), "section.three", true),
            region_candidate("section", None, None, 4, Some(3), "section.four", true),
        ];

        let regions = assemble_region_tree(&candidates);
        let third_level = &regions[0].children[0].children[0].children[0];

        assert_eq!(third_level.handle, "@r4");
        assert!(third_level.children.is_empty());
    }

    #[test]
    fn assemble_region_tree_preserves_document_order_handles() {
        let candidates = vec![
            region_candidate("main", None, None, 0, None, "main", true),
            region_candidate("dialog", Some("dialog"), None, 0, None, "dialog", true),
            region_candidate("section", None, None, 1, Some(0), "section", true),
        ];

        let regions = assemble_region_tree(&candidates);

        assert_eq!(regions[0].handle, "@r1");
        assert_eq!(regions[1].handle, "@r2");
        assert_eq!(regions[0].children[0].handle, "@r3");
    }

    #[test]
    fn match_text_prefers_exact_match() {
        let candidates = vec![
            ("Save draft".to_string(), "#save-draft".to_string()),
            ("Save".to_string(), "#save".to_string()),
        ];

        assert_eq!(
            match_text("Save", &candidates, None),
            Some(("#save".to_string(), Vec::new()))
        );
    }

    #[test]
    fn match_text_falls_back_to_case_insensitive_match() {
        let candidates = vec![("Save".to_string(), "#save".to_string())];

        assert_eq!(
            match_text("save", &candidates, None),
            Some(("#save".to_string(), Vec::new()))
        );
    }

    #[test]
    fn match_text_falls_back_to_trimmed_match() {
        let candidates = vec![("Save".to_string(), "#save".to_string())];

        assert_eq!(
            match_text("  Save  ", &candidates, None),
            Some(("#save".to_string(), Vec::new()))
        );
    }

    #[test]
    fn match_text_falls_back_to_contains_match() {
        let candidates = vec![("Save changes".to_string(), "#save".to_string())];

        assert_eq!(
            match_text("save", &candidates, None),
            Some(("#save".to_string(), Vec::new()))
        );
    }

    #[test]
    fn match_text_uses_token_overlap_when_needed() {
        let candidates = vec![
            ("Export report csv".to_string(), "#csv".to_string()),
            ("Export report pdf".to_string(), "#pdf".to_string()),
        ];

        assert_eq!(
            match_text("report export", &candidates, None),
            Some(("#csv".to_string(), vec!["#pdf".to_string()]))
        );
    }

    #[test]
    fn match_text_returns_none_when_no_match_exists() {
        let candidates = vec![("Delete".to_string(), "#delete".to_string())];

        assert_eq!(match_text("Archive", &candidates, None), None);
    }

    #[test]
    fn match_text_returns_alternatives_for_same_tier_ties() {
        let candidates = vec![
            ("Save changes".to_string(), "#save-primary".to_string()),
            (
                "Save current state".to_string(),
                "#save-secondary".to_string(),
            ),
        ];

        assert_eq!(
            match_text("save", &candidates, None),
            Some((
                "#save-primary".to_string(),
                vec!["#save-secondary".to_string()]
            ))
        );
    }

    #[test]
    fn select_active_dialog_returns_none_for_empty_regions() {
        assert_eq!(select_active_dialog(&[]), None);
    }

    #[test]
    fn select_active_dialog_returns_single_dialog() {
        let regions = vec![region_node(
            RegionKind::Dialog,
            "@r1",
            "dialog.one",
            true,
            Vec::new(),
        )];

        assert_eq!(
            select_active_dialog(&regions).map(|region| region.handle.as_str()),
            Some("@r1")
        );
    }

    #[test]
    fn select_active_dialog_returns_last_visible_dialog_in_document_order() {
        let regions = vec![
            region_node(RegionKind::Dialog, "@r1", "dialog.one", true, Vec::new()),
            region_node(
                RegionKind::Main,
                "@r2",
                "main",
                true,
                vec![region_node(
                    RegionKind::Dialog,
                    "@r3",
                    "dialog.two",
                    true,
                    Vec::new(),
                )],
            ),
            region_node(RegionKind::Dialog, "@r4", "dialog.three", false, Vec::new()),
        ];

        assert_eq!(
            select_active_dialog(&regions).map(|region| region.handle.as_str()),
            Some("@r3")
        );
    }

    #[test]
    fn select_active_dialog_returns_none_when_no_dialogs_are_visible() {
        let regions = vec![
            region_node(RegionKind::Main, "@r1", "main", true, Vec::new()),
            region_node(RegionKind::Dialog, "@r2", "dialog", false, Vec::new()),
        ];

        assert_eq!(select_active_dialog(&regions), None);
    }
}
