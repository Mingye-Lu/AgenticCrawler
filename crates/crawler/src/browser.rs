use crate::PlaywrightBridge;

#[derive(Debug)]
pub struct BrowserContext {
    bridge: PlaywrightBridge,
}

impl BrowserContext {
    #[must_use]
    pub fn new(bridge: PlaywrightBridge) -> Self {
        Self { bridge }
    }

    #[must_use]
    pub fn bridge(&self) -> &PlaywrightBridge {
        &self.bridge
    }

    pub fn bridge_mut(&mut self) -> &mut PlaywrightBridge {
        &mut self.bridge
    }

    #[must_use]
    pub fn into_bridge(self) -> PlaywrightBridge {
        self.bridge
    }
}
