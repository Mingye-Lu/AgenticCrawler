use std::collections::HashMap;

/// Represents a single interactive element with its selector, role, and name.
#[derive(Debug, Clone)]
pub struct RefEntry {
    pub selector: String,
    pub role: String,
    pub name: String,
}

/// Maps element references (e.g. "e1", "e5") to their entries.
/// Provides stable ref assignment: same selector always gets the same ref number.
#[derive(Debug, Clone)]
pub struct RefMap {
    /// Maps `ref_id` (e.g. "e1") to `RefEntry`
    map: HashMap<String, RefEntry>,
    /// Maps selector to `ref_id` (for stable lookup)
    selector_to_ref: HashMap<String, String>,
    next_ref: usize,
}

impl RefMap {
    /// Create a new empty `RefMap`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            selector_to_ref: HashMap::new(),
            next_ref: 1,
        }
    }

    /// If element with same selector already has a ref, return that ref.
    /// Otherwise assign next available ref number and return it.
    /// Returns the `ref_id` string (e.g. "e1", "e5").
    pub fn assign_or_reuse(&mut self, selector: &str, role: &str, name: &str) -> String {
        if let Some(existing) = self.selector_to_ref.get(selector) {
            return existing.clone();
        }
        let ref_id = format!("e{}", self.next_ref);
        self.next_ref += 1;
        self.map.insert(
            ref_id.clone(),
            RefEntry {
                selector: selector.to_string(),
                role: role.to_string(),
                name: name.to_string(),
            },
        );
        self.selector_to_ref
            .insert(selector.to_string(), ref_id.clone());
        ref_id
    }

    /// Look up a `RefEntry` by `ref_id` (e.g. "e1").
    #[must_use]
    pub fn get(&self, ref_id: &str) -> Option<&RefEntry> {
        self.map.get(ref_id)
    }

    /// Clear all refs and reset the counter to 1.
    pub fn clear(&mut self) {
        self.map.clear();
        self.selector_to_ref.clear();
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
/// Rejects: @ex, @e, #foo, .bar, empty string
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
}
