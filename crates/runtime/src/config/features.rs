use crate::json::JsonValue;
use crate::sandbox::{FilesystemIsolationMode, SandboxConfig};

use super::{
    expect_object, expect_string, optional_bool, optional_string, optional_string_array,
    optional_u16, ConfigError, McpConfigCollection,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeFeatureConfig {
    pub(super) hooks: RuntimeHookConfig,
    pub(super) mcp: McpConfigCollection,
    pub(super) oauth: Option<OAuthConfig>,
    pub(super) model: Option<String>,
    pub(super) sandbox: SandboxConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeHookConfig {
    pre_tool_use: Vec<String>,
    post_tool_use: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub callback_port: Option<u16>,
    pub manual_redirect_url: Option<String>,
    pub scopes: Vec<String>,
}

impl RuntimeFeatureConfig {
    #[must_use]
    pub fn with_hooks(mut self, hooks: RuntimeHookConfig) -> Self {
        self.hooks = hooks;
        self
    }

    #[must_use]
    pub fn hooks(&self) -> &RuntimeHookConfig {
        &self.hooks
    }

    #[must_use]
    pub fn mcp(&self) -> &McpConfigCollection {
        &self.mcp
    }

    #[must_use]
    pub fn oauth(&self) -> Option<&OAuthConfig> {
        self.oauth.as_ref()
    }

    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    #[must_use]
    pub fn sandbox(&self) -> &SandboxConfig {
        &self.sandbox
    }
}

impl RuntimeHookConfig {
    #[must_use]
    pub fn new(pre_tool_use: Vec<String>, post_tool_use: Vec<String>) -> Self {
        Self {
            pre_tool_use,
            post_tool_use,
        }
    }

    #[must_use]
    pub fn pre_tool_use(&self) -> &[String] {
        &self.pre_tool_use
    }

    #[must_use]
    pub fn post_tool_use(&self) -> &[String] {
        &self.post_tool_use
    }
}

pub(super) fn parse_optional_hooks_config(root: &JsonValue) -> Result<RuntimeHookConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(RuntimeHookConfig::default());
    };
    let Some(hooks_value) = object.get("hooks") else {
        return Ok(RuntimeHookConfig::default());
    };
    let hooks = expect_object(hooks_value, "merged settings.hooks")?;
    Ok(RuntimeHookConfig {
        pre_tool_use: optional_hook_command_array(hooks, "PreToolUse", "merged settings.hooks")?
            .unwrap_or_default(),
        post_tool_use: optional_hook_command_array(hooks, "PostToolUse", "merged settings.hooks")?
            .unwrap_or_default(),
    })
}

pub(super) fn parse_optional_sandbox_config(root: &JsonValue) -> Result<SandboxConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(SandboxConfig::default());
    };
    let Some(sandbox_value) = object.get("sandbox") else {
        return Ok(SandboxConfig::default());
    };
    let sandbox = expect_object(sandbox_value, "merged settings.sandbox")?;
    let filesystem_mode = optional_string(sandbox, "filesystemMode", "merged settings.sandbox")?
        .map(parse_filesystem_mode_label)
        .transpose()?;
    Ok(SandboxConfig {
        enabled: optional_bool(sandbox, "enabled", "merged settings.sandbox")?,
        namespace_restrictions: optional_bool(
            sandbox,
            "namespaceRestrictions",
            "merged settings.sandbox",
        )?,
        network_isolation: optional_bool(sandbox, "networkIsolation", "merged settings.sandbox")?,
        filesystem_mode,
        allowed_mounts: optional_string_array(sandbox, "allowedMounts", "merged settings.sandbox")?
            .unwrap_or_default(),
    })
}

