use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFingerprint {
    pub url: String,
    pub element_count: usize,
    pub text_hash: u64,
}

impl PageFingerprint {
    /// Compute a fingerprint from URL and `page_map` data.
    /// `page_map` is the JSON value returned by the `page_map` tool.
    /// Only hashes the first 1000 chars of visible text to stay cheap.
    #[must_use]
    pub fn compute(url: &str, page_map: &Value) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        let element_count = page_map
            .get("interactive")
            .and_then(|i| i.get("counts"))
            .and_then(|c| c.get("total"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;

        let text = extract_page_text(page_map);
        let truncated: String = text.chars().take(1000).collect();
        let text_hash = simple_hash(&truncated);

        Self {
            url: url.to_string(),
            element_count,
            text_hash,
        }
    }

    #[must_use]
    pub fn pages_identical(a: &PageFingerprint, b: &PageFingerprint) -> bool {
        a == b
    }
}

fn extract_page_text(page_map: &Value) -> String {
    let mut parts = Vec::new();

    if let Some(headings) = page_map.get("headings").and_then(Value::as_array) {
        for h in headings {
            if let Some(text) = h.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }

    if let Some(links) = page_map.get("links").and_then(Value::as_array) {
        for link in links {
            if let Some(text) = link.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }

    parts.join(" ")
}

/// FNV-1a 64-bit hash — zero dependencies, deterministic
fn simple_hash(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;
    let mut hash = FNV_OFFSET;
    for byte in s.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_page_map(headings: &[&str], links: &[&str], total_interactive: u64) -> Value {
        json!({
            "headings": headings.iter().map(|t| json!({"text": t, "level": 1})).collect::<Vec<_>>(),
            "links": links.iter().map(|t| json!({"text": t, "href": "https://example.com"})).collect::<Vec<_>>(),
            "interactive": {
                "counts": {"total": total_interactive}
            },
            "meta": {"url": "https://example.com", "title": "Test"}
        })
    }

    #[test]
    fn identical_pages_produce_identical_fingerprints() {
        let pm = sample_page_map(&["Welcome", "About"], &["Home", "Contact"], 5);
        let fp1 = PageFingerprint::compute("https://example.com", &pm);
        let fp2 = PageFingerprint::compute("https://example.com", &pm);

        assert_eq!(fp1, fp2);
        assert!(PageFingerprint::pages_identical(&fp1, &fp2));
    }

    #[test]
    fn different_text_produces_different_fingerprints() {
        let pm1 = sample_page_map(&["Welcome"], &["Home"], 3);
        let pm2 = sample_page_map(&["Goodbye"], &["Away"], 3);
        let fp1 = PageFingerprint::compute("https://example.com", &pm1);
        let fp2 = PageFingerprint::compute("https://example.com", &pm2);

        assert_ne!(fp1, fp2);
        assert!(!PageFingerprint::pages_identical(&fp1, &fp2));
    }

    #[test]
    fn url_change_produces_different_fingerprint() {
        let pm = sample_page_map(&["Welcome"], &["Home"], 3);
        let fp1 = PageFingerprint::compute("https://example.com/page1", &pm);
        let fp2 = PageFingerprint::compute("https://example.com/page2", &pm);

        assert_ne!(fp1, fp2);
        assert!(!PageFingerprint::pages_identical(&fp1, &fp2));
    }

    #[test]
    fn empty_page_map_produces_valid_fingerprint() {
        let pm = json!({});
        let fp = PageFingerprint::compute("https://empty.com", &pm);

        assert_eq!(fp.url, "https://empty.com");
        assert_eq!(fp.element_count, 0);
        assert_eq!(fp.text_hash, simple_hash(""));
    }

    #[test]
    fn text_truncated_at_1000_chars() {
        let long_heading = "A".repeat(1200);
        let pm = json!({
            "headings": [{"text": long_heading, "level": 1}],
            "links": [],
            "interactive": {"counts": {"total": 0}}
        });

        let fp = PageFingerprint::compute("https://example.com", &pm);
        let expected_hash = simple_hash(&"A".repeat(1000));
        assert_eq!(fp.text_hash, expected_hash);
    }

    #[test]
    fn element_count_extracted_from_interactive_total() {
        let pm = json!({
            "headings": [],
            "links": [],
            "interactive": {
                "counts": {"total": 42, "buttons": 10, "inputs": 32}
            }
        });

        let fp = PageFingerprint::compute("https://example.com", &pm);
        assert_eq!(fp.element_count, 42);
    }
}
