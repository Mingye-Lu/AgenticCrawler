use acrawl_core::ToolError;
use tokio::sync::Mutex;

use super::CrawlerAgent;
use crate::{BrowserBackend, BrowserContext, BrowserState, SharedBridge};

#[derive(Clone)]
pub(crate) struct BrowserSession {
    pub(crate) browser: BrowserContext,
    pub(crate) shared_bridge: SharedBridge,
}

impl BrowserSession {
    /// Initialize with a pre-existing bridge (extension or shared).
    fn from_bridge(shared_bridge: SharedBridge) -> Self {
        Self {
            browser: BrowserContext::new(shared_bridge.clone()),
            shared_bridge,
        }
    }

    /// Launch a fresh `CloakBrowser` (`PlaywrightBridge`).
    async fn launch_cloakbrowser() -> Result<Self, ToolError> {
        let bridge = crate::PlaywrightBridge::new()
            .await
            .map_err(|error| ToolError::new(error.to_string()))?;
        let shared_bridge: SharedBridge = std::sync::Arc::new(Mutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        ));
        Ok(Self {
            browser: BrowserContext::new(shared_bridge.clone()),
            shared_bridge,
        })
    }
}

impl CrawlerAgent {
    /// Drop the current browser context so the next tool call will spawn a
    /// fresh Playwright bridge. This is used by `/headed` and `/headless` to
    /// make the mode switch take effect immediately.
    pub fn reset_browser(&mut self) {
        self.browser = None;
        self.shared_bridge = None;
    }

    pub fn set_shared_bridge(&mut self, bridge: SharedBridge) {
        self.browser = None;
        self.shared_bridge = Some(bridge);
    }

    pub fn clear_shared_bridge(&mut self) {
        self.shared_bridge = None;
    }

    pub fn set_extension_mode(&mut self, active: bool) {
        self.extension_mode = active;
        if active {
            self.browser = None;
        }
    }

    pub async fn ensure_browser(&mut self) -> Result<(), ToolError> {
        if self.browser.is_some() {
            return Ok(());
        }

        if self.extension_mode {
            let bridge = self.shared_bridge.clone().ok_or_else(|| {
                ToolError::new(
                    "Extension mode active but browser extension not connected yet. \
                     Run /extension and wait for the browser to connect.",
                )
            })?;
            let session = BrowserSession::from_bridge(bridge);
            self.browser = Some(session.browser);
            self.shared_bridge = Some(session.shared_bridge);
            return Ok(());
        }

        if let Some(bridge) = self.shared_bridge.clone() {
            let session = BrowserSession::from_bridge(bridge);
            self.browser = Some(session.browser);
            self.shared_bridge = Some(session.shared_bridge);
            return Ok(());
        }

        let session = BrowserSession::launch_cloakbrowser().await?;
        self.browser = Some(session.browser);
        self.shared_bridge = Some(session.shared_bridge);
        Ok(())
    }

    pub async fn pause_browser_switch(&mut self) -> Result<(), ToolError> {
        if self.extension_mode {
            return Ok(());
        }

        let active_children = {
            let manager = self.agent_manager.lock().await;
            if manager.contains(&self.agent_id) {
                manager.get_active_children(&self.agent_id).len()
            } else {
                0
            }
        };

        if active_children > 0 {
            return Err(ToolError::new(
                "Cannot pause while sub-agents are running because they share the browser bridge.",
            ));
        }

        let is_headless = std::env::var("HEADLESS").map_or(true, |value| value != "false");
        if !is_headless {
            return Ok(());
        }

        let page_index = self.browser.as_ref().map_or(0, BrowserContext::page_index);
        let browser_state = self.export_browser_state().await;
        eprintln!(
            "Switching to headed mode. Note: JS runtime state (timers, WebSocket connections) will be lost."
        );

        std::env::set_var("HEADLESS", "false");
        self.reset_browser();
        self.ensure_browser().await?;
        if let Some(browser) = self.browser.as_mut() {
            browser.set_page_index(page_index);
        }

        if let Some(state) = browser_state {
            self.restore_browser_state(&state).await;
        }

        Ok(())
    }

    pub async fn export_browser_state(&self) -> Option<BrowserState> {
        let state_result = if let Some(browser) = self.browser.as_ref() {
            let mut browser = browser.clone();
            let mut bridge = browser.acquire_bridge().await.ok()?;
            bridge.export_cookies().await
        } else {
            let shared_bridge = self.shared_bridge.as_ref()?.clone();
            let mut bridge = shared_bridge.lock().await;
            bridge.export_cookies().await
        };

        match state_result {
            Ok(state) => Some(state),
            Err(error) => {
                eprintln!("Warning: failed to export browser state: {error}");
                None
            }
        }
    }

