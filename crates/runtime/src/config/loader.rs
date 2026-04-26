use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::json::JsonValue;

use super::{
    deep_merge_objects, expect_object, parse_mcp_server_config, parse_optional_hooks_config,
    parse_optional_oauth_config, parse_optional_sandbox_config, read_optional_json_object,
    ConfigEntry, ConfigError, ConfigSource, McpConfigCollection, OAuthConfig, RuntimeFeatureConfig,
    RuntimeHookConfig, ScopedMcpServerConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    merged: BTreeMap<String, JsonValue>,
    loaded_entries: Vec<ConfigEntry>,
    feature_config: RuntimeFeatureConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLoader {
    cwd: PathBuf,
    config_home: PathBuf,
}

impl ConfigLoader {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>, config_home: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            config_home: config_home.into(),
        }
    }

    #[must_use]
    pub fn default_for(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        let config_home = std::env::var_os("ACRAWL_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|home| PathBuf::from(home).join(".acrawl"))
            })
            .unwrap_or_else(|| PathBuf::from(".acrawl"));
        Self { cwd, config_home }
    }

    #[must_use]
    pub fn discover(&self) -> Vec<ConfigEntry> {
        let user_legacy_path = self.config_home.parent().map_or_else(
            || PathBuf::from(".acrawl.json"),
            |parent| parent.join(".acrawl.json"),
        );
        vec![
            ConfigEntry {
                source: ConfigSource::User,
                path: user_legacy_path,
            },
            ConfigEntry {
                source: ConfigSource::User,
                path: self.config_home.join("settings.json"),
            },
            ConfigEntry {
                source: ConfigSource::Project,
                path: self.cwd.join(".acrawl.json"),
            },
            ConfigEntry {
                source: ConfigSource::Project,
                path: self.cwd.join(".acrawl").join("settings.json"),
            },
            ConfigEntry {
                source: ConfigSource::Local,
                path: self.cwd.join(".acrawl").join("settings.local.json"),
            },
        ]
    }

    pub fn load(&self) -> Result<RuntimeConfig, ConfigError> {
        let mut merged = BTreeMap::new();
        let mut loaded_entries = Vec::new();
        let mut mcp_servers = BTreeMap::new();

        for entry in self.discover() {
            let Some(value) = read_optional_json_object(&entry.path)? else {
                continue;
            };
            merge_mcp_servers(&mut mcp_servers, entry.source, &value, &entry.path)?;
            deep_merge_objects(&mut merged, &value);
            loaded_entries.push(entry);
        }

        let merged_value = JsonValue::Object(merged.clone());

        let feature_config = RuntimeFeatureConfig {
            hooks: parse_optional_hooks_config(&merged_value)?,
            mcp: McpConfigCollection {
                servers: mcp_servers,
            },
            oauth: parse_optional_oauth_config(&merged_value, "merged settings.oauth")?,
            model: parse_optional_model(&merged_value),
            sandbox: parse_optional_sandbox_config(&merged_value)?,
        };

        Ok(RuntimeConfig {
            merged,
            loaded_entries,
            feature_config,
        })
    }
}

impl RuntimeConfig {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            merged: BTreeMap::new(),
            loaded_entries: Vec::new(),
            feature_config: RuntimeFeatureConfig::default(),
        }
    }

    #[must_use]
    pub fn merged(&self) -> &BTreeMap<String, JsonValue> {
        &self.merged
    }

    #[must_use]
    pub fn loaded_entries(&self) -> &[ConfigEntry] {
        &self.loaded_entries
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.merged.get(key)
    }

    #[must_use]
    pub fn as_json(&self) -> JsonValue {
        JsonValue::Object(self.merged.clone())
    }

    #[must_use]
    pub fn feature_config(&self) -> &RuntimeFeatureConfig {
        &self.feature_config
    }

    #[must_use]
    pub fn mcp(&self) -> &McpConfigCollection {
        self.feature_config.mcp()
    }

    #[must_use]
    pub fn hooks(&self) -> &RuntimeHookConfig {
        self.feature_config.hooks()
    }

    #[must_use]
    pub fn oauth(&self) -> Option<&OAuthConfig> {
        self.feature_config.oauth()
    }

    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.feature_config.model()
    }

    #[must_use]
    pub fn sandbox(&self) -> &crate::sandbox::SandboxConfig {
        self.feature_config.sandbox()
    }
}

fn merge_mcp_servers(
    target: &mut BTreeMap<String, ScopedMcpServerConfig>,
    source: ConfigSource,
    root: &BTreeMap<String, JsonValue>,
    path: &Path,
) -> Result<(), ConfigError> {
    let Some(mcp_servers) = root.get("mcpServers") else {
        return Ok(());
    };
    let servers = expect_object(mcp_servers, &format!("{}: mcpServers", path.display()))?;
    for (name, value) in servers {
        let parsed = parse_mcp_server_config(
            name,
            value,
            &format!("{}: mcpServers.{name}", path.display()),
        )?;
        target.insert(
            name.clone(),
            ScopedMcpServerConfig {
                scope: source,
                config: parsed,
            },
        );
    }
    Ok(())
}

