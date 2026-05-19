use regex::Regex;
use serde_json::Value;

use crate::{
    tool_effect::{CrawlScope, CrawlTask},
    ToolEffect, ToolError,
};

/// Parse the LLM's fork tool input into a typed [`CrawlTask`]. The shape is:
///
/// ```json
/// {
///   "objective": "collect details from posts page 2",
///   "scope": { "type": "single_page", "url": "https://example.com/posts?page=2" },
///   "success_criteria": "extracted at least 10 items",
///   "max_steps": 12
/// }
/// ```
///
/// `scope.type` is one of `single_page`, `url_list`, `url_pattern`. The
/// parser validates the regex compiles when `scope.type == url_pattern`.
pub fn execute(input: &Value) -> Result<ToolEffect, ToolError> {
    let objective = input
        .get("objective")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| ToolError::new("fork requires objective".to_string()))?
        .to_string();
    if objective.is_empty() {
        return Err(ToolError::new(
            "fork requires non-empty objective".to_string(),
        ));
    }

    let scope_value = input
        .get("scope")
        .ok_or_else(|| ToolError::new("fork requires scope".to_string()))?;
    let scope = parse_scope(scope_value)?;

    let success_criteria = input
        .get("success_criteria")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let max_steps = input
        .get("max_steps")
        .and_then(Value::as_u64)
        .map(|v| usize::try_from(v).unwrap_or(usize::MAX));
    let deadline_secs = input.get("deadline_secs").and_then(Value::as_u64);

    Ok(ToolEffect::Spawn(CrawlTask {
        objective,
        scope,
        success_criteria,
        max_steps,
        deadline_secs,
        page_index: None,
    }))
}

fn parse_scope(value: &Value) -> Result<CrawlScope, ToolError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ToolError::new("fork scope must be an object".to_string()))?;
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("fork scope.type must be a string".to_string()))?;
    match kind {
        "single_page" => {
            let url = obj
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .ok_or_else(|| {
                    ToolError::new("fork scope.url is required for single_page".to_string())
                })?;
            if url.is_empty() {
                return Err(ToolError::new(
                    "fork scope.url must not be empty".to_string(),
                ));
            }
            Ok(CrawlScope::SinglePage {
                url: url.to_string(),
            })
        }
        "url_list" => {
            let urls = obj.get("urls").and_then(Value::as_array).ok_or_else(|| {
                ToolError::new("fork scope.urls is required for url_list".to_string())
            })?;
            if urls.is_empty() {
                return Err(ToolError::new(
                    "fork scope.urls must contain at least one URL".to_string(),
                ));
            }
            let urls = urls
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            ToolError::new(
                                "fork scope.urls entries must be non-empty strings".to_string(),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CrawlScope::UrlList { urls })
        }
        "url_pattern" => {
            let regex = obj
                .get("regex")
                .and_then(Value::as_str)
                .map(str::trim)
                .ok_or_else(|| {
                    ToolError::new("fork scope.regex is required for url_pattern".to_string())
                })?;
            if regex.is_empty() {
                return Err(ToolError::new(
                    "fork scope.regex must not be empty".to_string(),
                ));
            }
            // Compile to surface invalid regex early — the registry will
            // re-compile, but the parser surfaces the error before the
            // claim attempt.
            Regex::new(regex)
                .map_err(|error| ToolError::new(format!("fork scope.regex is invalid: {error}")))?;
            Ok(CrawlScope::UrlPattern {
                regex: regex.to_string(),
            })
        }
        other => Err(ToolError::new(format!(
            "fork scope.type must be `single_page`, `url_list`, or `url_pattern`; got `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fork_returns_spawn_effect_for_single_page() {
        let effect = execute(&json!({
            "objective": "collect details",
            "scope": { "type": "single_page", "url": "https://example.com/a" }
        }))
        .expect("fork should parse single_page scope");
        match effect {
            ToolEffect::Spawn(task) => {
                assert_eq!(task.objective, "collect details");
                assert_eq!(
                    task.scope,
                    CrawlScope::SinglePage {
                        url: "https://example.com/a".to_string()
                    }
                );
                assert_eq!(task.page_index, None);
            }
            _ => panic!("expected Spawn effect"),
        }
    }

    #[test]
    fn fork_returns_spawn_effect_for_url_list() {
        let effect = execute(&json!({
            "objective": "compare results",
            "scope": { "type": "url_list", "urls": ["https://a.com", "https://b.com"] }
        }))
        .expect("fork should parse url_list scope");
        match effect {
            ToolEffect::Spawn(task) => {
                assert_eq!(
                    task.scope,
                    CrawlScope::UrlList {
                        urls: vec!["https://a.com".to_string(), "https://b.com".to_string()]
                    }
                );
            }
            _ => panic!("expected Spawn effect"),
        }
    }

    #[test]
    fn fork_returns_spawn_effect_for_url_pattern() {
        let effect = execute(&json!({
            "objective": "crawl posts",
            "scope": { "type": "url_pattern", "regex": "^https://example\\.com/posts/.*" }
        }))
        .expect("fork should parse url_pattern scope");
        match effect {
            ToolEffect::Spawn(task) => {
                assert_eq!(
                    task.scope,
                    CrawlScope::UrlPattern {
                        regex: "^https://example\\.com/posts/.*".to_string()
                    }
                );
            }
            _ => panic!("expected Spawn effect"),
        }
    }

    #[test]
    fn fork_rejects_missing_objective() {
        let err = execute(&json!({
            "scope": { "type": "single_page", "url": "https://example.com" }
        }))
        .expect_err("missing objective should fail");
        assert!(err.to_string().contains("requires objective"));
    }

    #[test]
    fn fork_rejects_empty_objective() {
        let err = execute(&json!({
            "objective": "   ",
            "scope": { "type": "single_page", "url": "https://example.com" }
        }))
        .expect_err("empty objective should fail");
        assert!(err.to_string().contains("non-empty objective"));
    }

    #[test]
    fn fork_rejects_missing_scope() {
        let err =
            execute(&json!({"objective": "do something"})).expect_err("missing scope should fail");
        assert!(err.to_string().contains("requires scope"));
    }

    #[test]
    fn fork_rejects_unknown_scope_type() {
        let err = execute(&json!({
            "objective": "x",
            "scope": { "type": "everything" }
        }))
        .expect_err("unknown scope.type should fail");
        assert!(err.to_string().contains("must be `single_page`"));
    }

    #[test]
    fn fork_rejects_empty_url_list() {
        let err = execute(&json!({
            "objective": "x",
            "scope": { "type": "url_list", "urls": [] }
        }))
        .expect_err("empty urls should fail");
        assert!(err.to_string().contains("at least one URL"));
    }

    #[test]
    fn fork_rejects_invalid_regex() {
        let err = execute(&json!({
            "objective": "x",
            "scope": { "type": "url_pattern", "regex": "[broken" }
        }))
        .expect_err("invalid regex should fail");
        assert!(err.to_string().contains("invalid"));
    }
}
