use browser::{InterceptAction, InterceptRule, MockResponse};
use serde_json::{json, Value};

use crate::{BrowserContext, CrawlState, ToolEffect, ToolExecutionError};

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let action = input["action"].as_str().unwrap_or("");

    match action {
        "block" | "mock_response" => {
            let pattern = input["pattern"]
                .as_str()
                .ok_or_else(|| ToolExecutionError::new("pattern is required for block/mock_response"))?;

            let intercept_action = if action == "block" {
                InterceptAction::Block
            } else {
                InterceptAction::MockResponse
            };

            let mock = if action == "mock_response" {
                let mock_val = &input["mock"];
                Some(MockResponse {
                    status: mock_val["status"].as_u64().map(|s| s as u16).unwrap_or(200),
                    headers: mock_val["headers"].as_object().map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    }),
                    body: mock_val["body"].as_str().map(str::to_string),
                    content_type: mock_val["content_type"].as_str().map(str::to_string),
                })
            } else {
                None
            };

            let rule = InterceptRule {
                pattern: pattern.to_string(),
                action: intercept_action,
                mock,
            };

            let rule_id = browser
                .acquire_bridge()
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?
                .add_intercept_rule(rule)
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?;

            crawl_state
                .intercept_rules
                .push((rule_id, pattern.to_string(), action.to_string()));
        }

        "remove_rule" => {
            let rule_id = input["rule_id"]
                .as_str()
                .ok_or_else(|| ToolExecutionError::new("rule_id is required for remove_rule"))?;

            browser
                .acquire_bridge()
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?
                .remove_intercept_rule(rule_id)
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?;

            crawl_state
                .intercept_rules
                .retain(|(id, _, _)| id.as_str() != rule_id);
        }

        "clear_all" => {
            browser
                .acquire_bridge()
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?
                .clear_intercept_rules()
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?;

            crawl_state.intercept_rules.clear();
        }

        other => {
            return Err(ToolExecutionError::new(format!(
                "unknown action '{other}'. Valid: block, mock_response, remove_rule, clear_all"
            )));
        }
    }

    let rules_json: Vec<Value> = crawl_state
        .intercept_rules
        .iter()
        .map(|(id, pat, act)| {
            json!({
                "rule_id": id,
                "pattern": pat,
                "action": act,
                "hit_count": 0
            })
        })
        .collect();

    Ok(ToolEffect::Reply(
        json!({"rules_active": rules_json}).to_string(),
    ))
}
