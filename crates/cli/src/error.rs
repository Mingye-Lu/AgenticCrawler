use std::fmt::{Display, Formatter};

use runtime::RuntimeError;

#[derive(Debug)]
pub(crate) enum CliError {
    Runtime(RuntimeError),
    Io(std::io::Error),
    Json(serde_json::Error),
    Other(Box<dyn std::error::Error>),
}

impl Display for CliError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::Other(e) => Some(e.as_ref()),
        }
    }
}

impl From<RuntimeError> for CliError {
    fn from(e: RuntimeError) -> Self {
        Self::Runtime(e)
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<Box<dyn std::error::Error>> for CliError {
    fn from(e: Box<dyn std::error::Error>) -> Self {
        Self::Other(e)
    }
}

impl From<String> for CliError {
    fn from(e: String) -> Self {
        Self::Other(e.into())
    }
}

impl From<&str> for CliError {
    fn from(e: &str) -> Self {
        Self::Other(e.into())
    }
}

impl From<runtime::PromptBuildError> for CliError {
    fn from(e: runtime::PromptBuildError) -> Self {
        Self::Other(Box::new(e))
    }
}

impl From<runtime::ConfigError> for CliError {
    fn from(e: runtime::ConfigError) -> Self {
        Self::Other(Box::new(e))
    }
}

impl From<runtime::SessionError> for CliError {
    fn from(e: runtime::SessionError) -> Self {
        Self::Other(Box::new(e))
    }
}
