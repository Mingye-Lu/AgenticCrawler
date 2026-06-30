use std::collections::HashMap;

/// How a ref is resolved to a DOM element within its owning frame.
///
/// This is the unified `[ref=eN]` resolution model from the ARIA-tree spec
/// (sections "Ref Mechanism (D1)" and "Ref Stability (D2)"):
/// - `Attr` is the canonical, frame-local resolution for every stamped node.
/// - `Selector` is the legacy/fallback path retained for interactive nodes
///   so existing action and self-healing paths keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Primary resolution: query `[data-acrawl-ref='eN']` within the owning frame.
    /// Holds the `eN` value (without the leading `@`).
    Attr(String),
    /// Fallback resolution for interactive nodes: a raw CSS selector.
    Selector(String),
}

/// An entry in the unified ref namespace.
///
/// This is a backward-compatible superset of the original `RefEntry`: the
/// `selector`, `role`, and `name` fields are preserved verbatim so existing
/// callers (`ref_resolve`, `page_map`) keep compiling, while the new
/// `stable_key`, `frame_id`, `resolution`, and `fallback_selector` fields
/// power the frame-aware identity-keyed model.
#[derive(Debug, Clone)]
pub struct RefEntry {
    /// Stable identity key: an opaque string derived from
    /// `(role, accessible-name, structural-path)` by the caller (T9).
    /// `RefMap` treats this as an opaque identity token and never parses it.
    pub stable_key: String,
    /// ARIA role string (e.g. "button", "link").
    pub role: String,
    /// Accessible name (empty string if none).
    pub name: String,
    /// Owning frame identifier (`None` = main frame).
    pub frame_id: Option<String>,
    /// How to resolve this ref to a DOM element.
    pub resolution: Resolution,
    /// Legacy CSS selector. Mirrors the `Selector` resolution payload and is
    /// empty for `Attr`-resolved (stamped) nodes. Retained for the existing
    /// `ref_resolve`/`page_map` call sites until T10 migrates them.
    pub selector: String,
    /// CSS selector fallback (for interactive nodes, kept for self-healing).
    /// `None` for stamped non-interactive/container nodes.
    pub fallback_selector: Option<String>,
}

/// Maps element references (e.g. "e1", "e5") to their entries.
///
/// Provides stable, frame-aware ref assignment: the same identity key always
/// gets the same ref number on the same normalized URL. Identity matching
/// never keys on the ref id itself (spec D2). Refs are cleared only on URL
/// change, preserving the existing `clear()` trigger semantics.
#[derive(Debug, Clone)]
pub struct RefMap {
    /// Maps `ref_id` (e.g. "e1") to `RefEntry`.
    map: HashMap<String, RefEntry>,
    /// Maps a stable identity key to its `ref_id` (for stable reuse lookup).
    /// The legacy `assign_or_reuse` path stores the selector string here as
    /// its identity key, so same-selector dedup still holds.
    key_to_ref: HashMap<String, String>,
    next_ref: usize,
}

