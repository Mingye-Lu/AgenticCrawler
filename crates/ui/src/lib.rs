use std::sync::OnceLock;

pub static TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliOutputFormat {
    Text,
    Json,
}

impl CliOutputFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unsupported value for --output-format: {other} (expected text or json)"
            )),
        }
    }
}

pub mod app;
pub mod auth;
pub mod config_runner;
pub mod display_width;
pub mod error;
pub mod events;
pub mod output_sink;
pub mod session_mgr;

pub use auth::configure::{run_auth_configure, AuthFlags};