fn parse_filesystem_mode_label(value: &str) -> Result<FilesystemIsolationMode, ConfigError> {
    match value {
        "off" => Ok(FilesystemIsolationMode::Off),
        "workspace-only" => Ok(FilesystemIsolationMode::WorkspaceOnly),
        "allow-list" => Ok(FilesystemIsolationMode::AllowList),
        other => Err(ConfigError::Parse(format!(
            "merged settings.sandbox.filesystemMode: unsupported filesystem mode {other}"
        ))),
    }
}

pub(super) fn parse_optional_oauth_config(
    root: &JsonValue,
    context: &str,
) -> Result<Option<OAuthConfig>, ConfigError> {
    let Some(oauth_value) = root.as_object().and_then(|object| object.get("oauth")) else {
        return Ok(None);
    };
    let object = expect_object(oauth_value, context)?;
    let client_id = expect_string(object, "clientId", context)?.to_string();
    let authorize_url = expect_string(object, "authorizeUrl", context)?.to_string();
    let token_url = expect_string(object, "tokenUrl", context)?.to_string();
    let callback_port = optional_u16(object, "callbackPort", context)?;
    let manual_redirect_url = optional_string(object, "manualRedirectUrl", context)?.map(str::to_string);
    let scopes = optional_string_array(object, "scopes", context)?.unwrap_or_default();
    Ok(Some(OAuthConfig {
        client_id,
        authorize_url,
        token_url,
        callback_port,
        manual_redirect_url,
        scopes,
    }))
}

