//! Settings-aware lifecycle for the Chrome extension browser backend.
//!
//! The interactive UI and the MCP server both need the same three steps to put
//! the extension backend into service: start [`WsBridgeServer`] on the
//! configured port/token, wait for the extension to dial in, then hand out a
//! [`SharedBridge`]. This module owns them so neither front end reimplements it.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use browser::{
    generate_bridge_token, read_bridge_file, BrowserBackend, ExtensionBridge, SharedBridge,
    WsBridgeServer,
};
use tokio::sync::watch;

pub const EXTENSION_BACKEND: &str = "extension";
pub const DEFAULT_BRIDGE_PORT: u16 = 19876;
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;

#[must_use]
pub fn extension_backend_selected() -> bool {
    runtime::load_settings().browser_backend.as_deref() == Some(EXTENSION_BACKEND)
}

/// Generates and persists an `extension_bridge_token` if one is not already
/// set. Called when the user enables the extension backend via
/// `acrawl config set browser_backend extension`, so that a subsequent
/// `acrawl config get extension_bridge_token` returns a usable token instead of
/// `null` — the token would otherwise only be minted on the first browser tool
/// call, which is the very call that needs the token to authenticate.
#[must_use]
pub fn ensure_bridge_token() -> String {
    let (_, token) = resolve_bridge_config();
    token
}

/// Generates and persists a token on first use so the copy already stored in
/// the extension stays valid across restarts.
fn resolve_bridge_config() -> (u16, String) {
    let settings = runtime::load_settings();
    let token = settings
        .extension_bridge_token
        .unwrap_or_else(generate_bridge_token);
    let _ = runtime::update_settings(|s| {
        s.extension_bridge_token = Some(token.clone());
    });
    let port = settings
        .extension_bridge_port
        .unwrap_or(DEFAULT_BRIDGE_PORT);
    (port, token)
}

#[must_use]
pub fn connect_timeout() -> Duration {
    let secs = runtime::load_settings()
        .extension_bridge_connect_timeout_secs
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

fn describe_bind_conflict(port: u16, error: &str) -> String {
    match read_bridge_file() {
        Some(info) if info.port == port && info.pid != std::process::id() => format!(
            "extension bridge port {port} is already in use by another acrawl process \
             (pid {pid} per bridge.json). The Chrome extension serves one process at a \
             time: stop extension mode there, or set a different `extension_bridge_port`.",
            pid = info.pid
        ),
        _ => format!("failed to start extension bridge on port {port}: {error}"),
    }
}

pub struct ExtensionBridgeManager {
    server: WsBridgeServer,
    token: String,
    /// Lazily-created [`SharedBridge`], cached so that every consumer (direct
    /// tools, script execution, and `run_goal`) shares a single
    /// [`ExtensionBridge`] — and therefore a single monotonically-increasing
    /// command-id counter. Creating a fresh bridge per consumer would restart
    /// command IDs at 1 while `run_ws_session` indexes pending responders only
    /// by ID, letting overlapping commands overwrite each other's responder.
    shared_bridge: OnceLock<SharedBridge>,
}

impl ExtensionBridgeManager {
    pub async fn start(port: u16, token: String) -> Result<Self, String> {
        match WsBridgeServer::start(port, token.clone()).await {
            Ok(server) => Ok(Self {
                server,
                token,
                shared_bridge: OnceLock::new(),
            }),
            Err(error) => Err(describe_bind_conflict(port, &error.to_string())),
        }
    }

    pub async fn start_from_settings() -> Result<Self, String> {
        let (port, token) = resolve_bridge_config();
        Self::start(port, token).await
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.server.port()
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.server.is_client_connected()
    }

    #[must_use]
    pub fn connection_watcher(&self) -> watch::Receiver<bool> {
        self.server.connection_watcher()
    }

    /// Safe to call before the extension connects: [`ExtensionBridge`] resolves
    /// the connection state per command, not at construction time.
    #[must_use]
    pub fn shared_bridge(&self) -> SharedBridge {
        self.shared_bridge
            .get_or_init(|| {
                let bridge =
                    ExtensionBridge::new(self.server.command_sender(), self.connection_watcher());
                Arc::new(tokio::sync::Mutex::new(
                    Box::new(bridge) as Box<dyn BrowserBackend + Send>
                ))
            })
            .clone()
    }

    pub async fn wait_for_connection(&mut self, timeout: Duration) -> bool {
        self.server.wait_for_connection(timeout).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_conflict_without_matching_record_reports_raw_error() {
        let message = describe_bind_conflict(1, "address in use");
        assert!(message.contains("address in use"), "{message}");
        assert!(!message.contains("bridge.json"), "{message}");
    }

    #[test]
    fn extension_backend_constant_matches_settings_value() {
        assert_eq!(EXTENSION_BACKEND, "extension");
    }

    #[test]
    fn default_bridge_port_matches_extension_default() {
        assert_eq!(DEFAULT_BRIDGE_PORT, 19876);
    }
}