    pub async fn restore_browser_state(&mut self, state: &BrowserState) {
        if let Err(error) = self.ensure_browser().await {
            eprintln!("Warning: failed to initialize browser for state restore: {error}");
            return;
        }

        let Some(browser) = self.browser.as_mut() else {
            return;
        };

        let mut bridge = match browser.acquire_bridge().await {
            Ok(bridge) => bridge,
            Err(error) => {
                eprintln!("Warning: failed to acquire browser after headed switch: {error}");
                return;
            }
        };

        if let Err(error) = bridge.import_cookies_only(state).await {
            eprintln!("Warning: failed to import cookies after headed switch: {error}");
        }

        let navigated = if !state.url.is_empty() && state.url != "about:blank" {
            match bridge.navigate(&state.url).await {
                Ok(_) => {
                    if let Err(error) = bridge.import_local_storage(state).await {
                        eprintln!(
                            "Warning: failed to import localStorage after headed switch: {error}"
                        );
                    }
                    true
                }
                Err(error) => {
                    eprintln!(
                        "Warning: failed to navigate to saved URL after headed switch: {error}"
                    );
                    false
                }
            }
        } else {
            false
        };

        drop(bridge);
        if navigated {
            browser.set_navigated_url(&state.url, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, OnceLock};

    use tokio::sync::Mutex;

    use super::*;
    use crate::registry::ToolRegistry;

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    async fn test_bridge() -> SharedBridge {
        Arc::new(Mutex::new(Box::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for lifecycle test"),
        ) as Box<dyn BrowserBackend + Send>))
    }

    #[tokio::test]
    async fn test_browser_lazy_init() {
        let shared_bridge = test_bridge().await;
        let mut agent =
            CrawlerAgent::new_for_testing(ToolRegistry::new()).with_agent_id("root".to_string());
        agent.shared_bridge = Some(shared_bridge.clone());

        agent
            .ensure_browser()
            .await
            .expect("ensure_browser should initialize lazily");

        let browser = agent.browser.as_ref().expect("browser should exist");
        assert_eq!(browser.page_index(), 0);
        assert!(Arc::ptr_eq(browser.bridge(), &shared_bridge));
    }

    #[tokio::test]
    async fn test_browser_reset_clears_state() {
        let shared_bridge = test_bridge().await;
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new());
        agent.shared_bridge = Some(shared_bridge);
        agent
            .ensure_browser()
            .await
            .expect("ensure_browser should succeed before reset");

        agent.reset_browser();

        assert!(agent.browser.is_none());
        assert!(agent.shared_bridge.is_none());
    }

