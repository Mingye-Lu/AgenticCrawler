use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

/// Settings loaded from settings.json configuration file.
/// All fields are optional with serde defaults to support partial JSON files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// Run browser in headless mode (default: true)
    #[serde(default)]
    pub headless: Option<bool>,

    /// Maximum number of agent loop iterations (default: 50)
    #[serde(default)]
    pub max_steps: Option<u32>,

    /// Last used model in provider/model format (e.g. "anthropic/claude-sonnet-4-6")
    #[serde(default)]
    pub model: Option<String>,

    /// Reasoning effort level for reasoning models (e.g. "high", "medium", "low")
    #[serde(default)]
    pub reasoning_effort: Option<String>,

    /// Directory for saved files (default: "workspace")
    #[serde(default)]
    pub workspace_dir: Option<String>,

    /// Use classic REPL instead of TUI (default: false)
    #[serde(default)]
    pub classic_repl: Option<bool>,

    /// Auto-compact input tokens threshold (default: 200000)
    #[serde(default)]
    pub auto_compact_input_tokens: Option<u64>,

    /// Max concurrent subagents per parent (default: 5)
    #[serde(default)]
    pub max_concurrent_per_parent: Option<u32>,

    /// Max fork depth (default: 3)
    #[serde(default)]
    pub max_fork_depth: Option<u32>,

    /// Max total agents across all parents (default: 10)
    #[serde(default)]
    pub max_total_agents: Option<u32>,

    /// Max steps for forked child agents (default: 15)
    #[serde(default)]
    pub fork_child_max_steps: Option<u32>,

    /// Timeout in seconds for wait_for_subagents (default: 60)
    #[serde(default)]
    pub fork_wait_timeout_secs: Option<u32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            headless: Some(true),
            max_steps: Some(50),
            model: None,
            reasoning_effort: None,
            workspace_dir: Some("workspace".to_string()),
            classic_repl: Some(false),
            auto_compact_input_tokens: Some(200_000),
            max_concurrent_per_parent: Some(5),
            max_fork_depth: Some(3),
            max_total_agents: Some(10),
            fork_child_max_steps: Some(15),
            fork_wait_timeout_secs: Some(60),
        }
    }
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

/// Get the settings file path: `config_home_dir()/settings.json`
#[must_use]
pub fn settings_file_path() -> PathBuf {
    config_home_dir().join("settings.json")
}

/// Load settings from settings.json.
/// Returns `Settings::default()` if file is missing or invalid JSON.
#[must_use]
pub fn load_settings() -> Settings {
    let path = settings_file_path();

    match fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Settings>(&content) {
            Ok(settings) => settings,
            Err(e) => {
                eprintln!("Warning: Failed to parse settings.json: {e}");
                Settings::default()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // File doesn't exist, return defaults
            Settings::default()
        }
        Err(e) => {
            eprintln!("Warning: Failed to read settings.json: {e}");
            Settings::default()
        }
    }
}

/// Save settings to settings.json.
/// Creates the config directory if it doesn't exist.
pub fn save_settings(settings: &Settings) -> io::Result<()> {
    let path = settings_file_path();
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));

    fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, json)?;

    Ok(())
}

/// Update a single setting by loading current settings, applying the mutation, and saving.
/// This avoids clobbering other settings that may have been changed externally.
pub fn update_settings(mutate: impl FnOnce(&mut Settings)) -> io::Result<()> {
    let mut settings = load_settings();
    mutate(&mut settings);
    save_settings(&settings)
}

/// Get headless setting, with default fallback.
#[must_use]
pub fn settings_get_headless(s: &Settings) -> bool {
    s.headless.unwrap_or(true)
}

/// Get `max_steps` setting, with default fallback.
#[must_use]
pub fn settings_get_max_steps(s: &Settings) -> u32 {
    s.max_steps.unwrap_or(50)
}

/// Get `workspace_dir` setting, with default fallback.
#[must_use]
pub fn settings_get_workspace_dir(s: &Settings) -> &str {
    s.workspace_dir.as_deref().unwrap_or("workspace")
}

/// Get `auto_compact_input_tokens` setting, with default fallback.
#[must_use]
pub fn settings_get_auto_compact_tokens(s: &Settings) -> u64 {
    s.auto_compact_input_tokens.unwrap_or(200_000)
}

/// Get `max_concurrent_per_parent` setting, with default fallback.
#[must_use]
pub fn settings_get_max_concurrent_per_parent(s: &Settings) -> u32 {
    s.max_concurrent_per_parent.unwrap_or(5)
}

/// Get `max_fork_depth` setting, with default fallback.
#[must_use]
pub fn settings_get_max_fork_depth(s: &Settings) -> u32 {
    s.max_fork_depth.unwrap_or(3)
}

/// Get `max_total_agents` setting, with default fallback.
#[must_use]
pub fn settings_get_max_total_agents(s: &Settings) -> u32 {
    s.max_total_agents.unwrap_or(10)
}

