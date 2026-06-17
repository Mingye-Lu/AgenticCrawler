mod browser_backend;
pub mod context;
pub mod extension;
pub mod fetch;
pub mod markdown;
pub mod observation;
pub mod playwright;
pub mod prune;
pub mod ref_map;
pub mod testing;
pub mod ws_server;

pub use browser_backend::{
    BrowserBackend, CookieInfo, CoverageData, FileCoverage, InterceptAction, InterceptRule,
    MockResponse, ScreenshotOptions, StorageEntry, StorageType,
};
pub use context::BrowserContext;
pub use extension::ExtensionBridge;
pub use fetch::{FetchError, FetchRouter, FetchedPage};
pub use observation::{
    ConsoleMessageEvent, ConsoleMessageType, NetworkRequestEvent, ObservationBuffer,
    ObservationEvent, RequestState, SeqCounter, WebSocketFrameEvent,
};
pub use playwright::{BridgeError, BrowserState, PageInfo, PlaywrightBridge, SharedBridge};
pub use ref_map::{parse_ref, RefEntry, RefMap};
pub use testing::NopBridge;
pub use ws_server::{
    generate_bridge_token, BridgeCommand, BridgeResponse, WsBridgeError, WsBridgeServer,
};
