use std::fmt;
use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageInfo {
    pub title: String,
    pub html: String,
}

/// Captured browser state for preservation across bridge restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserState {
    pub cookies: serde_json::Value,
    pub local_storage: serde_json::Value,
    pub url: String,
}

#[derive(Debug)]
pub enum BridgeError {
    ProcessSpawn {
        command: String,
        source: io::Error,
    },
    LaunchTimeout {
        timeout: Duration,
    },
    Protocol(String),
    PlaywrightNotInstalled(String),
    Io(io::Error),
    Json(serde_json::Error),
    ChildClosed,
    ShutdownTimeout {
        timeout: Duration,
    },
    CommandTimeout {
        timeout: Duration,
    },
    /// Extension WebSocket disconnected — run /extension to reconnect.
    ExtensionDisconnected,
    /// Extension did not respond within the timeout.
    ExtensionTimeout {
        timeout: Duration,
    },
    /// The operation is not supported by this backend.
    Unsupported(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProcessSpawn { command, source } => write!(
                f,
                "failed to spawn `{command}` for CloakBrowser bridge: {source}. Ensure Node.js and CloakBrowser are installed"
            ),
            Self::LaunchTimeout { timeout } => write!(
                f,
                "CloakBrowser bridge launch exceeded {} seconds",
                timeout.as_secs()
            ),
            Self::Protocol(message) => write!(f, "Browser bridge protocol error: {message}"),
            Self::PlaywrightNotInstalled(message) => write!(
                f,
                "CloakBrowser is not installed: {message}. Install with `npm install cloakbrowser`"
            ),
            Self::Io(error) => write!(f, "CloakBrowser bridge I/O error: {error}"),
            Self::Json(error) => write!(f, "CloakBrowser bridge JSON error: {error}"),
            Self::ChildClosed => write!(f, "CloakBrowser bridge process closed unexpectedly"),
            Self::ShutdownTimeout { timeout } => write!(
                f,
                "CloakBrowser bridge did not shut down within {} seconds",
                timeout.as_secs()
            ),
            Self::CommandTimeout { timeout } => write!(
                f,
                "CloakBrowser bridge command timed out after {} seconds",
                timeout.as_secs()
            ),
            Self::ExtensionDisconnected => {
                write!(
                    f,
                    "Extension disconnected — run /extension to reconnect or /cloakbrowser to switch backends."
                )
            }
            Self::ExtensionTimeout { timeout } => write!(
                f,
                "Extension did not respond within {} seconds. The browser may be busy.",
                timeout.as_secs()
            ),
            Self::Unsupported(message) => write!(f, "Operation not supported: {message}"),
        }
    }
}

impl std::error::Error for BridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProcessSpawn { source, .. } => Some(source),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::LaunchTimeout { .. }
            | Self::Protocol(_)
            | Self::PlaywrightNotInstalled(_)
            | Self::ChildClosed
            | Self::ShutdownTimeout { .. }
            | Self::CommandTimeout { .. }
            | Self::ExtensionDisconnected
            | Self::ExtensionTimeout { .. }
            | Self::Unsupported(_) => None,
        }
    }
}

impl From<io::Error> for BridgeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for BridgeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
#[derive(Debug, Serialize)]
pub(super) struct BridgeCommandEnvelope<'a> {
    pub(super) action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) url: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BridgeBootstrapMessage {
    pub(super) event: String,
    pub(super) ok: bool,
    #[serde(default)]
    pub(super) error: Option<BridgeErrorPayload>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BridgeResponseMessage {
    pub(super) event: String,
    pub(super) ok: bool,
    #[serde(default)]
    pub(super) result: Option<PageInfo>,
    #[serde(default)]
    pub(super) error: Option<BridgeErrorPayload>,
}

/// Generic bridge response that deserializes `result` as arbitrary JSON.
#[derive(Debug, Deserialize)]
pub(super) struct GenericBridgeResponseMessage {
    pub(super) event: String,
    pub(super) ok: bool,
    #[serde(default)]
    pub(super) result: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) error: Option<BridgeErrorPayload>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BridgeErrorPayload {
    pub(super) kind: String,
    pub(super) message: String,
}