fn parse_optional_model(root: &JsonValue) -> Option<String> {
    root.as_object()
        .and_then(|object| object.get("model"))
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::config::{
        ConfigLoader, ConfigSource, McpServerConfig, McpTransport, RuntimeConfig,
        ACRAWL_SETTINGS_SCHEMA_NAME,
    };
    use crate::json::JsonValue;

    fn temp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("runtime-config-{nanos}"))
    }

    #[test]
    fn rejects_non_object_settings_files() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(&home).expect("home config dir");
        fs::create_dir_all(&cwd).expect("project dir");
        fs::write(home.join("settings.json"), "[]").expect("write bad settings");

        let error = ConfigLoader::new(&cwd, &home)
            .load()
            .expect_err("config should fail");
        assert!(error
            .to_string()
            .contains("top-level settings value must be a JSON object"));

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn loads_and_merges_config_files_by_precedence() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(cwd.join(".acrawl")).expect("project config dir");
        fs::create_dir_all(&home).expect("home config dir");

        fs::write(
            home.parent().expect("home parent").join(".acrawl.json"),
            r#"{"model":"haiku","env":{"A":"1"},"mcpServers":{"home":{"command":"uvx","args":["home"]}}}"#,
        )
        .expect("write user compat config");
        fs::write(
            home.join("settings.json"),
            r#"{"model":"sonnet","env":{"A2":"1"},"hooks":{"PreToolUse":["base"]},"permissions":{"defaultMode":"plan"}}"#,
        )
        .expect("write user settings");
        fs::write(
            cwd.join(".acrawl.json"),
            r#"{"model":"project-compat","env":{"B":"2"}}"#,
        )
        .expect("write project compat config");
        fs::write(
            cwd.join(".acrawl").join("settings.json"),
            r#"{"env":{"C":"3"},"hooks":{"PostToolUse":["project"]},"mcpServers":{"project":{"command":"uvx","args":["project"]}}}"#,
        )
        .expect("write project settings");
        fs::write(
            cwd.join(".acrawl").join("settings.local.json"),
            r#"{"model":"opus","permissionMode":"acceptEdits"}"#,
        )
        .expect("write local settings");

        let loaded = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("config should load");

        assert_eq!(ACRAWL_SETTINGS_SCHEMA_NAME, "SettingsSchema");
        assert_eq!(loaded.loaded_entries().len(), 5);
        assert_eq!(loaded.loaded_entries()[0].source, ConfigSource::User);
        assert_eq!(
            loaded.get("model"),
            Some(&JsonValue::String("opus".to_string()))
        );
        assert_eq!(loaded.model(), Some("opus"));
        assert_eq!(
            loaded
                .get("env")
                .and_then(JsonValue::as_object)
                .expect("env object")
                .len(),
            4
        );
        assert!(loaded
            .get("hooks")
            .and_then(JsonValue::as_object)
            .expect("hooks object")
            .contains_key("PreToolUse"));
        assert!(loaded
            .get("hooks")
            .and_then(JsonValue::as_object)
            .expect("hooks object")
            .contains_key("PostToolUse"));
        assert_eq!(loaded.hooks().pre_tool_use(), &["base".to_string()]);
        assert_eq!(loaded.hooks().post_tool_use(), &["project".to_string()]);
        assert!(loaded.mcp().get("home").is_some());
        assert!(loaded.mcp().get("project").is_some());

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn parses_typed_mcp_and_oauth_config() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(cwd.join(".acrawl")).expect("project config dir");
        fs::create_dir_all(&home).expect("home config dir");

        fs::write(
            home.join("settings.json"),
            r#"{
              "mcpServers": {
                "stdio-server": {
                  "command": "uvx",
                  "args": ["mcp-server"],
                  "env": {"TOKEN": "secret"}
                },
                "remote-server": {
                  "type": "http",
                  "url": "https://example.test/mcp",
                  "headers": {"Authorization": "Bearer token"},
                  "headersHelper": "helper.sh",
                  "oauth": {
                    "clientId": "mcp-client",
                    "callbackPort": 7777,
                    "authServerMetadataUrl": "https://issuer.test/.well-known/oauth-authorization-server",
                    "xaa": true
                  }
                }
              },
              "oauth": {
                "clientId": "runtime-client",
                "authorizeUrl": "https://console.test/oauth/authorize",
                "tokenUrl": "https://console.test/oauth/token",
                "callbackPort": 54545,
                "manualRedirectUrl": "https://console.test/oauth/callback",
                "scopes": ["org:read", "user:write"]
              }
            }"#,
        )
        .expect("write user settings");
        fs::write(
            cwd.join(".acrawl").join("settings.local.json"),
            r#"{
              "mcpServers": {
                "remote-server": {
                  "type": "ws",
                  "url": "wss://override.test/mcp",
                  "headers": {"X-Env": "local"}
                }
              }
            }"#,
        )
        .expect("write local settings");

        let loaded = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("config should load");

        let stdio_server = loaded
            .mcp()
            .get("stdio-server")
            .expect("stdio server should exist");
        assert_eq!(stdio_server.scope, ConfigSource::User);
        assert_eq!(stdio_server.transport(), McpTransport::Stdio);

        let remote_server = loaded
            .mcp()
            .get("remote-server")
            .expect("remote server should exist");
        assert_eq!(remote_server.scope, ConfigSource::Local);
        assert_eq!(remote_server.transport(), McpTransport::Ws);
        match &remote_server.config {
            McpServerConfig::Ws(config) => {
                assert_eq!(config.url, "wss://override.test/mcp");
                assert_eq!(
                    config.headers.get("X-Env").map(String::as_str),
                    Some("local")
                );
            }
            other => panic!("expected ws config, got {other:?}"),
        }

        let oauth = loaded.oauth().expect("oauth config should exist");
        assert_eq!(oauth.client_id, "runtime-client");
        assert_eq!(oauth.callback_port, Some(54_545));
        assert_eq!(oauth.scopes, vec!["org:read", "user:write"]);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn rejects_invalid_mcp_server_shapes() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(&home).expect("home config dir");
        fs::create_dir_all(&cwd).expect("project dir");
        fs::write(
            home.join("settings.json"),
            r#"{"mcpServers":{"broken":{"type":"http","url":123}}}"#,
        )
        .expect("write broken settings");

        let error = ConfigLoader::new(&cwd, &home)
            .load()
            .expect_err("config should fail");
        assert!(error
            .to_string()
            .contains("mcpServers.broken: missing string field url"));

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn config_loader_new_preserves_cwd_and_config_home() {
        let cwd = std::path::PathBuf::from("project");
        let config_home = std::path::PathBuf::from("home/.acrawl");

        let loader = ConfigLoader::new(&cwd, &config_home);

        let discovered = loader.discover();
        assert_eq!(discovered[1].path, config_home.join("settings.json"));
        assert_eq!(discovered[2].path, cwd.join(".acrawl.json"));
    }

    #[test]
    fn load_returns_empty_runtime_config_when_no_files_exist() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(&cwd).expect("project dir");
        fs::create_dir_all(&home).expect("home dir");

        let loaded = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("missing config files should succeed");

        assert_eq!(loaded, RuntimeConfig::empty());

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn load_parses_present_config_file() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(&cwd).expect("project dir");
        fs::create_dir_all(&home).expect("home dir");
        fs::write(
            home.join("settings.json"),
            r#"{"model":"anthropic/claude-opus-4-6","hooks":{"PreToolUse":["echo pre"]}}"#,
        )
        .expect("write settings");

        let loaded = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("config should load");

        assert_eq!(loaded.loaded_entries().len(), 1);
        assert_eq!(loaded.loaded_entries()[0].source, ConfigSource::User);
        assert_eq!(loaded.model(), Some("anthropic/claude-opus-4-6"));
        assert_eq!(loaded.hooks().pre_tool_use(), &["echo pre".to_string()]);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn malformed_non_legacy_json_returns_parse_error() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(&cwd).expect("project dir");
        fs::create_dir_all(&home).expect("home dir");
        fs::write(home.join("settings.json"), "{not-json}").expect("write malformed settings");

        let error = ConfigLoader::new(&cwd, &home)
            .load()
            .expect_err("malformed JSON should fail");

        assert!(matches!(error, crate::config::ConfigError::Parse(_)));
        assert!(error.to_string().contains("settings.json"));

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn discover_returns_five_entries_with_expected_sources() {
        let loader = ConfigLoader::new("project", "home/.acrawl");
        let entries = loader.discover();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].source, ConfigSource::User);
        assert_eq!(entries[1].source, ConfigSource::User);
        assert_eq!(entries[2].source, ConfigSource::Project);
        assert_eq!(entries[3].source, ConfigSource::Project);
        assert_eq!(entries[4].source, ConfigSource::Local);
    }

    #[test]
    fn runtime_config_get_returns_none_for_absent_keys() {
        let config = RuntimeConfig::empty();
        assert!(config.get("nonexistent").is_none());
        assert!(config.get("model").is_none());
        assert!(config.get("env").is_none());
    }

    #[test]
    fn as_json_produces_object_matching_merged_content() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        fs::create_dir_all(&cwd).expect("project dir");
        fs::create_dir_all(&home).expect("home dir");
        fs::write(
            home.join("settings.json"),
            r#"{"model":"test-model","headless":true}"#,
        )
        .expect("write settings");

        let loaded = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("config should load");

        let json = loaded.as_json();
        assert_eq!(
            json.as_object()
                .and_then(|o| o.get("model"))
                .and_then(JsonValue::as_str),
            Some("test-model")
        );
        assert_eq!(
            json.as_object()
                .and_then(|o| o.get("headless"))
                .and_then(JsonValue::as_bool),
            Some(true)
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }
}
