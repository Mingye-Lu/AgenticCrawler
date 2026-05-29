use std::time::Duration;

mod backend_impl;
mod bridge;
mod bridge_script;
mod types;

const DEFAULT_LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_mins(1);
const CLOSE_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

pub use bridge::{PlaywrightBridge, SharedBridge};
pub use types::{BridgeError, BrowserState, PageInfo};
