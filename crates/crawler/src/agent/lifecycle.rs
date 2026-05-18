use runtime::ToolError;
use tokio::sync::Mutex;

use super::CrawlerAgent;
use crate::{BrowserBackend, BrowserContext, BrowserState, SharedBridge};

#[derive(Clone)]
pub(crate) struct BrowserSession {
    pub(crate) browser: BrowserContext,
    pub(crate) shared_bridge: SharedBridge,
}

impl BrowserSession {
    async fn initialize(shared_bridge: Option<SharedBridge>) -> Result<Self, ToolError> {
        let shared_bridge = if let Some(shared_bridge) = shared_bridge {
            shared_bridge
        } else {
            let bridge = crate::PlaywrightBridge::new()
                .await
                .map_err(|error| ToolError::new(error.to_string()))?;
            std::sync::Arc::new(Mutex::new(
                Box::new(bridge) as Box<dyn BrowserBackend + Send>
            ))
        };

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

    pub async fn ensure_browser(&mut self) -> Result<(), ToolError> {
        if self.browser.is_some() {
            return Ok(());
        }

        let session = BrowserSession::initialize(self.shared_bridge.clone()).await?;
        self.browser = Some(session.browser);
        self.shared_bridge = Some(session.shared_bridge);
        Ok(())
    }

    pub async fn pause_browser_switch(&mut self) -> Result<(), ToolError> {
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

    pub async fn export_browser_state_any(&self) -> Option<BrowserState> {
        self.export_browser_state().await
    }

    async fn export_browser_state(&self) -> Option<BrowserState> {
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
    use crate::tool_registry::ToolRegistry;

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
    async fn test_browser_session_initialize_reuses_bridge() {
        let shared_bridge = test_bridge().await;
        let session = BrowserSession::initialize(Some(shared_bridge.clone()))
            .await
            .expect("should initialize with existing bridge");

        assert!(Arc::ptr_eq(&session.shared_bridge, &shared_bridge));
        assert_eq!(session.browser.page_index(), 0);
    }
}
