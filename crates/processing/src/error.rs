use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Format error: {0}")]
    FormatError(String),
    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("File too large: size {actual_bytes} bytes exceeds limit {limit_bytes} bytes")]
    FileTooLarge { actual_bytes: u64, limit_bytes: u64 },
    #[error("Operation timed out after {seconds} seconds")]
    Timeout { seconds: u64 },
    #[error("Model not found at path: {0}")]
    ModelNotFound(PathBuf),
    #[error("Feature disabled: compile with --features {0} to enable")]
    FeatureDisabled(String),
    #[error("Corrupt or invalid file: {0}")]
    CorruptFile(String),
}
