use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

use super::feedback::InteractionKind;

#[derive(Debug)]
struct FillFormInput {
    fields: BTreeMap<String, String>,
    submit: bool,
    form_selector: String,
    widen: bool,
}

fn parse_input(input: &Value) -> Result<FillFormInput, CrawlError> {
    let fields_value = input
        .get("fields")
        .ok_or_else(|| CrawlError::new("missing required field: fields"))?;

    let fields_obj = fields_value
        .as_object()
        .ok_or_else(|| CrawlError::new("fields must be an object"))?;

    if fields_obj.is_empty() {
        return Err(CrawlError::new("fields must not be empty"));
    }

    let mut fields = BTreeMap::new();
    for (key, value) in fields_obj {
        let val_str = value
            .as_str()
            .ok_or_else(|| CrawlError::new(format!("field value for '{key}' must be a string")))?;
        fields.insert(key.clone(), val_str.to_string());
    }

    let submit = input
        .get("submit")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let form_selector = input
        .get("form_selector")
        .and_then(Value::as_str)
        .unwrap_or("form")
        .to_string();

    Ok(FillFormInput {
        fields,
        submit,
        form_selector,
        widen: input.get("widen").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_input(input)?;

    let resolved_fields: Vec<(String, String)> = params
        .fields
        .iter()
        .map(|(sel, val)| {
            let resolved = super::ref_resolve::resolve_selector(sel, browser.ref_map())
                .map_err(ToolExecutionError::new)?;
            Ok::<_, ToolExecutionError>((resolved, val.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let resolved_form_selector =
        super::ref_resolve::resolve_selector(&params.form_selector, browser.ref_map())
            .map_err(ToolExecutionError::new)?;

    fill_fields(browser, &resolved_fields).await?;

    if params.submit {
        let pre_url = match browser.acquire_bridge().await {
            Ok(mut b) => b
                .evaluate("window.location.href")
                .await
                .ok()
                .and_then(|v| v.as_str().map(String::from)),
            Err(_) => None,
        };

        let js = format!(
            r#"(() => {{
                const form = document.querySelector('{}');
                if (!form) return 'form_not_found';
                const btn = form.querySelector('button[type="submit"], input[type="submit"], button:not([type])');
                if (btn) {{ btn.click(); return 'clicked'; }}
                const evt = new Event('submit', {{ bubbles: true, cancelable: true }});
                if (form.dispatchEvent(evt)) form.submit();
                return 'dispatched';
            }})()"#,
            resolved_form_selector.replace('\'', "\\'")
        );
        let submit_result = browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .evaluate(&js)
            .await
            .map_err(|e| ToolExecutionError::new(format!("failed to submit form: {e}")))?;

        let outcome = submit_result
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        if outcome == "form_not_found" {
            return Err(ToolExecutionError::new(format!(
                "no <form> matched selector '{resolved_form_selector}'; to submit a div-based SPA form, use click(text='Submit') instead"
            )));
        }

        if let Some(ref old_url) = pre_url {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let current = match browser.acquire_bridge().await {
                    Ok(mut b) => b
                        .evaluate("window.location.href")
                        .await
                        .ok()
                        .and_then(|v| v.as_str().map(String::from)),
                    Err(_) => None,
                };
                if current.as_deref() != Some(old_url.as_str()) {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    break;
                }
            }
        }
    }

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(
        browser,
        crawl_state,
        if params.submit {
            InteractionKind::PossibleSubmit
        } else {
            InteractionKind::Passive
        },
        Some(&resolved_form_selector),
        params.widen,
    )
    .await?;

    let field_count = params.fields.len();
    Ok(ToolEffect::reply_json(&serde_json::json!({
        "seq": seq,
        "success": true,
        "message": format!(
            "Filled {field_count} field(s){}",
            if params.submit { " and submitted form" } else { "" }
        ),
        "page_state": page_state
    })))
}

async fn fill_fields(
    browser: &mut BrowserContext,
    resolved_fields: &[(String, String)],
) -> Result<(), ToolExecutionError> {
    for (selector, value) in resolved_fields {
        let fast_path = browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .fill(selector, value)
            .await;

        if let Err(fast_err) = fast_path {
            // Fallback for controls outside any <form> (div-based admin UIs,
            // modals, panels): fuzzy-match the field against page-wide labels.
            match resolve_field_by_label(browser, selector).await? {
                Some(fuzzy_selector) => {
                    browser
                        .acquire_bridge()
                        .await
                        .map_err(|e| ToolExecutionError::new(e.to_string()))?
                        .fill(&fuzzy_selector, value)
                        .await
                        .map_err(|e| {
                            ToolExecutionError::new(format!(
                                "failed to fill '{selector}' (matched label to '{fuzzy_selector}'): {e}"
                            ))
                        })?;
                }
                None => {
                    return Err(ToolExecutionError::new(format!(
                        "failed to fill '{selector}': {fast_err}"
                    )));
                }
            }
        }
    }
    Ok(())
}

const FIELD_DISCOVERY_JS: &str = r#"(() => {
    function selectorOf(el) {
        if (el.id) return '#' + CSS.escape(el.id);
        const path = [];
        let cur = el;
        while (cur && cur.parentElement) {
            if (cur.id) { path.unshift('#' + CSS.escape(cur.id)); break; }
            const parent = cur.parentElement;
            const tag = cur.tagName.toLowerCase();
            const same = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
            path.unshift(same.length > 1 ? tag + ':nth-of-type(' + (same.indexOf(cur) + 1) + ')' : tag);
            cur = parent;
        }
        return path.join(' > ');
    }
    const results = [];
    const controls = document.querySelectorAll(
        'input:not([type="hidden"]), textarea, select, ' +
        '[role="checkbox"], [role="switch"], [role="combobox"], [role="textbox"], ' +
        '[contenteditable="true"]'
    );
    for (const el of controls) {
        let label = '';
        // 1. label[for=id]
        if (el.id) {
            const lbl = document.querySelector(`label[for="${CSS.escape(el.id)}"]`);
            if (lbl) label = lbl.innerText.trim();
        }
        // 2. parent <label>
        if (!label) {
            const parent = el.closest('label');
            if (parent) label = parent.innerText.replace(el.value || '', '').trim();
        }
        // 3. aria-label
        if (!label) label = el.getAttribute('aria-label') || '';
        // 4. aria-labelledby (space-separated list of ids per spec)
        if (!label) {
            const lblById = el.getAttribute('aria-labelledby');
            if (lblById) label = lblById.split(/\s+/).filter(Boolean)
                .map(id => document.getElementById(id)?.innerText?.trim() || '')
                .filter(Boolean).join(' ').trim();
        }
        // 5. placeholder / title / name
        if (!label) label = el.placeholder || el.title || el.name || '';
        // 6. sibling textarea label — for rich editors that replaced a <textarea>
        if (!label && el.getAttribute('contenteditable') === 'true') {
            const group = el.closest('form, [class*="field"], [class*="form-group"]') || el.parentElement;
            if (group) {
                const sibling = group.querySelector('textarea');
                if (sibling) {
                    if (sibling.id) {
                        const lbl = document.querySelector(`label[for="${CSS.escape(sibling.id)}"]`);
                        if (lbl) label = lbl.innerText.trim();
                    }
                    if (!label) label = sibling.getAttribute('aria-label') || '';
                }
            }
        }
        if (label) {
            results.push([label.slice(0, 80), selectorOf(el)]);
        }
    }
    return results;
})()"#;

