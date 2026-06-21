use api::StoredProviderConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredGroup {
    Simple,
    Bedrock,
    Azure,
    Custom,
    Vertex,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredInputs {
    pub api_key: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub region: Option<String>,
    pub resource_name: Option<String>,
    pub deployment_name: Option<String>,
    pub base_url: Option<String>,
    pub gcp_project: Option<String>,
    pub gcp_region: Option<String>,
}

#[must_use]
pub fn build_provider_config(group: CredGroup, inputs: CredInputs) -> StoredProviderConfig {
    let api_key = normalize(inputs.api_key);
    let access_key = normalize(inputs.access_key);
    let secret_key = normalize(inputs.secret_key);
    let region = normalize(inputs.region);
    let resource_name = normalize(inputs.resource_name);
    let deployment_name = normalize(inputs.deployment_name);
    let base_url = normalize(inputs.base_url);
    let gcp_project_id = normalize(inputs.gcp_project);
    let gcp_region = normalize(inputs.gcp_region);

    match group {
        CredGroup::Simple => StoredProviderConfig {
            auth_method: simple_auth_method(api_key.as_deref(), base_url.as_deref()).to_string(),
            api_key,
            base_url,
            ..StoredProviderConfig::default()
        },
        CredGroup::Bedrock => StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: access_key,
            aws_secret_access_key: secret_key,
            region,
            ..StoredProviderConfig::default()
        },
        CredGroup::Azure => StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key,
            resource_name,
            deployment_name,
            ..StoredProviderConfig::default()
        },
        CredGroup::Custom => {
            let auth_method = if api_key.is_some() { "api_key" } else { "none" };
            StoredProviderConfig {
                auth_method: auth_method.to_string(),
                api_key,
                base_url,
                ..StoredProviderConfig::default()
            }
        }
        CredGroup::Vertex => StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key,
            gcp_project_id,
            gcp_region,
            ..StoredProviderConfig::default()
        },
    }
}

fn normalize(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn simple_auth_method(api_key: Option<&str>, base_url: Option<&str>) -> &'static str {
    if base_url.is_some() || api_key.is_some_and(|key| key.starts_with("sk-ant-")) {
        "api_key"
    } else {
        "openai_key"
    }
}

#[cfg(test)]
mod tests {
    use super::{build_provider_config, CredGroup, CredInputs};
    use api::StoredProviderConfig;

    #[test]
    fn simple_openai_matches_interactive_shape() {
        let config = build_provider_config(
            CredGroup::Simple,
            CredInputs {
                api_key: Some("sk-test-123".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "openai_key".to_string(),
                api_key: Some("sk-test-123".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }

    #[test]
    fn simple_anthropic_matches_interactive_shape() {
        let config = build_provider_config(
            CredGroup::Simple,
            CredInputs {
                api_key: Some("sk-ant-test-123".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "api_key".to_string(),
                api_key: Some("sk-ant-test-123".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }

    #[test]
    fn simple_preset_matches_interactive_shape() {
        let config = build_provider_config(
            CredGroup::Simple,
            CredInputs {
                api_key: Some("gemini-test-123".to_string()),
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "api_key".to_string(),
                api_key: Some("gemini-test-123".to_string()),
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }

    #[test]
    fn bedrock_matches_interactive_shape() {
        let config = build_provider_config(
            CredGroup::Bedrock,
            CredInputs {
                access_key: Some("AKIA_TEST".to_string()),
                secret_key: Some("secret-test".to_string()),
                region: Some("us-east-1".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "api_key".to_string(),
                api_key: Some("AKIA_TEST".to_string()),
                aws_secret_access_key: Some("secret-test".to_string()),
                region: Some("us-east-1".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }

    #[test]
    fn azure_matches_interactive_shape() {
        let config = build_provider_config(
            CredGroup::Azure,
            CredInputs {
                api_key: Some("azure-test-123".to_string()),
                resource_name: Some("myresource".to_string()),
                deployment_name: Some("gpt-4o".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "api_key".to_string(),
                api_key: Some("azure-test-123".to_string()),
                resource_name: Some("myresource".to_string()),
                deployment_name: Some("gpt-4o".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }

    #[test]
    fn custom_without_api_key_matches_interactive_shape() {
        let config = build_provider_config(
            CredGroup::Custom,
            CredInputs {
                base_url: Some("http://localhost:11434/v1".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "none".to_string(),
                base_url: Some("http://localhost:11434/v1".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }

    #[test]
    fn custom_with_api_key_matches_interactive_shape() {
        let config = build_provider_config(
            CredGroup::Custom,
            CredInputs {
                api_key: Some("local-key".to_string()),
                base_url: Some("http://localhost:11434/v1".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "api_key".to_string(),
                api_key: Some("local-key".to_string()),
                base_url: Some("http://localhost:11434/v1".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }

    #[test]
    fn vertex_matches_expected_shape() {
        let config = build_provider_config(
            CredGroup::Vertex,
            CredInputs {
                api_key: Some("ya29.test-token".to_string()),
                gcp_project: Some("my-project".to_string()),
                gcp_region: Some("us-central1".to_string()),
                ..CredInputs::default()
            },
        );

        assert_eq!(
            config,
            StoredProviderConfig {
                auth_method: "api_key".to_string(),
                api_key: Some("ya29.test-token".to_string()),
                gcp_project_id: Some("my-project".to_string()),
                gcp_region: Some("us-central1".to_string()),
                ..StoredProviderConfig::default()
            }
        );
    }
}
