mod browser_backend;
pub mod context;
pub mod extension;
pub mod fetch;
pub mod markdown;
pub mod playwright;
pub mod prune;
pub mod ref_map;
pub mod testing;
pub mod ws_server;

pub use browser_backend::{BrowserBackend, ScreenshotOptions};
pub use context::BrowserContext;
pub use extension::ExtensionBridge;
pub use fetch::{FetchError, FetchRouter, FetchedPage};
pub use playwright::{BridgeError, BrowserState, PageInfo, PlaywrightBridge, SharedBridge};
pub use ref_map::{parse_ref, RefEntry, RefMap};
pub use testing::NopBridge;
pub use ws_server::{
    generate_bridge_token, BridgeCommand, BridgeResponse, WsBridgeError, WsBridgeServer,
};