async fn resolve_field_by_label(
    browser: &mut BrowserContext,
    label_query: &str,
) -> Result<Option<String>, ToolExecutionError> {
    let script = FIELD_DISCOVERY_JS;

    let raw = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let pairs: Vec<(String, String)> = raw
        .get("value")
        .and_then(|v| serde_json::from_value::<Vec<[String; 2]>>(v.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|[name, sel]| (name, sel))
        .collect();

    Ok(crate::semantic::match_text(label_query, &pairs, None).map(|(best, _)| best))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_fields() {
        let input = json!({
            "fields": {"#name": "John", "#email": "john@example.com"}
        });
        let result = parse_input(&input).unwrap();
        assert_eq!(result.fields.len(), 2);
        assert_eq!(result.fields["#name"], "John");
        assert_eq!(result.fields["#email"], "john@example.com");
        assert!(!result.submit);
        assert_eq!(result.form_selector, "form");
    }

    #[test]
    fn parse_with_submit_and_form_selector() {
        let input = json!({
            "fields": {"#q": "rust"},
            "submit": true,
            "form_selector": "#search-form"
        });
        let result = parse_input(&input).unwrap();
        assert!(result.submit);
        assert_eq!(result.form_selector, "#search-form");
    }

    #[test]
    fn parse_missing_fields_returns_error() {
        let input = json!({});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("fields"));
    }

    #[test]
    fn parse_empty_fields_returns_error() {
        let input = json!({"fields": {}});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn parse_non_object_fields_returns_error() {
        let input = json!({"fields": "not an object"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("object"));
    }

    #[test]
    fn parse_non_string_field_value_returns_error() {
        let input = json!({"fields": {"#name": 42}});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("string"));
    }

    #[test]
    fn parse_defaults_submit_false_and_form_selector() {
        let input = json!({"fields": {"#x": "y"}});
        let result = parse_input(&input).unwrap();
        assert!(!result.submit);
        assert_eq!(result.form_selector, "form");
    }

    #[test]
    fn fill_form_response_includes_page_state() {
        use serde_json::json;
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Filled 2 field(s) and submitted form",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
        assert!(!response["page_state"]["page_map"].is_null());
    }

    #[test]
    fn match_text_exact_wins_over_fuzzy() {
        let candidates = vec![
            ("Email address".to_string(), "#email".to_string()),
            ("Password".to_string(), "#pw".to_string()),
        ];
        let (best, _) = crate::semantic::match_text("Email address", &candidates, None).unwrap();
        assert_eq!(best, "#email");
    }

    #[test]
    fn match_text_case_insensitive_fallback() {
        let candidates = vec![("Email address".to_string(), "#email".to_string())];
        let (best, _) = crate::semantic::match_text("email address", &candidates, None).unwrap();
        assert_eq!(best, "#email");
    }

    #[test]
    fn match_text_contains_fallback() {
        let candidates = vec![("Email address".to_string(), "#email".to_string())];
        let (best, _) = crate::semantic::match_text("email", &candidates, None).unwrap();
        assert_eq!(best, "#email");
    }

    #[test]
    fn match_text_no_match_returns_none() {
        let candidates = vec![("Email address".to_string(), "#email".to_string())];
        assert!(crate::semantic::match_text("phone", &candidates, None).is_none());
    }

    #[test]
    fn match_text_ambiguous_returns_best_and_alternatives() {
        let candidates = vec![
            ("Name".to_string(), "#name-1".to_string()),
            ("Name".to_string(), "#name-2".to_string()),
        ];
        let (best, alternatives) = crate::semantic::match_text("Name", &candidates, None).unwrap();
        assert_eq!(best, "#name-1");
        assert_eq!(alternatives, vec!["#name-2".to_string()]);
    }

    #[test]
    fn field_discovery_js_includes_editor_surfaces() {
        assert!(
            FIELD_DISCOVERY_JS.contains(r#"[contenteditable="true"]"#),
            "Missing contenteditable selector"
        );
        assert!(
            FIELD_DISCOVERY_JS.contains(r"group.querySelector('textarea')"),
            "Missing sibling textarea label fallback"
        );
    }
}
