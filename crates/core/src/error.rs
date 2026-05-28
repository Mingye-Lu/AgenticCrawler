use std::fmt::{Display, Formatter};

/// Error type for runtime failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    message: String,
}

impl RuntimeError {
    /// Create a new runtime error with the given message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// Error type for tool execution failures in the runtime trait.
///
/// This is a simple wrapper used by the `ToolExecutor` trait to report
/// tool execution errors from the runtime's perspective.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    /// Create a new tool error with the given message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for ToolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

/// Error type for tool execution failures.
///
/// Most failures are `Message(_)`. The `RequiresAsync` variant is a sentinel
/// emitted by the synchronous handler stub for any tool that actually needs
/// the async path; the agent loop matches on it via [`ToolExecutionError::is_requires_async`]
/// instead of substring-matching the error text — which used to misclassify
/// any unrelated error whose message happened to contain the same phrase.
#[derive(Debug)]
pub enum ToolExecutionError {
    /// A plain, user-visible error message.
    Message(String),
    /// The named tool is registered as async-only and must go through
    /// `execute_async`. Carries the tool name purely for diagnostics.
    RequiresAsync { tool_name: String },
}

impl ToolExecutionError {
    /// Construct a plain message error.
    pub fn new<S: Into<String>>(message: S) -> Self {
        Self::Message(message.into())
    }

    /// Construct the sentinel emitted by the registry stub for async-only tools.
    pub fn requires_async<S: Into<String>>(tool_name: S) -> Self {
        Self::RequiresAsync {
            tool_name: tool_name.into(),
        }
    }

    /// True iff this error is the `RequiresAsync` sentinel.
    #[must_use]
    pub const fn is_requires_async(&self) -> bool {
        matches!(self, Self::RequiresAsync { .. })
    }
}

impl std::fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(s) => write!(f, "{s}"),
            Self::RequiresAsync { tool_name } => write!(
                f,
                "tool `{tool_name}` requires async execution via execute_async"
            ),
        }
    }
}

impl std::error::Error for ToolExecutionError {}
