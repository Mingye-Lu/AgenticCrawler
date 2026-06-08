//! Error types for the script crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScriptParseError {
    #[error("failed to deserialize script JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported schema_version {found}; expected {expected}")]
    WrongSchemaVersion { found: u32, expected: u32 },
    #[error("invalid script structure: {0}")]
    Structural(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("unsupported schema_version {found}; expected {expected}")]
    WrongSchemaVersion { found: u32, expected: u32 },
    #[error("unknown tool `{tool}`")]
    UnknownTool { tool: String },
    #[error("nesting depth {depth} exceeds maximum {max}")]
    ExcessiveNesting { depth: usize, max: usize },
    #[error("script size {size_bytes} bytes exceeds maximum {max_bytes} bytes")]
    ScriptTooLarge { size_bytes: usize, max_bytes: usize },
    #[error("parallel branch count {branch_count} exceeds maximum {max}")]
    TooManyParallelBranches { branch_count: usize, max: usize },
    #[error("empty steps in {context}")]
    EmptySteps { context: String },
    #[error("undefined variable `{name}` in {context}")]
    UndefinedVariable { name: String, context: String },
}