impl RefMap {
    /// Create a new empty `RefMap`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            key_to_ref: HashMap::new(),
            next_ref: 1,
        }
    }

    /// Assign a ref by identity key (stable across snapshots on the same URL).
    ///
    /// If `stable_key` is already known, the existing `eN` is REUSED and the
    /// stored entry is left untouched (re-walking an unchanged node never
    /// mints a new ref). Otherwise a new `eN` is minted.
    ///
    /// `resolution` selects the DOM resolution strategy:
    /// - `Resolution::Selector(css)` mirrors `css` into both `selector` and
    ///   `fallback_selector`.
    /// - `Resolution::Attr(_)` is normalized so the stored payload equals the
    ///   assigned `eN`; `selector` is empty and `fallback_selector` is `None`.
    ///
    /// Returns the `ref_id` string (e.g. "e1", "e5").
    pub fn assign_by_identity(
        &mut self,
        stable_key: &str,
        role: &str,
        name: &str,
        frame_id: Option<&str>,
        resolution: Resolution,
    ) -> String {
        if let Some(existing) = self.key_to_ref.get(stable_key) {
            return existing.clone();
        }
        let ref_id = format!("e{}", self.next_ref);
        self.next_ref += 1;

        let (selector, fallback_selector, resolution) = match resolution {
            Resolution::Selector(s) => (s.clone(), Some(s.clone()), Resolution::Selector(s)),
            // Normalize the stamped payload to the assigned eN so the stored
            // resolution is self-consistent with the map key.
            Resolution::Attr(_) => (String::new(), None, Resolution::Attr(ref_id.clone())),
        };

        self.map.insert(
            ref_id.clone(),
            RefEntry {
                stable_key: stable_key.to_string(),
                role: role.to_string(),
                name: name.to_string(),
                frame_id: frame_id.map(String::from),
                resolution,
                selector,
                fallback_selector,
            },
        );
        self.key_to_ref
            .insert(stable_key.to_string(), ref_id.clone());
        ref_id
    }

    /// Legacy selector-keyed assignment. If an element with the same selector
    /// already has a ref, that ref is returned; otherwise a new ref is minted.
    ///
    /// Backward-compatible shim over [`RefMap::assign_by_identity`]: the
    /// selector doubles as the identity key and as a `Selector` resolution.
    /// Kept so existing call sites compile until T10 migrates them.
    pub fn assign_or_reuse(&mut self, selector: &str, role: &str, name: &str) -> String {
        self.assign_by_identity(
            selector,
            role,
            name,
            None,
            Resolution::Selector(selector.to_string()),
        )
    }

    /// Resolve a `ref_id` (e.g. "e5") to its owning frame and DOM query.
    ///
    /// Returns `(frame_id, query)` where:
    /// - for `Attr` resolution `query == "[data-acrawl-ref='eN']"`,
    /// - for `Selector` resolution `query` is the stored CSS selector.
    ///
    /// `frame_id` is `None` for the main frame.
    #[must_use]
    pub fn resolve(&self, ref_id: &str) -> Option<(Option<String>, String)> {
        let entry = self.map.get(ref_id)?;
        let query = match &entry.resolution {
            Resolution::Attr(attr) => format!("[data-acrawl-ref='{attr}']"),
            Resolution::Selector(sel) => sel.clone(),
        };
        Some((entry.frame_id.clone(), query))
    }

    /// Look up a `RefEntry` by `ref_id` (e.g. "e1").
    #[must_use]
    pub fn get(&self, ref_id: &str) -> Option<&RefEntry> {
        self.map.get(ref_id)
    }

    /// Reverse-resolve a stored DOM query or fallback selector back to its `ref_id`.
    #[must_use]
    pub fn ref_id_for_query(&self, query: &str) -> Option<&str> {
        self.map.iter().find_map(|(ref_id, entry)| {
            let resolved_query = match &entry.resolution {
                Resolution::Attr(attr) => format!("[data-acrawl-ref='{attr}']"),
                Resolution::Selector(selector) => selector.clone(),
            };

            if resolved_query == query || entry.fallback_selector.as_deref() == Some(query) {
                Some(ref_id.as_str())
            } else {
                None
            }
        })
    }

    /// Clear all refs and reset the counter to 1.
    ///
    /// Called on URL change. Preserves the existing trigger semantics: every
    /// ref, identity key, and the counter are reset so a fresh page starts
    /// from `e1` again.
    pub fn clear(&mut self) {
        self.map.clear();
        self.key_to_ref.clear();
        self.next_ref = 1;
    }
}

