// Transitional re-export shim — all logic now lives in the `agent` crate.
pub use acrawl_core::ToolSpec;
pub use agent::{
    build_system_prompt, mvp_tool_specs,
    ChildBlock, ChildControlRegistry, ChildEvent, ChildEventKind, ChildEventSender,
    ChildLifecycle, ChildSnapshot, ChildSnapshotRegistry, ClaimConflict, ClaimGuard,
    CrawlState, OutputError, OutputFormat, SharedApiClient, ToolEffect,
    ToolExecutionError, UrlClaimRegistry, write_output,
};
pub use browser::{
    markdown, ws_server, BridgeCommand, BridgeError, BridgeResponse, BrowserBackend,
    BrowserContext, BrowserState, ExtensionBridge, FetchError, FetchRouter, FetchedPage,
    PageInfo, PlaywrightBridge, SharedBridge, WsBridgeError, WsBridgeServer,
    generate_bridge_token,
};
