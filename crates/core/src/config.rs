use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

/// OAuth provider configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub callback_port: Option<u16>,
    pub manual_redirect_url: Option<String>,
    pub scopes: Vec<String>,
}

static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_tui_active(active: bool) {
    TUI_ACTIVE.store(active, Ordering::Release);
}

#[must_use]
pub fn is_tui_active() -> bool {
    TUI_ACTIVE.load(Ordering::Acquire)
}

#[must_use]
pub fn child_stderr() -> Stdio {
    if is_tui_active() {
        Stdio::null()
    } else {
        Stdio::inherit()
    }
}

/// Platform-specific home directory.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Get the configuration home directory.
/// Reads `ACRAWL_CONFIG_HOME` env var, falls back to ~/.acrawl/
#[must_use]
pub fn config_home_dir() -> PathBuf {
    if let Ok(custom_home) = std::env::var("ACRAWL_CONFIG_HOME") {
        PathBuf::from(custom_home)
    } else {
        home_dir().join(".acrawl")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_tui_active_toggles_flag() {
        set_tui_active(true);
        assert!(is_tui_active());
        set_tui_active(false);
        assert!(!is_tui_active());
    }

    #[test]
    fn child_stderr_returns_inherit_when_tui_inactive() {
        set_tui_active(false);
        let stdio = child_stderr();
        let result = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "echo test"][..]
            } else {
                &[][..]
            })
            .stderr(stdio)
            .stdout(std::process::Stdio::null())
            .status();
        assert!(result.is_ok());
    }

    #[test]
    fn child_stderr_returns_null_when_tui_active() {
        set_tui_active(true);
        let stdio = child_stderr();
        let result = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
            .args(if cfg!(windows) {
                &["/C", "echo error_output 1>&2"][..]
            } else {
                &["-c", "echo error_output >&2"][..]
            })
            .stderr(stdio)
            .stdout(std::process::Stdio::null())
            .output();
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        set_tui_active(false);
    }
}
