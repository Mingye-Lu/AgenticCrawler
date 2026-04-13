use serde::{Deserialize, Serialize};
use std::fmt::Display;

use crate::error::ApiError;
use crate::AuthSource;

/// Anthropic model representation from the models API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicModel {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Display for AnthropicModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(display_name) = &self.display_name {
            write!(f, "{display_name}")
        } else {
            write!(f, "{}", self.id)
        }
    }
}

/// List of Anthropic models from the API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicModelList {
    pub data: Vec<AnthropicModel>,
}

/// `OpenAI` model representation from the models API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAiModel {
    pub id: String,
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub owned_by: Option<String>,
}

impl Display for OpenAiModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

/// List of `OpenAI` models from the API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiModelList {
    pub data: Vec<OpenAiModel>,
}

/// Fetch the list of available Anthropic models.
///
/// # Errors
///
/// Returns `ApiError` if the HTTP request fails or the response cannot be deserialized.
pub async fn list_anthropic_models(api_key: &str) -> Result<Vec<AnthropicModel>, ApiError> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(ApiError::Http)?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: None,
            body,
            retryable: status.is_server_error(),
        });
    }

    let model_list: AnthropicModelList = response.json().await.map_err(ApiError::Http)?;

    Ok(model_list.data)
}

/// Fetch the list of available `OpenAI` models.
///
/// # Errors
///
/// Returns `ApiError` if the HTTP request fails or the response cannot be deserialized.
pub async fn list_openai_models(auth: &AuthSource) -> Result<Vec<OpenAiModel>, ApiError> {
    let client = reqwest::Client::new();
    let mut request = client.get("https://api.openai.com/v1/models");

    // Use bearer token if available, fall back to API key
    if let Some(token) = auth.bearer_token() {
        request = request.bearer_auth(token);
    } else if let Some(api_key) = auth.api_key() {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await.map_err(ApiError::Http)?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: None,
            body,
            retryable: status.is_server_error(),
        });
    }

    let model_list: OpenAiModelList = response.json().await.map_err(ApiError::Http)?;

    Ok(model_list.data)
}

/// Fetch models from the public models.dev catalog.
/// This is used for OAuth-authenticated providers where `/v1/models` requires
/// API-key scopes that OAuth tokens do not carry.
///
/// # Errors
///
/// Returns `ApiError` if the HTTP request fails or the response cannot be deserialized.
pub async fn list_models_dev(provider_id: &str) -> Result<Vec<OpenAiModel>, ApiError> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://models.dev/api.json")
        .header("User-Agent", "acrawl")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(ApiError::Http)?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: None,
            body,
            retryable: status.is_server_error(),
        });
    }

    let catalog: std::collections::HashMap<String, serde_json::Value> =
        response.json().await.map_err(ApiError::Http)?;
    let Some(provider) = catalog.get(provider_id) else {
        return Ok(vec![]);
    };
    let Some(models_obj) = provider.get("models").and_then(|v| v.as_object()) else {
        return Ok(vec![]);
    };
    Ok(models_obj
        .keys()
        .map(|id| OpenAiModel {
            id: id.clone(),
            created: None,
            owned_by: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_model_list_deserializes() {
        let json = r#"{
            "data": [
                {"id": "claude-opus-4-6", "display_name": "Claude Opus 4.6", "created_at": "2024-01-01T00:00:00Z"},
                {"id": "claude-sonnet-4-6", "display_name": "Claude Sonnet 4.6", "created_at": "2024-01-01T00:00:00Z"}
            ],
            "has_more": false
        }"#;

        let model_list: AnthropicModelList =
            serde_json::from_str(json).expect("failed to deserialize");
        assert_eq!(model_list.data.len(), 2);
        assert_eq!(model_list.data[0].id, "claude-opus-4-6");
        assert_eq!(
            model_list.data[0].display_name,
            Some("Claude Opus 4.6".to_string())
        );
        assert_eq!(
            model_list.data[0].created_at,
            Some("2024-01-01T00:00:00Z".to_string())
        );
    }

    #[test]
    fn anthropic_model_display_prefers_display_name() {
        let model = AnthropicModel {
            id: "claude-3".to_string(),
            display_name: Some("Claude 3".to_string()),
            created_at: None,
        };
        assert_eq!(model.to_string(), "Claude 3");
    }

    #[test]
    fn anthropic_model_display_falls_back_to_id() {
        let model = AnthropicModel {
            id: "claude-3".to_string(),
            display_name: None,
            created_at: None,
        };
        assert_eq!(model.to_string(), "claude-3");
    }

    #[test]
    fn openai_model_list_deserializes() {
        let json = r#"{
            "object": "list",
            "data": [
                {"id": "gpt-4o", "object": "model", "created": 1715367049, "owned_by": "openai"},
                {"id": "o3", "object": "model", "created": 1715367050, "owned_by": "openai"}
            ]
        }"#;

        let model_list: OpenAiModelList =
            serde_json::from_str(json).expect("failed to deserialize");
        assert_eq!(model_list.data.len(), 2);
        assert_eq!(model_list.data[0].id, "gpt-4o");
        assert_eq!(model_list.data[0].created, Some(1_715_367_049));
        assert_eq!(model_list.data[0].owned_by, Some("openai".to_string()));
    }

    #[test]
    fn openai_model_display_uses_id() {
        let model = OpenAiModel {
            id: "gpt-4o".to_string(),
            created: Some(1_715_367_049),
            owned_by: Some("openai".to_string()),
        };
        assert_eq!(model.to_string(), "gpt-4o");
    }

    #[test]
    fn anthropic_model_extra_fields_ignored() {
        let json = r#"{
            "id": "claude-3",
            "display_name": "Claude 3",
            "created_at": "2024-01-01T00:00:00Z",
            "extra_field": "should be ignored",
            "another_field": 123
        }"#;

        let model: AnthropicModel = serde_json::from_str(json).expect("failed to deserialize");
        assert_eq!(model.id, "claude-3");
        assert_eq!(model.display_name, Some("Claude 3".to_string()));
    }

    #[test]
    fn openai_model_extra_fields_ignored() {
        let json = r#"{
            "id": "gpt-4o",
            "created": 1715367049,
            "owned_by": "openai",
            "object": "model",
            "extra_field": "should be ignored"
        }"#;

        let model: OpenAiModel = serde_json::from_str(json).expect("failed to deserialize");
        assert_eq!(model.id, "gpt-4o");
        assert_eq!(model.created, Some(1_715_367_049));
    }
}