    #[tokio::test]
    async fn test_lifecycle_state_transitions() {
        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "true");
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new());

        agent
            .ensure_browser()
            .await
            .expect("first initialization should succeed");
        let first_bridge = agent.shared_bridge.clone().expect("bridge should exist");

        agent
            .ensure_browser()
            .await
            .expect("second initialization should reuse existing browser");
        let reused_bridge = agent
            .shared_bridge
            .clone()
            .expect("bridge should still exist");
        assert!(Arc::ptr_eq(&first_bridge, &reused_bridge));

        agent.reset_browser();
        assert!(agent.browser.is_none());
        assert!(agent.shared_bridge.is_none());

        agent
            .ensure_browser()
            .await
            .expect("reinitialization after reset should succeed");
        let second_bridge = agent
            .shared_bridge
            .clone()
            .expect("bridge should be recreated");
        assert!(!Arc::ptr_eq(&first_bridge, &second_bridge));
    }

    #[tokio::test]
    async fn test_ensure_browser_creates_bridge_from_scratch() {
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new());
        assert!(agent.shared_bridge.is_none());
        assert!(agent.browser.is_none());

        agent
            .ensure_browser()
            .await
            .expect("should create browser from scratch");

        assert!(agent.shared_bridge.is_some());
        assert!(agent.browser.is_some());
        let browser = agent.browser.as_ref().unwrap();
        assert_eq!(browser.page_index(), 0);
    }

    #[tokio::test]
    async fn test_double_reset_is_idempotent() {
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new());
        agent.shared_bridge = Some(test_bridge().await);
        agent
            .ensure_browser()
            .await
            .expect("initial ensure should succeed");

        agent.reset_browser();
        agent.reset_browser();

        assert!(agent.browser.is_none());
        assert!(agent.shared_bridge.is_none());
    }

    #[tokio::test]
    async fn ensure_browser_routes_through_extension_bridge_not_playwright() {
        // Create an ExtensionBridge backed by a channel — no CloakBrowser subprocess.
        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(10);
        let (_watch_tx, connected) = tokio::sync::watch::channel(true);
        let bridge = crate::ExtensionBridge::new(command_tx, connected);
        let shared: SharedBridge = Arc::new(Mutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        ));

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new())
            .with_agent_id("ext-test".to_string());
        agent.set_shared_bridge(shared.clone());

        // ensure_browser should reuse the extension bridge, not spawn PlaywrightBridge.
        agent
            .ensure_browser()
            .await
            .expect("should initialize with extension bridge");

        let browser = agent
            .browser
            .as_ref()
            .expect("browser should be initialized");
        assert!(
            Arc::ptr_eq(browser.bridge(), &shared),
            "browser should use the extension bridge, not spawn a new PlaywrightBridge"
        );
    }

    #[tokio::test]
    async fn tool_execution_routes_commands_through_extension_bridge() {
        use crate::ws_server::BridgeResponse;
        use acrawl_core::ToolExecutor;
        use serde_json::json;

        // Create ExtensionBridge with a channel so we can observe commands.
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(10);
        let (_watch_tx, connected) = tokio::sync::watch::channel(true);
        let bridge = crate::ExtensionBridge::new(command_tx, connected);
        let shared: SharedBridge = Arc::new(Mutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        ));

        let registry = crate::registry::ToolRegistry::new_with_core_tools();
        let mut agent =
            CrawlerAgent::new_for_testing(registry).with_agent_id("ext-tool-test".to_string());
        agent.set_shared_bridge(shared);

        // Execute click — routes entirely through extension bridge, never CloakBrowser.
        let handle =
            tokio::spawn(
                async move { agent.execute("click", r##"{"selector": "#submit"}"##).await },
            );

        // acquire_bridge() calls switch_tab first
        let (cmd, resp_tx) = command_rx
            .recv()
            .await
            .expect("extension should receive switch_tab");
        assert_eq!(cmd.action, "switch_tab");
        resp_tx
            .send(BridgeResponse {
                id: cmd.id,
                ok: true,
                result: Some(json!({"url": "about:blank", "title": ""})),
                error: None,
            })
            .unwrap();

        // Then click
        let (cmd, resp_tx) = command_rx
            .recv()
            .await
            .expect("extension should receive click");
        assert_eq!(cmd.action, "click");
        resp_tx
            .send(BridgeResponse {
                id: cmd.id,
                ok: true,
                result: None,
                error: None,
            })
            .unwrap();

        // post_action_page_state: acquire_bridge (switch_tab) + page_map
        let (cmd, resp_tx) = command_rx
            .recv()
            .await
            .expect("extension should receive second switch_tab");
        assert_eq!(cmd.action, "switch_tab");
        resp_tx
            .send(BridgeResponse {
                id: cmd.id,
                ok: true,
                result: Some(json!({"url": "about:blank", "title": ""})),
                error: None,
            })
            .unwrap();

        let (cmd, resp_tx) = command_rx
            .recv()
            .await
            .expect("extension should receive page_map");
        assert_eq!(cmd.action, "page_map");
        resp_tx
            .send(BridgeResponse {
                id: cmd.id,
                ok: true,
                result: Some(json!({
                    "headings": [],
                    "landmarks": [],
                    "forms": [],
                    "links": [],
                    "interactive": {},
                    "meta": {"url": "https://test.com", "title": "Test Page", "description": ""}
                })),
                error: None,
            })
            .unwrap();

        let result = handle.await.expect("task should complete");
        let output = result.expect("click should succeed through extension bridge");
        assert!(
            output.text.contains("Clicked element: #submit"),
            "unexpected output: {output:?}"
        );
    }

    #[tokio::test]
    async fn set_shared_bridge_overrides_existing_playwright_browser() {
        // Simulate: tool fires before extension connects → creates PlaywrightBridge.
        // Then extension connects → set_shared_bridge should override.
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new())
            .with_agent_id("race-test".to_string());
        agent.shared_bridge = Some(test_bridge().await);
        agent
            .ensure_browser()
            .await
            .expect("initial browser with PlaywrightBridge");

        let old_bridge = agent.shared_bridge.clone().unwrap();

        // Extension connects → set_shared_bridge clears browser, sets new bridge.
        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(10);
        let (_watch_tx, connected) = tokio::sync::watch::channel(true);
        let ext_bridge = crate::ExtensionBridge::new(command_tx, connected);
        let ext_shared: SharedBridge = Arc::new(Mutex::new(
            Box::new(ext_bridge) as Box<dyn BrowserBackend + Send>
        ));
        agent.set_shared_bridge(ext_shared.clone());

        assert!(
            agent.browser.is_none(),
            "set_shared_bridge should clear browser"
        );
        assert!(!Arc::ptr_eq(&old_bridge, &ext_shared));

        // Next ensure_browser should use extension, not the old PlaywrightBridge.
        agent
            .ensure_browser()
            .await
            .expect("should reinitialize with extension bridge");
        let browser = agent.browser.as_ref().unwrap();
        assert!(
            Arc::ptr_eq(browser.bridge(), &ext_shared),
            "after set_shared_bridge, browser should use extension bridge"
        );
    }

    #[tokio::test]
    async fn test_browser_session_reuses_bridge() {
        let shared_bridge = test_bridge().await;
        let session = BrowserSession::from_bridge(shared_bridge.clone());

        assert!(Arc::ptr_eq(&session.shared_bridge, &shared_bridge));
        assert_eq!(session.browser.page_index(), 0);
    }

    #[tokio::test]
    async fn extension_mode_errors_immediately_without_bridge() {
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new());
        agent.set_extension_mode(true);

        let err = agent
            .ensure_browser()
            .await
            .expect_err("should fail immediately in extension mode without bridge");
        assert!(
            err.to_string().contains("Extension mode active"),
            "unexpected error: {err}"
        );
    }
}