/// Hook lists may be plain command strings or nested objects (`command`, nested `hooks`).
fn optional_hook_command_array(
    object: &std::collections::BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<Vec<String>>, ConfigError> {
    match object.get(key) {
        Some(value) => {
            let Some(array) = value.as_array() else {
                return Err(ConfigError::Parse(format!(
                    "{context}: field {key} must be an array"
                )));
            };
            let mut out = Vec::new();
            for item in array {
                collect_hook_commands(item, &mut out);
            }
            Ok(Some(out))
        }
        None => Ok(None),
    }
}

fn collect_hook_commands(value: &JsonValue, out: &mut Vec<String>) {
    if let Some(text) = value.as_str() {
        out.push(text.to_string());
        return;
    }
    let Some(obj) = value.as_object() else {
        return;
    };
    let is_command_hook = obj
        .get("type")
        .and_then(JsonValue::as_str)
        .is_none_or(|hook_type| hook_type == "command");
    if let Some(cmd) = obj.get("command").and_then(JsonValue::as_str) {
        if is_command_hook {
            out.push(cmd.to_string());
        }
    }
    if let Some(hooks) = obj.get("hooks").and_then(JsonValue::as_array) {
        for hook in hooks {
            collect_hook_commands(hook, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::json::JsonValue;
    use crate::sandbox::FilesystemIsolationMode;

    use super::{OAuthConfig, RuntimeFeatureConfig, RuntimeHookConfig};
    use crate::config::{ConfigLoader, ConfigSource, RuntimeConfig};

    fn temp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("runtime-config-{nanos}"))
    }

    #[test]
    fn loads_hook_commands_from_nested_objects() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        std::fs::create_dir_all(cwd.join(".acrawl")).expect("project config dir");
        std::fs::create_dir_all(&home).expect("home config dir");

        std::fs::write(
            cwd.join(".acrawl").join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/bin/pre"}]}],"PostToolUse":["shell-p"]}}"#,
        )
        .expect("write project settings");

        let loaded = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("config should load");

        assert_eq!(loaded.hooks().pre_tool_use(), &["/bin/pre".to_string()]);
        assert_eq!(loaded.hooks().post_tool_use(), &["shell-p".to_string()]);

        std::fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn parses_sandbox_config() {
        let root = temp_dir();
        let cwd = root.join("project");
        let home = root.join("home").join(".acrawl");
        std::fs::create_dir_all(cwd.join(".acrawl")).expect("project config dir");
        std::fs::create_dir_all(&home).expect("home config dir");

        std::fs::write(
            cwd.join(".acrawl").join("settings.local.json"),
            r#"{
              "sandbox": {
                "enabled": true,
                "namespaceRestrictions": false,
                "networkIsolation": true,
                "filesystemMode": "allow-list",
                "allowedMounts": ["logs", "tmp/cache"]
              }
            }"#,
        )
        .expect("write local settings");

        let loaded = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("config should load");

        assert_eq!(loaded.sandbox().enabled, Some(true));
        assert_eq!(loaded.sandbox().namespace_restrictions, Some(false));
        assert_eq!(loaded.sandbox().network_isolation, Some(true));
        assert_eq!(
            loaded.sandbox().filesystem_mode,
            Some(FilesystemIsolationMode::AllowList)
        );
        assert_eq!(loaded.sandbox().allowed_mounts, vec!["logs", "tmp/cache"]);

        std::fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn runtime_feature_config_defaults_and_with_hooks_work() {
        let hooks = RuntimeHookConfig::new(vec!["before".to_string()], vec!["after".to_string()]);
        let feature_config = RuntimeFeatureConfig::default().with_hooks(hooks.clone());

        assert_eq!(RuntimeFeatureConfig::default().hooks(), &RuntimeHookConfig::default());
        assert_eq!(feature_config.hooks(), &hooks);
        assert!(feature_config.mcp().servers().is_empty());
        assert!(feature_config.oauth().is_none());
        assert!(feature_config.model().is_none());
        assert_eq!(feature_config.sandbox().filesystem_mode, None);
    }

    #[test]
    fn runtime_config_empty_has_expected_default_values() {
        let config = RuntimeConfig::empty();

        assert!(config.merged().is_empty());
        assert!(config.loaded_entries().is_empty());
        assert_eq!(config.as_json(), JsonValue::Object(BTreeMap::default()));
        assert_eq!(config.feature_config(), &RuntimeFeatureConfig::default());
        assert_eq!(config.hooks().pre_tool_use(), &[] as &[String]);
        assert_eq!(config.hooks().post_tool_use(), &[] as &[String]);
        assert!(config.oauth().is_none());
        assert!(config.model().is_none());
        assert_eq!(config.sandbox().allowed_mounts, Vec::<String>::new());
    }

    #[test]
    fn config_source_variants_construct_and_compare_in_expected_order() {
        let variants = [ConfigSource::User, ConfigSource::Project, ConfigSource::Local];

        assert_eq!(variants[0], ConfigSource::User);
        assert_eq!(variants[1], ConfigSource::Project);
        assert_eq!(variants[2], ConfigSource::Local);
        assert!(variants[0] < variants[1]);
        assert!(variants[1] < variants[2]);
    }

    #[test]
    fn hook_config_new_stores_and_retrieves_command_lists() {
        let hooks = RuntimeHookConfig::new(
            vec!["pre-1".to_string(), "pre-2".to_string()],
            vec!["post-1".to_string()],
        );
        assert_eq!(hooks.pre_tool_use(), &["pre-1", "pre-2"]);
        assert_eq!(hooks.post_tool_use(), &["post-1"]);
    }

    #[test]
    fn oauth_config_stores_all_fields_correctly() {
        let oauth = OAuthConfig {
            client_id: "client-123".to_string(),
            authorize_url: "https://auth.example/authorize".to_string(),
            token_url: "https://auth.example/token".to_string(),
            callback_port: Some(8080),
            manual_redirect_url: Some("https://auth.example/callback".to_string()),
            scopes: vec!["read".to_string(), "write".to_string()],
        };
        assert_eq!(oauth.client_id, "client-123");
        assert_eq!(oauth.authorize_url, "https://auth.example/authorize");
        assert_eq!(oauth.token_url, "https://auth.example/token");
        assert_eq!(oauth.callback_port, Some(8080));
        assert_eq!(
            oauth.manual_redirect_url.as_deref(),
            Some("https://auth.example/callback")
        );
        assert_eq!(oauth.scopes, vec!["read", "write"]);
    }
}