impl Default for RefMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse @eN or bare eN → returns `Some("eN")` or None.
/// Accepts: @e1, @e99, @e123, e1, e99, e123
/// Rejects: @ex, @e, #foo, .bar, @r1, empty string
#[must_use]
pub fn parse_ref(input: &str) -> Option<String> {
    let stripped = input.strip_prefix('@').unwrap_or(input);
    if stripped.starts_with('e') && stripped.len() > 1 {
        let digits = &stripped[1..];
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return Some(stripped.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ref_with_at_prefix_e1() {
        assert_eq!(parse_ref("@e1"), Some("e1".to_string()));
    }

    #[test]
    fn parse_ref_with_at_prefix_e99() {
        assert_eq!(parse_ref("@e99"), Some("e99".to_string()));
    }

    #[test]
    fn parse_ref_bare_e5() {
        assert_eq!(parse_ref("e5"), Some("e5".to_string()));
    }

    #[test]
    fn parse_ref_rejects_at_ex() {
        assert_eq!(parse_ref("@ex"), None);
    }

    #[test]
    fn parse_ref_rejects_at_e() {
        assert_eq!(parse_ref("@e"), None);
    }

    #[test]
    fn parse_ref_rejects_hash_selector() {
        assert_eq!(parse_ref("#my-button"), None);
    }

    #[test]
    fn parse_ref_rejects_empty_string() {
        assert_eq!(parse_ref(""), None);
    }

    #[test]
    fn parse_ref_with_at_prefix_e123() {
        assert_eq!(parse_ref("@e123"), Some("e123".to_string()));
    }

    #[test]
    fn assign_or_reuse_same_selector_twice() {
        let mut map = RefMap::new();
        let ref1 = map.assign_or_reuse("button.submit", "button", "Submit");
        let ref2 = map.assign_or_reuse("button.submit", "button", "Submit");
        assert_eq!(ref1, ref2);
        assert_eq!(ref1, "e1");
    }

    #[test]
    fn assign_or_reuse_different_selectors_increments() {
        let mut map = RefMap::new();
        let ref1 = map.assign_or_reuse("button.submit", "button", "Submit");
        let ref2 = map.assign_or_reuse("input.email", "textbox", "Email");
        assert_eq!(ref1, "e1");
        assert_eq!(ref2, "e2");
    }

    #[test]
    fn clear_resets_counter() {
        let mut map = RefMap::new();
        let ref1 = map.assign_or_reuse("button.submit", "button", "Submit");
        assert_eq!(ref1, "e1");
        map.clear();
        let ref2 = map.assign_or_reuse("button.submit", "button", "Submit");
        assert_eq!(ref2, "e1");
    }

    // --- Unified identity-keyed / frame-aware behavior (T6) ---

    #[test]
    fn test_same_identity_reuses_ref() {
        let mut map = RefMap::new();
        let first = map.assign_by_identity(
            "key_K",
            "button",
            "Submit",
            None,
            Resolution::Attr(String::new()),
        );
        assert_eq!(first, "e1");
        let again = map.assign_by_identity(
            "key_K",
            "button",
            "Submit",
            None,
            Resolution::Attr(String::new()),
        );
        assert_eq!(again, "e1");
        let other = map.assign_by_identity(
            "key_L",
            "link",
            "Home",
            None,
            Resolution::Attr(String::new()),
        );
        assert_eq!(other, "e2");
    }

    #[test]
    fn test_clear_resets_counter() {
        let mut map = RefMap::new();
        let first = map.assign_by_identity(
            "key_K",
            "button",
            "Submit",
            None,
            Resolution::Attr(String::new()),
        );
        assert_eq!(first, "e1");
        map.clear();
        let after = map.assign_by_identity(
            "key_K",
            "button",
            "Submit",
            None,
            Resolution::Attr(String::new()),
        );
        assert_eq!(after, "e1");
    }

    #[test]
    fn test_resolve_attr_resolution() {
        let mut map = RefMap::new();
        let ref_id = map.assign_by_identity(
            "key_attr",
            "button",
            "Submit",
            Some("f2"),
            Resolution::Attr(String::new()),
        );
        let (frame_id, query) = map.resolve(&ref_id).expect("ref should resolve");
        assert_eq!(frame_id, Some("f2".to_string()));
        assert_eq!(query, format!("[data-acrawl-ref='{ref_id}']"));
        assert_eq!(
            map.get(&ref_id).unwrap().resolution,
            Resolution::Attr(ref_id.clone())
        );
    }

    #[test]
    fn test_resolve_selector_resolution() {
        let mut map = RefMap::new();
        let ref_id = map.assign_by_identity(
            "key_sel",
            "button",
            "Submit",
            None,
            Resolution::Selector("button.submit".to_string()),
        );
        let (frame_id, query) = map.resolve(&ref_id).expect("ref should resolve");
        assert_eq!(frame_id, None);
        assert_eq!(query, "button.submit");
        let entry = map.get(&ref_id).unwrap();
        assert_eq!(entry.selector, "button.submit");
        assert_eq!(entry.fallback_selector, Some("button.submit".to_string()));
    }

    #[test]
    fn test_resolve_unknown_ref_is_none() {
        let map = RefMap::new();
        assert!(map.resolve("e999").is_none());
    }

    #[test]
    fn test_parse_ref() {
        assert_eq!(parse_ref("@e5"), Some("e5".to_string()));
        // Bare eN remains accepted for backward compatibility: existing call
        // sites pass bare refs (see `parse_ref_bare_e5`), so this deliberately
        // diverges from the T6 stub's `e5 -> None`.
        assert_eq!(parse_ref("e5"), Some("e5".to_string()));
        assert_eq!(parse_ref("@r1"), None);
        assert_eq!(parse_ref("#foo"), None);
    }

    #[test]
    fn legacy_assign_or_reuse_populates_selector_and_resolution() {
        let mut map = RefMap::new();
        let ref_id = map.assign_or_reuse("button.submit", "button", "Submit");
        let entry = map.get(&ref_id).unwrap();
        assert_eq!(entry.selector, "button.submit");
        assert_eq!(entry.role, "button");
        assert_eq!(entry.name, "Submit");
        assert_eq!(entry.frame_id, None);
        assert_eq!(
            entry.resolution,
            Resolution::Selector("button.submit".to_string())
        );
        assert_eq!(
            map.resolve(&ref_id),
            Some((None, "button.submit".to_string()))
        );
    }

    #[test]
    fn frame_id_is_preserved_per_entry() {
        let mut map = RefMap::new();
        let main_ref = map.assign_by_identity(
            "k_main",
            "link",
            "Home",
            None,
            Resolution::Attr(String::new()),
        );
        let frame_ref = map.assign_by_identity(
            "k_frame",
            "link",
            "Docs",
            Some("frame-7"),
            Resolution::Attr(String::new()),
        );
        assert_eq!(map.resolve(&main_ref).unwrap().0, None);
        assert_eq!(
            map.resolve(&frame_ref).unwrap().0,
            Some("frame-7".to_string())
        );
    }
}
