use std::collections::BTreeMap;

use crate::json::JsonValue;

use super::{expect_object, expect_string, optional_bool, optional_string, optional_u16, ConfigError, ConfigSource};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpConfigCollection {
    pub(super) servers: BTreeMap<String, ScopedMcpServerConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedMcpServerConfig {
    pub scope: ConfigSource,
    pub config: McpServerConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    Sse,
    Http,
    Ws,
    Sdk,
    ClaudeAiProxy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerConfig {
    Stdio(McpStdioServerConfig),
    Sse(McpRemoteServerConfig),
    Http(McpRemoteServerConfig),
    Ws(McpWebSocketServerConfig),
    Sdk(McpSdkServerConfig),
    ClaudeAiProxy(McpClaudeAiProxyServerConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStdioServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRemoteServerConfig {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub headers_helper: Option<String>,
    pub oauth: Option<McpOAuthConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpWebSocketServerConfig {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub headers_helper: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSdkServerConfig {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClaudeAiProxyServerConfig {
    pub url: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthConfig {
    pub client_id: Option<String>,
    pub callback_port: Option<u16>,
    pub auth_server_metadata_url: Option<String>,
    pub xaa: Option<bool>,
}

impl McpConfigCollection {
    #[must_use]
    pub fn servers(&self) -> &BTreeMap<String, ScopedMcpServerConfig> {
        &self.servers
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ScopedMcpServerConfig> {
        self.servers.get(name)
    }
}

impl ScopedMcpServerConfig {
    #[must_use]
    pub fn transport(&self) -> McpTransport {
        self.config.transport()
    }
}

impl McpServerConfig {
    #[must_use]
    pub fn transport(&self) -> McpTransport {
        match self {
            Self::Stdio(_) => McpTransport::Stdio,
            Self::Sse(_) => McpTransport::Sse,
            Self::Http(_) => McpTransport::Http,
            Self::Ws(_) => McpTransport::Ws,
            Self::Sdk(_) => McpTransport::Sdk,
            Self::ClaudeAiProxy(_) => McpTransport::ClaudeAiProxy,
        }
    }
}

pub(super) fn parse_mcp_server_config(
    server_name: &str,
    value: &JsonValue,
    context: &str,
) -> Result<McpServerConfig, ConfigError> {
    let object = expect_object(value, context)?;
    let server_type = optional_string(object, "type", context)?.unwrap_or("stdio");
    match server_type {
        "stdio" => Ok(McpServerConfig::Stdio(McpStdioServerConfig {
            command: expect_string(object, "command", context)?.to_string(),
            args: super::optional_string_array(object, "args", context)?.unwrap_or_default(),
            env: optional_string_map(object, "env", context)?.unwrap_or_default(),
        })),
        "sse" => Ok(McpServerConfig::Sse(parse_mcp_remote_server_config(object, context)?)),
        "http" => Ok(McpServerConfig::Http(parse_mcp_remote_server_config(object, context)?)),
        "ws" => Ok(McpServerConfig::Ws(McpWebSocketServerConfig {
            url: expect_string(object, "url", context)?.to_string(),
            headers: optional_string_map(object, "headers", context)?.unwrap_or_default(),
            headers_helper: optional_string(object, "headersHelper", context)?.map(str::to_string),
        })),
        "sdk" => Ok(McpServerConfig::Sdk(McpSdkServerConfig {
            name: expect_string(object, "name", context)?.to_string(),
        })),
        "claudeai-proxy" => Ok(McpServerConfig::ClaudeAiProxy(McpClaudeAiProxyServerConfig {
            url: expect_string(object, "url", context)?.to_string(),
            id: expect_string(object, "id", context)?.to_string(),
        })),
        other => Err(ConfigError::Parse(format!(
            "{context}: unsupported MCP server type for {server_name}: {other}"
        ))),
    }
}

fn parse_mcp_remote_server_config(
    object: &BTreeMap<String, JsonValue>,
    context: &str,
) -> Result<McpRemoteServerConfig, ConfigError> {
    Ok(McpRemoteServerConfig {
        url: expect_string(object, "url", context)?.to_string(),
        headers: optional_string_map(object, "headers", context)?.unwrap_or_default(),
        headers_helper: optional_string(object, "headersHelper", context)?.map(str::to_string),
        oauth: parse_optional_mcp_oauth_config(object, context)?,
    })
}

fn parse_optional_mcp_oauth_config(
    object: &BTreeMap<String, JsonValue>,
    context: &str,
) -> Result<Option<McpOAuthConfig>, ConfigError> {
    let Some(value) = object.get("oauth") else {
        return Ok(None);
    };
    let oauth = expect_object(value, &format!("{context}.oauth"))?;
    Ok(Some(McpOAuthConfig {
        client_id: optional_string(oauth, "clientId", context)?.map(str::to_string),
        callback_port: optional_u16(oauth, "callbackPort", context)?,
        auth_server_metadata_url: optional_string(oauth, "authServerMetadataUrl", context)?.map(str::to_string),
        xaa: optional_bool(oauth, "xaa", context)?,
    }))
}

fn optional_string_map(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<BTreeMap<String, String>>, ConfigError> {
    match object.get(key) {
        Some(value) => {
            let Some(map) = value.as_object() else {
                return Err(ConfigError::Parse(format!(
                    "{context}: field {key} must be an object"
                )));
            };
            map.iter()
                .map(|(entry_key, entry_value)| {
                    entry_value
                        .as_str()
                        .map(|text| (entry_key.clone(), text.to_string()))
                        .ok_or_else(|| {
                            ConfigError::Parse(format!(
                                "{context}: field {key} must contain only string values"
                            ))
                        })
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(Some)
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        McpClaudeAiProxyServerConfig, McpRemoteServerConfig, McpSdkServerConfig, McpServerConfig,
        McpStdioServerConfig, McpTransport, McpWebSocketServerConfig, ScopedMcpServerConfig,
    };
    use crate::config::ConfigSource;

    #[test]
    fn mcp_server_config_transport_matches_stdio_sse_and_http_variants() {
        let stdio = McpServerConfig::Stdio(McpStdioServerConfig {
            command: "uvx".to_string(),
            args: vec!["server".to_string()],
            env: BTreeMap::default(),
        });
        let sse = McpServerConfig::Sse(McpRemoteServerConfig {
            url: "https://example.test/sse".to_string(),
            headers: BTreeMap::default(),
            headers_helper: None,
            oauth: None,
        });
        let http = McpServerConfig::Http(McpRemoteServerConfig {
            url: "https://example.test/http".to_string(),
            headers: BTreeMap::default(),
            headers_helper: Some("helper".to_string()),
            oauth: None,
        });

        assert_eq!(stdio.transport(), McpTransport::Stdio);
        assert_eq!(sse.transport(), McpTransport::Sse);
        assert_eq!(http.transport(), McpTransport::Http);
    }

    #[test]
    fn ws_sdk_and_claude_ai_proxy_transport_detection() {
        let ws = McpServerConfig::Ws(McpWebSocketServerConfig {
            url: "wss://example.test/mcp".to_string(),
            headers: BTreeMap::default(),
            headers_helper: None,
        });
        let sdk = McpServerConfig::Sdk(McpSdkServerConfig {
            name: "test-sdk".to_string(),
        });
        let proxy = McpServerConfig::ClaudeAiProxy(McpClaudeAiProxyServerConfig {
            url: "https://proxy.test/mcp".to_string(),
            id: "proxy-1".to_string(),
        });

        assert_eq!(ws.transport(), McpTransport::Ws);
        assert_eq!(sdk.transport(), McpTransport::Sdk);
        assert_eq!(proxy.transport(), McpTransport::ClaudeAiProxy);
    }

    #[test]
    fn scoped_config_delegates_transport_to_inner_config() {
        let scoped = ScopedMcpServerConfig {
            scope: ConfigSource::Project,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: "test".to_string(),
                args: Vec::new(),
                env: BTreeMap::default(),
            }),
        };
        assert_eq!(scoped.transport(), McpTransport::Stdio);
        assert_eq!(scoped.transport(), scoped.config.transport());
    }
}
