use browser::{parse_ref, RefMap};

/// Resolve an @eN ref or bare eN ref to its CSS selector.
/// If input is a CSS selector (not a ref), returns it unchanged.
/// Returns Err if input looks like a ref but is not found in the map.
pub fn resolve_selector(input: &str, ref_map: &RefMap) -> Result<String, String> {
    if let Some(ref_id) = parse_ref(input) {
        match ref_map.get(&ref_id) {
            Some(entry) => Ok(entry.selector.clone()),
            None => Err(format!(
                "Unknown element ref @{ref_id}. Call page_map to refresh."
            )),
        }
    } else {
        Ok(input.to_string())
    }
}

#[cfg(test)]
mod tests {
    use browser::RefMap;

    use super::*;

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
        assert!(err.contains("Unknown element ref"));
        assert!(err.contains("@e999"));
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
}
