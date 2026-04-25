use runtime::ToolError;
use tokio::sync::Mutex;

use super::CrawlerAgent;
use crate::{BrowserContext, SharedBridge};

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
            std::sync::Arc::new(Mutex::new(bridge))
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

    pub(super) async fn ensure_browser(&mut self) -> Result<(), ToolError> {
        if self.browser.is_some() {
            return Ok(());
        }

        let session = BrowserSession::initialize(self.shared_bridge.clone()).await?;
        self.browser = Some(session.browser);
        self.shared_bridge = Some(session.shared_bridge);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::*;
    use crate::tool_registry::ToolRegistry;

    async fn test_bridge() -> SharedBridge {
        Arc::new(Mutex::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for lifecycle test"),
        ))
    }

    #[tokio::test]
    async fn test_browser_lazy_init() {
        let shared_bridge = test_bridge().await;
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new()).with_agent_id("root".to_string());
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
        let reused_bridge = agent.shared_bridge.clone().expect("bridge should still exist");
        assert!(Arc::ptr_eq(&first_bridge, &reused_bridge));

        agent.reset_browser();
        assert!(agent.browser.is_none());
        assert!(agent.shared_bridge.is_none());

        agent
            .ensure_browser()
            .await
            .expect("reinitialization after reset should succeed");
        let second_bridge = agent.shared_bridge.clone().expect("bridge should be recreated");
        assert!(!Arc::ptr_eq(&first_bridge, &second_bridge));
    }
}