/// Get `fork_child_max_steps` setting, with default fallback.
#[must_use]
pub fn settings_get_fork_child_max_steps(s: &Settings) -> u32 {
    s.fork_child_max_steps.unwrap_or(15)
}

/// Get `fork_wait_timeout_secs` setting, with default fallback.
#[must_use]
pub fn settings_get_fork_wait_timeout_secs(s: &Settings) -> u32 {
    s.fork_wait_timeout_secs.unwrap_or(60)
}

/// Helper to get home directory.
/// On Windows, tries USERPROFILE then HOMEPATH.
/// On Unix, tries HOME.
fn home_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(home) = std::env::var("USERPROFILE") {
            return PathBuf::from(home);
        }
        if let Ok(home) = std::env::var("HOMEPATH") {
            return PathBuf::from(home);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }

    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn setup_temp_dir() -> PathBuf {
        let temp_dir =
            std::env::temp_dir().join(format!("acrawl_settings_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        temp_dir
    }

    fn cleanup_temp_dir(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn test_load_settings_missing_file_returns_defaults() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        let settings = load_settings();

        assert_eq!(settings.headless, Some(true));
        assert_eq!(settings.max_steps, Some(50));
        assert_eq!(settings.workspace_dir, Some("workspace".to_string()));
        assert_eq!(settings.classic_repl, Some(false));
        assert_eq!(settings.auto_compact_input_tokens, Some(200_000));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_load_settings_partial_json() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        // Write partial JSON
        let settings_path = temp_dir.join("settings.json");
        fs::write(&settings_path, r#"{"max_steps": 100}"#).expect("Failed to write test settings");

        let settings = load_settings();

        assert_eq!(settings.max_steps, Some(100));
        assert_eq!(settings.headless, None);
        assert_eq!(settings.workspace_dir, None);

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_settings_round_trip() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let original = Settings {
            headless: Some(false),
            max_steps: Some(100),
            model: Some("anthropic/claude-sonnet-4-6".to_string()),
            reasoning_effort: Some("high".to_string()),
            workspace_dir: Some("custom_workspace".to_string()),
            classic_repl: Some(true),
            auto_compact_input_tokens: Some(500_000),
            max_concurrent_per_parent: Some(8),
            max_fork_depth: Some(5),
            max_total_agents: Some(20),
            fork_child_max_steps: Some(25),
            fork_wait_timeout_secs: Some(120),
        };

        save_settings(&original).expect("Failed to save settings");
        let loaded = load_settings();

        assert_eq!(loaded, original);

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_config_home_dir_with_env_var() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        let custom_path = temp_dir.join("custom_config");

        std::env::set_var("ACRAWL_CONFIG_HOME", &custom_path);
        let home = config_home_dir();

        assert_eq!(home, custom_path);

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_config_home_dir_without_env_var() {
        let _lock = test_env_lock();
        std::env::remove_var("ACRAWL_CONFIG_HOME");

        let home = config_home_dir();
        let expected = home_dir().join(".acrawl");

        assert_eq!(home, expected);
    }

    #[test]
    fn test_settings_get_max_steps_with_none() {
        let settings = Settings {
            max_steps: None,
            ..Default::default()
        };

        assert_eq!(settings_get_max_steps(&settings), 50);
    }

    #[test]
    fn test_settings_get_max_steps_with_value() {
        let settings = Settings {
            max_steps: Some(100),
            ..Default::default()
        };

        assert_eq!(settings_get_max_steps(&settings), 100);
    }

    #[test]
    fn test_settings_get_headless() {
        let settings_true = Settings {
            headless: Some(true),
            ..Default::default()
        };
        assert!(settings_get_headless(&settings_true));

        let settings_false = Settings {
            headless: Some(false),
            ..Default::default()
        };
        assert!(!settings_get_headless(&settings_false));

        let settings_none = Settings {
            headless: None,
            ..Default::default()
        };
        assert!(settings_get_headless(&settings_none)); // defaults to true
    }

    #[test]
    fn test_settings_get_workspace_dir() {
        let settings_custom = Settings {
            workspace_dir: Some("my_workspace".to_string()),
            ..Default::default()
        };
        assert_eq!(settings_get_workspace_dir(&settings_custom), "my_workspace");

        let settings_none = Settings {
            workspace_dir: None,
            ..Default::default()
        };
        assert_eq!(settings_get_workspace_dir(&settings_none), "workspace");
    }

    #[test]
    fn test_settings_get_auto_compact_tokens() {
        let settings_custom = Settings {
            auto_compact_input_tokens: Some(500_000),
            ..Default::default()
        };
        assert_eq!(settings_get_auto_compact_tokens(&settings_custom), 500_000);

        let settings_none = Settings {
            auto_compact_input_tokens: None,
            ..Default::default()
        };
        assert_eq!(settings_get_auto_compact_tokens(&settings_none), 200_000);
    }

    #[test]
    fn test_invalid_json_returns_defaults() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        // Write invalid JSON
        let settings_path = temp_dir.join("settings.json");
        fs::write(&settings_path, r"{ invalid json }").expect("Failed to write test settings");

        let settings = load_settings();

        // Should return defaults on parse error
        assert_eq!(settings.headless, Some(true));
        assert_eq!(settings.max_steps, Some(50));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_update_settings_updates_field_without_clobbering_others() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        save_settings(&Settings {
            headless: Some(false),
            max_steps: Some(88),
            model: Some("anthropic/claude-sonnet-4-6".to_string()),
            reasoning_effort: Some("medium".to_string()),
            workspace_dir: Some("custom_workspace".to_string()),
            classic_repl: Some(true),
            auto_compact_input_tokens: Some(123_456),
            max_concurrent_per_parent: Some(7),
            max_fork_depth: Some(4),
            max_total_agents: Some(15),
            fork_child_max_steps: Some(20),
            fork_wait_timeout_secs: Some(90),
        })
        .expect("save settings");

        update_settings(|settings| {
            settings.model = Some("openai/o4-mini".to_string());
        })
        .expect("update settings");

        let loaded = load_settings();
        assert_eq!(loaded.headless, Some(false));
        assert_eq!(loaded.max_steps, Some(88));
        assert_eq!(loaded.model, Some("openai/o4-mini".to_string()));
        assert_eq!(loaded.reasoning_effort, Some("medium".to_string()));
        assert_eq!(loaded.workspace_dir, Some("custom_workspace".to_string()));
        assert_eq!(loaded.classic_repl, Some(true));
        assert_eq!(loaded.auto_compact_input_tokens, Some(123_456));
        assert_eq!(loaded.max_concurrent_per_parent, Some(7));
        assert_eq!(loaded.max_fork_depth, Some(4));
        assert_eq!(loaded.max_total_agents, Some(15));
        assert_eq!(loaded.fork_child_max_steps, Some(20));
        assert_eq!(loaded.fork_wait_timeout_secs, Some(90));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_fork_settings_defaults() {
        let settings = Settings::default();
        assert_eq!(settings.max_concurrent_per_parent, Some(5));
        assert_eq!(settings.max_fork_depth, Some(3));
        assert_eq!(settings.max_total_agents, Some(10));
        assert_eq!(settings.fork_child_max_steps, Some(15));
        assert_eq!(settings.fork_wait_timeout_secs, Some(60));
    }

    #[test]
    fn test_fork_settings_round_trip() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let original = Settings {
            headless: Some(true),
            max_steps: Some(50),
            model: None,
            reasoning_effort: None,
            workspace_dir: Some("workspace".to_string()),
            classic_repl: Some(false),
            auto_compact_input_tokens: Some(200_000),
            max_concurrent_per_parent: Some(8),
            max_fork_depth: Some(4),
            max_total_agents: Some(16),
            fork_child_max_steps: Some(20),
            fork_wait_timeout_secs: Some(75),
        };

        save_settings(&original).expect("Failed to save settings");
        let loaded = load_settings();

        assert_eq!(loaded.max_concurrent_per_parent, Some(8));
        assert_eq!(loaded.max_fork_depth, Some(4));
        assert_eq!(loaded.max_total_agents, Some(16));
        assert_eq!(loaded.fork_child_max_steps, Some(20));
        assert_eq!(loaded.fork_wait_timeout_secs, Some(75));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_fork_settings_partial_json() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();

        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let settings_path = temp_dir.join("settings.json");
        fs::write(
            &settings_path,
            r#"{"max_fork_depth": 6, "fork_wait_timeout_secs": 120}"#,
        )
        .expect("Failed to write test settings");

        let settings = load_settings();

        assert_eq!(settings.max_fork_depth, Some(6));
        assert_eq!(settings.fork_wait_timeout_secs, Some(120));
        assert_eq!(settings.max_concurrent_per_parent, None);
        assert_eq!(settings.max_total_agents, None);
        assert_eq!(settings.fork_child_max_steps, None);

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_fork_settings_getters_with_none() {
        let settings = Settings {
            headless: None,
            max_steps: None,
            model: None,
            reasoning_effort: None,
            workspace_dir: None,
            classic_repl: None,
            auto_compact_input_tokens: None,
            max_concurrent_per_parent: None,
            max_fork_depth: None,
            max_total_agents: None,
            fork_child_max_steps: None,
            fork_wait_timeout_secs: None,
        };

        assert_eq!(settings_get_max_concurrent_per_parent(&settings), 5);
        assert_eq!(settings_get_max_fork_depth(&settings), 3);
        assert_eq!(settings_get_max_total_agents(&settings), 10);
        assert_eq!(settings_get_fork_child_max_steps(&settings), 15);
        assert_eq!(settings_get_fork_wait_timeout_secs(&settings), 60);
    }
}
