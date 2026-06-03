mod browser_backend;
pub mod context;
pub mod extension;
pub mod fetch;
pub mod markdown;
pub mod playwright;
pub mod ws_server;

pub use browser_backend::{BrowserBackend, ScreenshotOptions};
pub use context::BrowserContext;
pub use extension::ExtensionBridge;
pub use fetch::{FetchError, FetchRouter, FetchedPage};
pub use playwright::{BridgeError, BrowserState, PageInfo, PlaywrightBridge, SharedBridge};
pub use ws_server::{
    generate_bridge_token, BridgeCommand, BridgeResponse, WsBridgeError, WsBridgeServer,
};
