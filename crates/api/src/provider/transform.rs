//! Per-provider message transformation infrastructure.
//!
//! Some LLM providers have constraints on tool call IDs or other message fields.
//! This module provides a trait-based system to apply provider-specific transformations
//! to requests before sending them to the API.

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
/// This transform strips non-alphanumeric characters and truncates/pads to exactly 9 chars.
#[derive(Debug, Clone, Copy)]
pub struct MistralTransform;

impl ProviderTransform for MistralTransform {
    fn transform_tool_call_id(&self, id: &str) -> String {
        // Strip all non-alphanumeric characters
        let alphanumeric: String = id.chars().filter(|c| c.is_alphanumeric()).collect();

        // If we have at least 9 alphanumeric chars, take the first 9
        if alphanumeric.len() >= 9 {
            alphanumeric.chars().take(9).collect()
        } else {
            // Pad with '0' to reach exactly 9 characters
            format!("{alphanumeric:0<9}")
        }
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
    fn test_mistral_transform_scrubs_tool_id() {
        let transform = MistralTransform;

        // Long ID: take first 9 alphanumeric chars
        assert_eq!(
            transform.transform_tool_call_id("call_abc123def456"),
            "callabc12"
        );

        // ID with non-alphanumeric: strip them, then take first 9
        assert_eq!(
            transform.transform_tool_call_id("call-abc-123-def-456"),
            "callabc12"
        );

        // Short ID: pad with '0' to reach 9 chars
        assert_eq!(transform.transform_tool_call_id("abc"), "abc000000");

        // Exactly 9 alphanumeric: pass through
        assert_eq!(transform.transform_tool_call_id("abcdefghi"), "abcdefghi");

        // ID with only non-alphanumeric: becomes all '0's
        assert_eq!(transform.transform_tool_call_id("---"), "000000000");

        // Mixed case preserved
        assert_eq!(transform.transform_tool_call_id("CallABC123"), "CallABC12");
    }

    #[test]
    fn test_mistral_transform_requires_alternating_roles() {
        let transform = MistralTransform;
        assert!(!transform.requires_alternating_roles());
    }
}
