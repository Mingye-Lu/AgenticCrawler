//! Per-provider message transformation infrastructure.
//!
//! Some LLM providers have constraints on tool call IDs or other message fields.
//! This module provides a trait-based system to apply provider-specific transformations
//! to requests before sending them to the API.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::types::MessageRequest;

/// Trait for provider-specific message transformations.
///
/// Implementations can override any of these methods to apply provider-specific
/// constraints or transformations to requests.
pub trait ProviderTransform: Send + Sync {
    /// Transform the entire request before sending.
    ///
    /// Default implementation does nothing.
    fn transform_request(&self, _request: &mut MessageRequest) {}

    /// Transform a tool call ID to meet provider constraints.
    ///
    /// Default implementation returns the ID unchanged.
    fn transform_tool_call_id(&self, id: &str) -> String {
        id.to_string()
    }

    /// Whether this provider requires alternating message roles.
    ///
    /// Default implementation returns false.
    fn requires_alternating_roles(&self) -> bool {
        false
    }

    fn clone_boxed(&self) -> Box<dyn ProviderTransform>;
}

/// No-op transform that passes all requests through unchanged.
#[derive(Debug, Clone, Copy)]
pub struct NoOpTransform;

impl ProviderTransform for NoOpTransform {
    fn clone_boxed(&self) -> Box<dyn ProviderTransform> {
        Box::new(*self)
    }
}

/// Mistral-specific transform that scrubs tool call IDs to 9-char alphanumeric.
///
/// Mistral has a constraint that tool call IDs must be exactly 9 alphanumeric characters.
#[derive(Debug, Clone, Copy)]
pub struct MistralTransform;

/// Base36 alphabet (digits + lowercase letters) used to encode the hash into
/// alphanumeric characters Mistral accepts.
const BASE36_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Length Mistral requires for tool call IDs.
const MISTRAL_ID_LEN: usize = 9;

impl ProviderTransform for MistralTransform {
    fn transform_tool_call_id(&self, id: &str) -> String {
        // Upstream ids share a fixed prefix (e.g. `toolu_01…`, `call_…`), so
        // simply stripping non-alphanumeric characters and taking the first 9
        // leaves almost no entropy -- most of those 9 characters are the
        // constant prefix. Instead, derive all 9 output characters from a
        // deterministic hash of the *entire* original id, so every byte of
        // the input id contributes to the output and near-identical ids
        // (differing only in a suffix) don't collide.
        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        let mut value = hasher.finish();

        let base = BASE36_ALPHABET.len() as u64;
        let mut chars = [b'0'; MISTRAL_ID_LEN];
        for slot in chars.iter_mut().rev() {
            let digit = usize::try_from(value % base).expect("value % 36 fits in usize");
            *slot = BASE36_ALPHABET[digit];
            value /= base;
        }

        String::from_utf8(chars.to_vec()).expect("base36 alphabet is ASCII")
    }

    fn clone_boxed(&self) -> Box<dyn ProviderTransform> {
        Box::new(*self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_transform_passes_through() {
        let transform = NoOpTransform;
        assert_eq!(
            transform.transform_tool_call_id("call_abc123"),
            "call_abc123"
        );
        assert_eq!(transform.transform_tool_call_id("test-id"), "test-id");
        assert!(!transform.requires_alternating_roles());
    }

    #[test]
    fn test_mistral_transform_output_is_nine_alphanumeric_chars() {
        let transform = MistralTransform;

        for id in [
            "call_abc123def456",
            "call-abc-123-def-456",
            "abc",
            "abcdefghi",
            "---",
            "CallABC123",
            "toolu_016a09aa2b3e4c5d",
        ] {
            let out = transform.transform_tool_call_id(id);
            assert_eq!(out.len(), 9, "id {id:?} -> {out:?} must be 9 chars long");
            assert!(
                out.chars().all(|c| c.is_ascii_alphanumeric()),
                "id {id:?} -> {out:?} must be alphanumeric"
            );
        }
    }

    #[test]
    fn test_mistral_transform_is_deterministic() {
        let transform = MistralTransform;
        let id = "call_abc123def456";
        assert_eq!(
            transform.transform_tool_call_id(id),
            transform.transform_tool_call_id(id)
        );
    }

    /// Regression test: upstream ids sharing a fixed prefix (e.g. `toolu_01…`,
    /// `call_…`) must not collide just because their prefixes overlap. Taking
    /// a positional prefix of the stripped id previously mapped
    /// `call_abc123def456` and `call_abc999xyz789` to the same output
    /// (`callabc12`), since both begin with `callabc12` once non-alphanumeric
    /// characters are stripped.
    #[test]
    fn test_mistral_transform_does_not_collide_on_shared_prefix() {
        let transform = MistralTransform;

        let a = transform.transform_tool_call_id("call_abc123def456");
        let b = transform.transform_tool_call_id("call_abc999xyz789");
        assert_ne!(a, b, "ids sharing a common prefix must not collide");

        // A larger sample of ids sharing the same "call_" prefix, varying
        // only in a short random-looking suffix (the realistic shape of
        // upstream tool-call ids), should be pairwise distinct.
        let ids: Vec<String> = (0..50).map(|i| format!("call_{i:06}")).collect();
        let outputs: std::collections::HashSet<String> = ids
            .iter()
            .map(|id| transform.transform_tool_call_id(id))
            .collect();
        assert_eq!(
            outputs.len(),
            ids.len(),
            "expected all 50 prefix-sharing ids to map to distinct 9-char ids"
        );
    }

    #[test]
    fn test_mistral_transform_requires_alternating_roles() {
        let transform = MistralTransform;
        assert!(!transform.requires_alternating_roles());
    }
}
