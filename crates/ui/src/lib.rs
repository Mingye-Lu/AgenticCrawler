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
pub mod script_run;
pub mod session_mgr;

pub use auth::configure::{run_auth_configure, AuthFlags};
pub use auth::status::{run_auth_list, run_auth_status};
pub use config_runner::{run_config, ConfigAction};
pub use script_run::{run_script_cli, RunFormat, RunOptions};

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

    /// The one lock for tests that mutate process-wide environment (`ACRAWL_CONFIG_HOME`).
    /// Every such test in this crate must take it: separate per-module locks do not exclude
    /// each other, so two modules rewriting the same variable race under `cargo test`.
    pub fn test_env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
