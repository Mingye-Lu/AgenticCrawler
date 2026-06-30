use serde_json::Value;

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

use super::feedback::InteractionKind;

#[derive(Debug)]
struct ClickInput {
    selector: Option<String>,
    text: Option<String>,
    role: Option<String>,
    region: Option<String>,
    widen: bool,
}

fn parse_input(input: &Value) -> Result<ClickInput, CrawlError> {
    let selector = input
        .get("selector")
        .and_then(Value::as_str)
        .map(str::to_string);
    let text = input
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string);

    match (&selector, &text) {
        (None, None) => return Err(CrawlError::new("either 'selector' or 'text' is required")),
        (Some(_), Some(_)) => {
            return Err(CrawlError::new(
                "'selector' and 'text' are mutually exclusive",
            ))
        }
        _ => {}
    }

    if let Some(ref s) = selector {
        if s.is_empty() {
            return Err(CrawlError::new("selector must not be empty"));
        }
    }

    Ok(ClickInput {
        selector,
        text,
        role: input
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string),
        region: input
            .get("region")
            .and_then(Value::as_str)
            .map(str::to_string),
        widen: input.get("widen").and_then(Value::as_bool).unwrap_or(false),
    })
}

async fn resolve_by_text(
    browser: &mut BrowserContext,
    query: &str,
    role_filter: Option<&str>,
    region: Option<&str>,
) -> Result<String, ToolExecutionError> {
    let scope_sel: Option<String> = match region {
        Some(r) => match super::ref_resolve::resolve_scope_ref(r, browser) {
            Ok(Some(query)) => Some(query),
            Err(message) => return Err(ToolExecutionError::new(message)),
            Ok(None) => match r {
                "dialog" => Some(
                    "[role=\"dialog\"],[role=\"alertdialog\"],[aria-modal=\"true\"]".to_string(),
                ),
                "main" => Some("main,[role=\"main\"]".to_string()),
                "sidebar" => Some("[role=\"complementary\"],aside".to_string()),
                other => Some(other.to_string()),
            },
        },
        None => None,
    };

    let scope_init = match &scope_sel {
        Some(s) => format!(
            r"(() => {{ const __scope = document.querySelector({s:?}); if (!__scope) throw new Error('region scope element not found: {s}'); return __scope; }})()"
        ),
        None => "document".to_string(),
    };

    let script = build_resolve_by_text_script(&scope_init);

    let raw = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let triples: Vec<[String; 3]> = raw
        .get("value")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let pairs: Vec<(String, String)> = triples
        .iter()
        .filter(|[_, _, role]| {
            role_filter.is_none_or(|rf| role.to_lowercase().contains(&rf.to_lowercase()))
        })
        .map(|[name, sel, _]| (name.clone(), sel.clone()))
        .collect();

    match crate::semantic::match_text(query, &pairs, None) {
        Some((best, _alternatives)) => Ok(best),
        None => Err(ToolExecutionError::new(format!(
            "no element found with text '{query}'{}",
            role_filter
                .map(|r| format!(" and role '{r}'"))
                .unwrap_or_default()
        ))),
    }
}

