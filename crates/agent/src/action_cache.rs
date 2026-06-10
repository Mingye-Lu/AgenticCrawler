use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::page_fingerprint::PageFingerprint;

const DEFAULT_TTL_SECS: u64 = 30;

/// Read-only tools that are safe to cache.
pub const CACHEABLE_TOOLS: &[&str] = &["page_map", "read_content", "list_resources", "execute_js"];

#[must_use]
pub fn is_cacheable(tool_name: &str) -> bool {
    CACHEABLE_TOOLS.contains(&tool_name)
}

#[derive(Debug, Clone)]
pub struct CachedAction {
    pub output: String,
    pub stored_at: Instant,
    pub fingerprint: PageFingerprint,
    pub ttl_secs: u64,
}

impl CachedAction {
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.stored_at.elapsed().as_secs() >= self.ttl_secs
    }
}

#[derive(Debug, Clone)]
pub struct ActionCache {
    entries: HashMap<String, CachedAction>,
    ttl_secs: u64,
}

impl Default for ActionCache {
    fn default() -> Self {
        Self::new(DEFAULT_TTL_SECS)
    }
}

impl ActionCache {
    #[must_use]
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: HashMap::new(),
            ttl_secs,
        }
    }

    /// Build a cache key from `tool_name` + canonical JSON input + page fingerprint.
    #[must_use]
    pub fn make_key(tool_name: &str, input: &Value, fingerprint: &PageFingerprint) -> String {
        let canonical_input = canonicalize_json(input);
        let raw = format!(
            "{tool_name}:{canonical_input}:{}:{}:{}",
            fingerprint.url, fingerprint.element_count, fingerprint.text_hash
        );
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Look up a cached result. Returns None if not found, expired, or fingerprint mismatch.
    pub fn lookup(&mut self, key: &str, current_fingerprint: &PageFingerprint) -> Option<String> {
        self.evict_expired();
        let entry = self.entries.get(key)?;
        if entry.fingerprint != *current_fingerprint {
            return None;
        }
        Some(entry.output.clone())
    }

    /// Store a result in the cache.
    pub fn store(&mut self, key: String, output: String, fingerprint: PageFingerprint) {
        self.evict_expired();
        self.entries.insert(
            key,
            CachedAction {
                output,
                stored_at: Instant::now(),
                fingerprint,
                ttl_secs: self.ttl_secs,
            },
        );
    }

    /// Remove expired entries to avoid unbounded growth.
    pub fn evict_expired(&mut self) {
        self.entries.retain(|_, value| !value.is_expired());
    }
}

fn canonicalize_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(string) => {
            serde_json::to_string(string).unwrap_or_else(|_| "\"\"".to_string())
        }
        Value::Array(items) => {
            let canonical_items = items
                .iter()
                .map(canonicalize_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{canonical_items}]")
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(left, _)| *left);
            let canonical_entries = entries
                .into_iter()
                .map(|(key, nested)| {
                    let encoded_key =
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                    format!("{encoded_key}:{}", canonicalize_json(nested))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{canonical_entries}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fp(url: &str) -> PageFingerprint {
        PageFingerprint {
            url: url.to_string(),
            element_count: 3,
            text_hash: 42,
        }
    }

    #[test]
    fn cache_hit_same_fingerprint() {
        let mut cache = ActionCache::new(30);
        let fingerprint = fp("https://example.com");
        let key = ActionCache::make_key("page_map", &json!({}), &fingerprint);
        cache.store(key.clone(), "result".to_string(), fingerprint.clone());

        let hit = cache.lookup(&key, &fingerprint);

        assert_eq!(hit.as_deref(), Some("result"));
    }

    #[test]
    fn cache_miss_on_fingerprint_change() {
        let mut cache = ActionCache::new(30);
        let fingerprint_one = fp("https://example.com");
        let fingerprint_two = PageFingerprint {
            url: "https://example.com".to_string(),
            element_count: 10,
            text_hash: 99,
        };
        let key = ActionCache::make_key("page_map", &json!({}), &fingerprint_one);
        cache.store(key.clone(), "result".to_string(), fingerprint_one);

        assert!(cache.lookup(&key, &fingerprint_two).is_none());
    }

    #[test]
    fn cache_miss_after_ttl_expires() {
        let mut cache = ActionCache::new(0);
        let fingerprint = fp("https://example.com");
        let key = ActionCache::make_key("page_map", &json!({}), &fingerprint);
        cache.store(key.clone(), "result".to_string(), fingerprint.clone());

        assert!(cache.lookup(&key, &fingerprint).is_none());
    }

    #[test]
    fn interaction_tools_not_cacheable() {
        assert!(!is_cacheable("click"));
        assert!(!is_cacheable("fill_form"));
        assert!(!is_cacheable("navigate"));
        assert!(!is_cacheable("scroll"));
    }

    #[test]
    fn read_tools_are_cacheable() {
        assert!(is_cacheable("page_map"));
        assert!(is_cacheable("read_content"));
        assert!(is_cacheable("list_resources"));
        assert!(is_cacheable("execute_js"));
    }

    #[test]
    fn different_inputs_produce_different_keys() {
        let fingerprint = fp("https://example.com");
        let key_one = ActionCache::make_key("page_map", &json!({}), &fingerprint);
        let key_two = ActionCache::make_key("page_map", &json!({"scope": "#main"}), &fingerprint);

        assert_ne!(key_one, key_two);
    }

    #[test]
    fn equivalent_object_inputs_produce_same_key() {
        let fingerprint = fp("https://example.com");
        let key_one = ActionCache::make_key(
            "read_content",
            &json!({"selector": "#main", "offset": 0, "max_chars": 1000}),
            &fingerprint,
        );
        let key_two = ActionCache::make_key(
            "read_content",
            &json!({"max_chars": 1000, "offset": 0, "selector": "#main"}),
            &fingerprint,
        );

        assert_eq!(key_one, key_two);
    }
}
