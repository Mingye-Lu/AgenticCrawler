//! Settings-aware lifecycle for the Chrome extension browser backend.
//!
//! The interactive UI and the MCP server both need the same three steps to put
//! the extension backend into service: start [`WsBridgeServer`] on the
//! configured port, wait for the extension to dial in, then hand out a
//! [`SharedBridge`]. This module owns them so neither front end reimplements it.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use browser::{read_bridge_file, BrowserBackend, ExtensionBridge, SharedBridge, WsBridgeServer};
use tokio::sync::watch;

pub const EXTENSION_BACKEND: &str = "extension";
pub const DEFAULT_BRIDGE_PORT: u16 = 19876;
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 3;

#[must_use]
pub fn extension_backend_selected() -> bool {
    runtime::load_settings().browser_backend.as_deref() == Some(EXTENSION_BACKEND)
}

#[must_use]
pub fn bridge_port() -> u16 {
    runtime::load_settings()
        .extension_bridge_port
        .unwrap_or(DEFAULT_BRIDGE_PORT)
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
    /// Lazily-created [`SharedBridge`], cached so that every consumer (direct
    /// tools, script execution, and `run_goal`) shares a single
    /// [`ExtensionBridge`] — and therefore a single monotonically-increasing
    /// command-id counter. Creating a fresh bridge per consumer would restart
    /// command IDs at 1 while `run_ws_session` indexes pending responders only
    /// by ID, letting overlapping commands overwrite each other's responder.
    shared_bridge: OnceLock<SharedBridge>,
}

impl ExtensionBridgeManager {
    pub async fn start(port: u16) -> Result<Self, String> {
        match WsBridgeServer::start(port).await {
            Ok(server) => Ok(Self {
                server,
                shared_bridge: OnceLock::new(),
            }),
            Err(error) => Err(describe_bind_conflict(port, &error.to_string())),
        }
    }

    pub async fn start_from_settings() -> Result<Self, String> {
        Self::start(bridge_port()).await
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.server.port()
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

    #[test]
    fn default_connect_timeout_is_short_grace_for_extension_reconnect() {
        assert_eq!(DEFAULT_CONNECT_TIMEOUT_SECS, 3);
    }
}