fn build_resolve_by_text_script(scope_init: &str) -> String {
    format!(
        r#"(() => {{
        function selectorOf(el) {{
            if (el.id) return '#' + CSS.escape(el.id);
            const path = [];
            let cur = el;
            while (cur && cur.parentElement) {{
                if (cur.id) {{ path.unshift('#' + CSS.escape(cur.id)); break; }}
                const parent = cur.parentElement;
                const tag = cur.tagName.toLowerCase();
                const same = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
                path.unshift(same.length > 1 ? tag + ':nth-of-type(' + (same.indexOf(cur) + 1) + ')' : tag);
                cur = parent;
            }}
            return path.join(' > ');
        }}
        const root = {scope_init};
        const sels = [
            'button','a[href]','[role="button"]','[role="tab"]',
            '[role="menuitem"]','[role="checkbox"]','[role="switch"]',
            '[role="link"]','input[type="button"]','input[type="submit"]',
            'input[type="checkbox"]','input[type="radio"]'
        ];
        const candidates = [];
        for (const el of root.querySelectorAll(sels.join(','))) {{
            let name = el.getAttribute('aria-label') || '';
            if (!name) {{
                const lby = el.getAttribute('aria-labelledby');
                if (lby) name = lby
                    .split(/\s+/)
                    .filter(Boolean)
                    .map(id => document.getElementById(id)?.innerText?.trim() || '')
                    .filter(Boolean)
                    .join(' ')
                    .trim();
            }}
            if (!name && el.id) {{
                const lbl = document.querySelector('label[for="' + CSS.escape(el.id) + '"]');
                if (lbl) name = (lbl.innerText || '').trim();
            }}
            if (!name) {{
                const wrapping = el.closest('label');
                if (wrapping) name = (wrapping.innerText || '').replace((el.textContent || '').trim(), '').trim();
            }}
            if (!name) name = (el.innerText || '').trim();
            if (!name) name = el.title || el.placeholder || '';
            if (!name) continue;
            const roleAttr = el.getAttribute('role');
            const inputType = (el.type || '').toLowerCase();
            const role = roleAttr ||
                (el.tagName === 'INPUT' && inputType === 'checkbox' ? 'checkbox' :
                 el.tagName === 'INPUT' && inputType === 'radio' ? 'radio' :
                 el.tagName === 'INPUT' && ['button','submit','reset','image'].includes(inputType) ? 'button' :
                 el.tagName === 'A' ? 'link' : el.tagName.toLowerCase());
            candidates.push([name.slice(0, 80), selectorOf(el), role]);
        }}
        return candidates;
    }})()"#
    )
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_input(input)?;

    let selector = if let Some(ref sel) = params.selector {
        super::ref_resolve::resolve_selector(sel, browser.ref_map())
            .map_err(ToolExecutionError::new)?
    } else {
        let text = params.text.as_deref().unwrap_or_default();
        resolve_by_text(
            browser,
            text,
            params.role.as_deref(),
            params.region.as_deref(),
        )
        .await?
    };

    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .click(&selector)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(
        browser,
        crawl_state,
        InteractionKind::PossibleSubmit,
        Some(&selector),
        params.widen,
    )
    .await?;

    let display_target = params.text.as_deref().map_or_else(
        || params.selector.clone().unwrap_or_default(),
        |t| format!("text: {t}"),
    );

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "seq": seq,
        "success": true,
        "message": format!("Clicked element: {display_target}"),
        "page_state": page_state
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_selector() {
        let input = json!({"selector": ".btn-primary"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.selector, Some(".btn-primary".to_string()));
        assert!(result.text.is_none());
    }

    #[test]
    fn parse_valid_text() {
        let input = json!({"text": "Submit"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.text, Some("Submit".to_string()));
        assert!(result.selector.is_none());
    }

    #[test]
    fn parse_text_with_role_and_region() {
        let input = json!({"text": "Workers", "role": "tab", "region": "[ref=e3]"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.text, Some("Workers".to_string()));
        assert_eq!(result.role, Some("tab".to_string()));
        assert_eq!(result.region, Some("[ref=e3]".to_string()));
    }

    #[test]
    fn parse_rejects_both_selector_and_text() {
        let input = json!({"selector": "#btn", "text": "Submit"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn parse_rejects_neither_selector_nor_text() {
        let input = json!({});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("required"));
    }

    #[test]
    fn parse_missing_selector_returns_error() {
        let input = json!({});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("required") || err.to_string().contains("selector"));
    }

    #[test]
    fn parse_empty_selector_returns_error() {
        let input = json!({"selector": ""});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn parse_non_string_selector_returns_error() {
        let input = json!({"selector": 123});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn parse_complex_selector() {
        let input = json!({"selector": "div.container > ul > li:nth-child(2) a"});
        let result = parse_input(&input).unwrap();
        assert_eq!(
            result.selector,
            Some("div.container > ul > li:nth-child(2) a".to_string())
        );
    }

    #[test]
    fn match_text_semantics_exact_wins() {
        let candidates = vec![
            ("Submit".to_string(), "#submit".to_string()),
            ("Cancel".to_string(), "#cancel".to_string()),
        ];
        let (best, _) = crate::semantic::match_text("Submit", &candidates, None).unwrap();
        assert_eq!(best, "#submit");
    }

    #[test]
    fn match_text_semantics_case_insensitive() {
        let candidates = vec![("Submit Form".to_string(), "#submit".to_string())];
        let (best, _) = crate::semantic::match_text("submit form", &candidates, None).unwrap();
        assert_eq!(best, "#submit");
    }

    #[test]
    fn match_text_semantics_contains_fallback() {
        let candidates = vec![("Email address".to_string(), "#email".to_string())];
        let (best, _) = crate::semantic::match_text("email", &candidates, None).unwrap();
        assert_eq!(best, "#email");
    }

    #[test]
    fn match_text_semantics_no_match() {
        let candidates = vec![("Email address".to_string(), "#email".to_string())];
        assert!(crate::semantic::match_text("phone", &candidates, None).is_none());
    }

    #[test]
    fn click_response_includes_page_state() {
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Clicked element: .btn",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
    }
}
