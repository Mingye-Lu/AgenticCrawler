use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::semantic::{compute_accessible_name, OptionCandidate, RawElementFacts};
use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

use super::feedback::InteractionKind;

const OPEN_WAIT: Duration = Duration::from_millis(300);
const VERIFY_WAIT: Duration = Duration::from_millis(200);
const MAX_OPTION_ATTEMPTS: usize = 5;
const OPEN_KEYS: [&str; 3] = ["Enter", "Space", "ArrowDown"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOptionInput {
    pub selector: String,
    pub value: Option<String>,
    pub label: Option<String>,
    pub index: Option<usize>,
    pub widen: bool,
}

pub fn parse_input(input: &Value) -> Result<SelectOptionInput, CrawlError> {
    let selector = input
        .get("selector")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: selector"))?;

    if selector.trim().is_empty() {
        return Err(CrawlError::new("selector must not be empty"));
    }

    let index = match input.get("index") {
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or_else(|| CrawlError::new("index must be a non-negative integer"))?;
            Some(
                usize::try_from(raw)
                    .map_err(|_| CrawlError::new("index is too large for this platform"))?,
            )
        }
        None => None,
    };

    Ok(SelectOptionInput {
        selector: selector.to_string(),
        value: input
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string),
        label: input
            .get("label")
            .and_then(Value::as_str)
            .map(str::to_string),
        index,
        widen: input.get("widen").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_input(input)?;
    let selector = super::ref_resolve::resolve_selector(&params.selector, browser.ref_map())
        .map_err(ToolExecutionError::new)?;

    if check_is_native_select(browser, &selector).await? {
        return execute_native(&params, &selector, browser, crawl_state).await;
    }

    execute_custom(&params, &selector, browser, crawl_state).await
}

async fn execute_native(
    params: &SelectOptionInput,
    selector: &str,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let options = get_native_options(browser, selector).await?;

    let selected = if let Some(value) = params.value.as_deref() {
        browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .select_option(selector, value)
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;
        Some(value.to_string())
    } else if let Some(label) = params.label.as_deref() {
        select_native_by_label(browser, selector, label).await?;
        Some(label.to_string())
    } else if let Some(index) = params.index {
        let option = options.get(index).ok_or_else(|| {
            ToolExecutionError::new(format!(
                "option index {index} is out of range; available indices: 0..{}",
                options.len().saturating_sub(1)
            ))
        })?;
        select_native_by_index(browser, selector, index).await?;
        Some(option.name.clone())
    } else {
        let seq = super::seq::increment_seq(crawl_state, browser).await;
        let page_state = super::feedback::post_action_page_state(
            browser,
            crawl_state,
            InteractionKind::Passive,
            Some(selector),
            params.widen,
        )
        .await?;
        return Ok(ToolEffect::reply_json(&json!({
            "seq": seq,
            "success": true,
            "mode": "list",
            "options": option_candidates_json(&options),
            "page_state": page_state,
        })));
    };

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(
        browser,
        crawl_state,
        InteractionKind::Passive,
        Some(selector),
        params.widen,
    )
    .await?;

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": true,
        "selected": selected,
        "options": option_candidates_json(&options),
        "page_state": page_state,
    })))
}

async fn execute_custom(
    params: &SelectOptionInput,
    selector: &str,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let before_snapshot = extract_dom_snapshot(browser).await?;
    let before = snapshot_elements(&before_snapshot)?;

    open_custom_dropdown(browser, selector, &before).await?;
    let options = locate_and_enumerate_options(browser, selector, &before).await?;

    if options.is_empty() {
        return Err(ToolExecutionError::new(
            "dropdown opened, but no visible options were found",
        ));
    }

    if params.value.is_none() && params.label.is_none() && params.index.is_none() {
        let seq = super::seq::increment_seq(crawl_state, browser).await;
        let page_state = super::feedback::post_action_page_state(
            browser,
            crawl_state,
            InteractionKind::Passive,
            Some(selector),
            params.widen,
        )
        .await?;
        return Ok(ToolEffect::reply_json(&json!({
            "seq": seq,
            "success": true,
            "mode": "list",
            "options": option_candidates_json(&options),
            "page_state": page_state,
        })));
    }

    let (target_option, target_text, target_index) = select_target_option(params, &options)?;
    let keyboard_selected =
        try_keyboard_select(browser, selector, &target_option.selector, target_index).await?;

    if !keyboard_selected {
        browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .click(&target_option.selector)
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;
    }

    tokio::time::sleep(VERIFY_WAIT).await;
    let verified =
        verify_custom_selection(browser, selector, &target_option.selector, &target_text).await?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(
        browser,
        crawl_state,
        InteractionKind::Passive,
        Some(selector),
        params.widen,
    )
    .await?;

    if verified {
        return Ok(ToolEffect::reply_json(&json!({
            "seq": seq,
            "success": true,
            "selected": target_text,
            "options": option_candidates_json(&options),
            "page_state": page_state,
        })));
    }

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": false,
        "selected": target_text,
        "message": "Selection could not be verified; returning visible options for retry.",
        "options": option_candidates_json(&options),
        "page_state": page_state,
    })))
}

async fn check_is_native_select(
    browser: &mut BrowserContext,
    selector: &str,
) -> Result<bool, ToolExecutionError> {
    let selector_json = js_string(selector)?;
    let script = format!(
        r"(() => {{
            const el = document.querySelector({selector_json});
            return el ? el.tagName : null;
        }})()"
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(evaluate_payload(&result)
        .as_str()
        .is_some_and(|tag| tag.eq_ignore_ascii_case("select")))
}

async fn select_native_by_label(
    browser: &mut BrowserContext,
    selector: &str,
    label: &str,
) -> Result<(), ToolExecutionError> {
    let selector_json = js_string(selector)?;
    let label_json = js_string(label)?;
    let script = format!(
        r"(() => {{
            const select = document.querySelector({selector_json});
            if (!select) return {{ ok: false, error: 'select_not_found' }};
            if (select.tagName !== 'SELECT') return {{ ok: false, error: 'not_select' }};

            const normalizedTarget = {label_json}.trim().toLowerCase();
            const index = Array.from(select.options).findIndex((option) =>
                (option.textContent || '').trim().toLowerCase() === normalizedTarget
            );

            if (index < 0) return {{ ok: false, error: 'label_not_found' }};

            select.selectedIndex = index;
            select.dispatchEvent(new Event('input', {{ bubbles: true }}));
            select.dispatchEvent(new Event('change', {{ bubbles: true }}));
            return {{ ok: true }};
        }})()"
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    if evaluate_payload(&result)
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(());
    }

    let error = evaluate_payload(&result)
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("label_not_found");
    Err(ToolExecutionError::new(format!(
        "native select option with label '{label}' not found ({error})"
    )))
}

async fn select_native_by_index(
    browser: &mut BrowserContext,
    selector: &str,
    index: usize,
) -> Result<(), ToolExecutionError> {
    let selector_json = js_string(selector)?;
    let script = format!(
        r"(() => {{
            const select = document.querySelector({selector_json});
            if (!select) return {{ ok: false, error: 'select_not_found' }};
            if (select.tagName !== 'SELECT') return {{ ok: false, error: 'not_select' }};
            const index = {index};
            if (index < 0 || index >= select.options.length) {{
                return {{ ok: false, error: 'index_out_of_range' }};
            }}

            select.selectedIndex = index;
            select.dispatchEvent(new Event('input', {{ bubbles: true }}));
            select.dispatchEvent(new Event('change', {{ bubbles: true }}));
            return {{ ok: true }};
        }})()"
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    if evaluate_payload(&result)
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(());
    }

    let error = evaluate_payload(&result)
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("index_out_of_range");
    Err(ToolExecutionError::new(format!(
        "native select option index {index} could not be selected ({error})"
    )))
}

async fn get_native_options(
    browser: &mut BrowserContext,
    selector: &str,
) -> Result<Vec<OptionCandidate>, ToolExecutionError> {
    let selector_json = js_string(selector)?;
    let script = format!(
        r"(() => {{
            const select = document.querySelector({selector_json});
            if (!select || select.tagName !== 'SELECT') return [];

            return Array.from(select.options).map((option, index) => {{
                const optionSelector = option.id
                    ? '#' + CSS.escape(option.id)
                    : {selector_json} + ' > option:nth-of-type(' + (index + 1) + ')';
                return {{
                    name: (option.textContent || '').trim(),
                    selector: optionSelector,
                    role: 'option',
                    aria_selected: option.selected ? 'true' : null,
                    disabled: option.disabled,
                }};
            }});
        }})()"
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(parse_option_candidates(&result))
}

async fn open_custom_dropdown(
    browser: &mut BrowserContext,
    selector: &str,
    before: &[RawElementFacts],
) -> Result<(), ToolExecutionError> {
    let already_open = trigger_facts_for_selector(selector, before)
        .and_then(|facts| facts.aria_expanded.as_deref())
        .is_some_and(|value| value == "true");
    if already_open {
        return Ok(());
    }

    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .click(selector)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    tokio::time::sleep(OPEN_WAIT).await;
    let after = snapshot_elements(&extract_dom_snapshot(browser).await?)?;
    if detect_dropdown_opened_from_snapshots(before, &after) {
        return Ok(());
    }

    for key in OPEN_KEYS {
        browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .press_key(key, Some(selector))
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;

        tokio::time::sleep(OPEN_WAIT).await;
        let after = snapshot_elements(&extract_dom_snapshot(browser).await?)?;
        if detect_dropdown_opened_from_snapshots(before, &after) {
            break;
        }
    }

    Ok(())
}

fn detect_dropdown_opened_from_snapshots(
    before: &[RawElementFacts],
    after: &[RawElementFacts],
) -> bool {
    let before_selectors = before
        .iter()
        .map(|facts| facts.selector.as_str())
        .collect::<HashSet<_>>();

    let expanded_flip = after.iter().any(|facts| {
        if facts.aria_expanded.as_deref() != Some("true") {
            return false;
        }

        before
            .iter()
            .find(|prior| prior.selector == facts.selector)
            .and_then(|prior| prior.aria_expanded.as_deref())
            != Some("true")
    });
    if expanded_flip {
        return true;
    }

    let new_container = after.iter().any(|facts| {
        is_visible_container(facts) && !before_selectors.contains(facts.selector.as_str())
    });
    if new_container {
        return true;
    }

    let new_option = after.iter().any(|facts| {
        is_visible_option(facts) && !before_selectors.contains(facts.selector.as_str())
    });
    if new_option {
        return true;
    }

    after.iter().any(|facts| {
        facts.visible && facts.floating && !before_selectors.contains(facts.selector.as_str())
    })
}

async fn locate_and_enumerate_options(
    browser: &mut BrowserContext,
    selector: &str,
    before: &[RawElementFacts],
) -> Result<Vec<OptionCandidate>, ToolExecutionError> {
    let mut last_options = Vec::new();

    for attempt in 0..MAX_OPTION_ATTEMPTS {
        let after_snapshot = extract_dom_snapshot(browser).await?;
        let after = snapshot_elements(&after_snapshot)?;

        let scopes = option_search_scopes(selector, before, &after);
        for scope in scopes {
            let mut options = enumerate_options_in_scope(browser, scope.as_deref()).await?;
            if options.is_empty() {
                if let Some(scope_selector) = scope.as_deref() {
                    if scroll_scope_once(browser, scope_selector).await? {
                        tokio::time::sleep(OPEN_WAIT).await;
                        options = enumerate_options_in_scope(browser, Some(scope_selector)).await?;
                    }
                } else {
                    options = snapshot_option_candidates(&after);
                }
            }

            if !options.is_empty() {
                return Ok(options);
            }
            last_options = options;
        }

        if attempt + 1 < MAX_OPTION_ATTEMPTS {
            tokio::time::sleep(OPEN_WAIT).await;
        }
    }

    Ok(last_options)
}

fn option_search_scopes(
    selector: &str,
    before: &[RawElementFacts],
    after: &[RawElementFacts],
) -> Vec<Option<String>> {
    let mut scopes = Vec::new();
    let mut seen = HashSet::new();
    let mut has_document_scope = false;
    let before_visible = before
        .iter()
        .filter(|facts| facts.visible)
        .map(|facts| facts.selector.as_str())
        .collect::<HashSet<_>>();

    let trigger = trigger_facts_for_selector(selector, after)
        .or_else(|| trigger_facts_for_selector(selector, before));

    if let Some(trigger) = trigger {
        for id in controlled_ids(trigger) {
            add_scope(
                &mut scopes,
                &mut seen,
                &mut has_document_scope,
                Some(format!("#{id}")),
            );
        }
    }

    for facts in after.iter().filter(|facts| is_visible_container(facts)) {
        add_scope(
            &mut scopes,
            &mut seen,
            &mut has_document_scope,
            Some(facts.selector.clone()),
        );
    }

    for facts in after.iter().filter(|facts| {
        facts.visible && facts.floating && !before_visible.contains(facts.selector.as_str())
    }) {
        add_scope(
            &mut scopes,
            &mut seen,
            &mut has_document_scope,
            Some(facts.selector.clone()),
        );
    }

    add_scope(&mut scopes, &mut seen, &mut has_document_scope, None);
    scopes
}

fn add_scope(
    scopes: &mut Vec<Option<String>>,
    seen: &mut HashSet<String>,
    has_document_scope: &mut bool,
    scope: Option<String>,
) {
    match scope {
        Some(selector) if seen.insert(selector.clone()) => scopes.push(Some(selector)),
        None if !*has_document_scope => {
            *has_document_scope = true;
            scopes.push(None);
        }
        Some(_) | None => {}
    }
}

async fn enumerate_options_in_scope(
    browser: &mut BrowserContext,
    scope: Option<&str>,
) -> Result<Vec<OptionCandidate>, ToolExecutionError> {
    let scope_json = match scope {
        Some(value) => js_string(value)?,
        None => "null".to_string(),
    };
    let script = format!(
        r#"(() => {{
            const scopeSelector = {scope_json};
            const root = scopeSelector ? document.querySelector(scopeSelector) : document;
            if (!root) return [];

            function cssPath(el) {{
                if (el.id) return '#' + CSS.escape(el.id);
                const parts = [];
                let cur = el;
                while (cur && cur !== document.body && cur !== document.documentElement) {{
                    let seg = cur.tagName.toLowerCase();
                    const parent = cur.parentElement;
                    if (parent) {{
                        const sibs = Array.from(parent.children).filter((child) => child.tagName === cur.tagName);
                        if (sibs.length > 1) seg += ':nth-of-type(' + (sibs.indexOf(cur) + 1) + ')';
                    }}
                    parts.unshift(seg);
                    cur = cur.parentElement;
                }}
                return parts.join(' > ');
            }}

            function isVisible(el) {{
                if (el.hidden) return false;
                const style = getComputedStyle(el);
                if (style.display === 'none' || style.visibility === 'hidden') return false;
                const rect = el.getBoundingClientRect();
                return rect.width > 0 || rect.height > 0;
            }}

            function labelledByText(el) {{
                const labelledBy = el.getAttribute('aria-labelledby');
                if (!labelledBy) return '';
                return labelledBy
                    .split(/\s+/)
                    .filter(Boolean)
                    .map((id) => document.getElementById(id)?.innerText?.trim() || '')
                    .filter(Boolean)
                    .join(' ')
                    .trim();
            }}

            return Array.from(root.querySelectorAll('[role="option"], [role="menuitem"], [role="treeitem"], li'))
                .filter((el) => isVisible(el))
                .map((el) => {{
                    const name = (
                        (el.innerText || '').trim() ||
                        el.getAttribute('aria-label') ||
                        labelledByText(el) ||
                        el.getAttribute('title') ||
                        ''
                    ).trim().slice(0, 120);
                    return {{
                        name,
                        selector: cssPath(el),
                        role: el.getAttribute('role'),
                        aria_selected: el.getAttribute('aria-selected'),
                        disabled: el.matches('[disabled], [aria-disabled="true"]'),
                    }};
                }})
                .filter((option) => option.name && option.selector);
        }})()"#
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(parse_option_candidates(&result))
}

async fn scroll_scope_once(
    browser: &mut BrowserContext,
    scope: &str,
) -> Result<bool, ToolExecutionError> {
    let scope_json = js_string(scope)?;
    let script = format!(
        r"(() => {{
            const root = document.querySelector({scope_json});
            if (!root) return false;
            root.scrollTop = root.scrollHeight;
            return true;
        }})()"
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(evaluate_payload(&result).as_bool().unwrap_or(false))
}

fn select_target_option<'a>(
    params: &'a SelectOptionInput,
    options: &'a [OptionCandidate],
) -> Result<(&'a OptionCandidate, String, usize), ToolExecutionError> {
    if let Some(index) = params.index {
        let option = options.get(index).ok_or_else(|| {
            ToolExecutionError::new(format!(
                "option index {index} is out of range; available indices: 0..{}",
                options.len().saturating_sub(1)
            ))
        })?;
        return Ok((option, option.name.clone(), index));
    }

    let target_text = params
        .label
        .as_deref()
        .or(params.value.as_deref())
        .unwrap_or_default()
        .to_string();

    let pairs = options
        .iter()
        .map(|option| (option.name.clone(), option.selector.clone()))
        .collect::<Vec<_>>();
    let (best_selector, _) =
        crate::semantic::match_text(&target_text, &pairs, None).ok_or_else(|| {
            ToolExecutionError::new(format!(
                "option '{target_text}' not found. Available: {:?}",
                options
                    .iter()
                    .map(|option| option.name.as_str())
                    .collect::<Vec<_>>()
            ))
        })?;

    let index = options
        .iter()
        .position(|option| option.selector == best_selector)
        .ok_or_else(|| {
            ToolExecutionError::new("matched option selector disappeared before selection")
        })?;

    Ok((&options[index], target_text, index))
}

async fn try_keyboard_select(
    browser: &mut BrowserContext,
    trigger_selector: &str,
    target_selector: &str,
    target_index: usize,
) -> Result<bool, ToolExecutionError> {
    if is_target_active(browser, trigger_selector, target_selector).await? {
        browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .press_key("Enter", Some(trigger_selector))
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;
        return Ok(true);
    }

    for _ in 0..=target_index {
        browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .press_key("ArrowDown", Some(trigger_selector))
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;
        tokio::time::sleep(VERIFY_WAIT).await;

        if is_target_active(browser, trigger_selector, target_selector).await? {
            browser
                .acquire_bridge()
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?
                .press_key("Enter", Some(trigger_selector))
                .await
                .map_err(|e| ToolExecutionError::new(e.to_string()))?;
            return Ok(true);
        }
    }

    Ok(false)
}

async fn is_target_active(
    browser: &mut BrowserContext,
    trigger_selector: &str,
    target_selector: &str,
) -> Result<bool, ToolExecutionError> {
    let trigger_json = js_string(trigger_selector)?;
    let target_json = js_string(target_selector)?;
    let script = format!(
        r"(() => {{
            const trigger = document.querySelector({trigger_json});
            const target = document.querySelector({target_json});
            if (!trigger || !target) return false;

            const activeId = trigger.getAttribute('aria-activedescendant');
            if (activeId && target.id && activeId === target.id) return true;
            if (target.getAttribute('aria-selected') === 'true') return true;
            return document.activeElement === target;
        }})()"
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(evaluate_payload(&result).as_bool().unwrap_or(false))
}

async fn verify_custom_selection(
    browser: &mut BrowserContext,
    trigger_selector: &str,
    target_selector: &str,
    target_text: &str,
) -> Result<bool, ToolExecutionError> {
    let trigger_json = js_string(trigger_selector)?;
    let target_json = js_string(target_selector)?;
    let target_text_json = js_string(target_text)?;
    let script = format!(
        r"(() => {{
            const trigger = document.querySelector({trigger_json});
            const target = document.querySelector({target_json});
            const targetText = {target_text_json}.trim().toLowerCase();
            if (!trigger) return false;

            const triggerTexts = [
                trigger.innerText || '',
                trigger.textContent || '',
                trigger.getAttribute('aria-label') || '',
                'value' in trigger ? String(trigger.value || '') : ''
            ]
                .map((value) => value.trim().toLowerCase())
                .filter(Boolean);
            if (triggerTexts.some((value) => value.includes(targetText))) return true;

            if (target && target.getAttribute('aria-selected') === 'true') return true;

            return false;
        }})()"
    );

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    Ok(evaluate_payload(&result).as_bool().unwrap_or(false))
}

async fn extract_dom_snapshot(browser: &mut BrowserContext) -> Result<Value, ToolExecutionError> {
    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .extract_dom_snapshot(None)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))
}

fn snapshot_elements(snapshot: &Value) -> Result<Vec<RawElementFacts>, ToolExecutionError> {
    serde_json::from_value(
        snapshot
            .get("elements")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|e| ToolExecutionError::new(format!("failed to parse DOM snapshot: {e}")))
}

fn trigger_facts_for_selector<'a>(
    selector: &str,
    facts: &'a [RawElementFacts],
) -> Option<&'a RawElementFacts> {
    facts.iter().find(|facts| facts.selector == selector)
}

fn controlled_ids(trigger: &RawElementFacts) -> Vec<String> {
    [
        trigger.aria_controls.as_deref(),
        trigger.aria_owns.as_deref(),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split_whitespace())
    .filter(|value| !value.is_empty())
    .map(str::to_string)
    .collect()
}

fn snapshot_option_candidates(facts: &[RawElementFacts]) -> Vec<OptionCandidate> {
    facts
        .iter()
        .filter(|facts| is_visible_option(facts))
        .map(|facts| OptionCandidate {
            name: compute_accessible_name(facts),
            selector: facts.selector.clone(),
            role: facts.role.clone(),
            aria_selected: facts.aria_selected.clone(),
            disabled: false,
        })
        .filter(|option| !option.name.is_empty())
        .collect()
}

fn parse_option_candidates(raw: &Value) -> Vec<OptionCandidate> {
    evaluate_payload(raw)
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(Value::as_str)?.trim().to_string();
                    let selector = item
                        .get("selector")
                        .and_then(Value::as_str)?
                        .trim()
                        .to_string();
                    if name.is_empty() || selector.is_empty() {
                        return None;
                    }

                    Some(OptionCandidate {
                        name,
                        selector,
                        role: item.get("role").and_then(Value::as_str).map(str::to_string),
                        aria_selected: item
                            .get("aria_selected")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        disabled: item
                            .get("disabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn option_candidates_json(options: &[OptionCandidate]) -> Value {
    Value::Array(
        options
            .iter()
            .map(|option| {
                json!({
                    "name": option.name,
                    "selector": option.selector,
                    "role": option.role,
                    "aria_selected": option.aria_selected,
                    "disabled": option.disabled,
                })
            })
            .collect(),
    )
}

fn evaluate_payload(value: &Value) -> &Value {
    value.get("value").unwrap_or(value)
}

fn js_string(value: &str) -> Result<String, ToolExecutionError> {
    serde_json::to_string(value).map_err(|e| ToolExecutionError::new(e.to_string()))
}

fn is_visible_container(facts: &RawElementFacts) -> bool {
    facts.visible && matches!(facts.role.as_deref(), Some("listbox" | "menu" | "menubar"))
}

fn is_visible_option(facts: &RawElementFacts) -> bool {
    facts.visible
        && matches!(
            facts.role.as_deref(),
            Some("option" | "menuitem" | "treeitem")
        )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use browser::{
        BridgeError, BrowserBackend, BrowserState, ObservationEvent, PageInfo, ScreenshotOptions,
        SharedBridge, StorageEntry, StorageType,
    };
    use serde_json::json;
    use tokio::sync::Mutex as AsyncMutex;

    use super::*;

    #[derive(Debug, Default)]
    struct MockState {
        evaluate_results: VecDeque<Value>,
        snapshot_results: VecDeque<Value>,
        clicks: Vec<String>,
        keypresses: Vec<(String, Option<String>)>,
        native_selects: Vec<(String, String)>,
        evaluate_scripts: Vec<String>,
    }

    #[derive(Debug, Clone)]
    struct MockBackend {
        state: Arc<Mutex<MockState>>,
    }

    impl MockBackend {
        fn new(state: Arc<Mutex<MockState>>) -> Self {
            Self { state }
        }
    }

    #[async_trait]
    impl BrowserBackend for MockBackend {
        async fn navigate(&mut self, _url: &str) -> Result<PageInfo, BridgeError> {
            Ok(PageInfo {
                title: "Test".to_string(),
                html: String::new(),
            })
        }

        async fn new_page(&mut self, _url: Option<&str>) -> Result<usize, BridgeError> {
            Ok(0)
        }

        async fn close_page(&mut self, _page_index: usize) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn scroll(&mut self, _direction: &str, _pixels: i64) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn page_map(
            &mut self,
            _scope: Option<&str>,
            _compound_enrichment: bool,
        ) -> Result<Value, BridgeError> {
            Ok(json!({
                "headings": [],
                "landmarks": [],
                "forms": [],
                "links": [],
                "interactive": {},
                "meta": {"title": "Test", "url": "https://example.com", "description": ""}
            }))
        }

        async fn read_content(
            &mut self,
            _heading: Option<&str>,
            _selector: Option<&str>,
            _offset: usize,
            _max_chars: usize,
        ) -> Result<Value, BridgeError> {
            Ok(json!({}))
        }

        async fn wait_for_selector(
            &mut self,
            _selector: &str,
            _timeout_ms: u64,
            _state: Option<&str>,
        ) -> Result<bool, BridgeError> {
            Ok(true)
        }

        async fn select_option(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
            self.state
                .lock()
                .unwrap()
                .native_selects
                .push((selector.to_string(), value.to_string()));
            Ok(())
        }

        async fn evaluate(&mut self, script: &str) -> Result<Value, BridgeError> {
            let mut state = self.state.lock().unwrap();
            state.evaluate_scripts.push(script.to_string());
            Ok(state.evaluate_results.pop_front().unwrap_or(Value::Null))
        }

        async fn hover(&mut self, _selector: &str) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn press_key(
            &mut self,
            key: &str,
            selector: Option<&str>,
        ) -> Result<(), BridgeError> {
            self.state
                .lock()
                .unwrap()
                .keypresses
                .push((key.to_string(), selector.map(str::to_string)));
            Ok(())
        }

        async fn switch_tab(&mut self, _index: i64) -> Result<Value, BridgeError> {
            Ok(json!({"ok": true}))
        }

        async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
            Ok(BrowserState {
                cookies: Value::Array(Vec::new()),
                local_storage: Value::Object(serde_json::Map::new()),
                url: String::new(),
            })
        }

        async fn import_cookies(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn import_cookies_only(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn import_local_storage(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn list_resources(&mut self) -> Result<Value, BridgeError> {
            Ok(json!([]))
        }

        async fn save_file(
            &mut self,
            _url: &str,
            _path: &str,
            _headers: Option<&std::collections::BTreeMap<String, String>>,
        ) -> Result<String, BridgeError> {
            Ok(String::new())
        }

        async fn click(&mut self, selector: &str) -> Result<(), BridgeError> {
            self.state.lock().unwrap().clicks.push(selector.to_string());
            Ok(())
        }

        async fn click_at(&mut self, _x: f64, _y: f64) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn fill(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn screenshot(
            &mut self,
            _options: &ScreenshotOptions<'_>,
        ) -> Result<(String, usize), BridgeError> {
            Ok((String::new(), 0))
        }

        async fn go_back(&mut self) -> Result<String, BridgeError> {
            Ok(String::new())
        }

        async fn set_device(&mut self, _options: &Value) -> Result<Value, BridgeError> {
            Ok(json!({}))
        }

        async fn extract_dom_snapshot(
            &mut self,
            _scope: Option<&str>,
        ) -> Result<Value, BridgeError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .snapshot_results
                .pop_front()
                .unwrap_or_else(|| json!({"elements": []})))
        }

        async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
            Ok(Vec::new())
        }

        async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn get_storage(
            &mut self,
            _storage_type: StorageType,
        ) -> Result<(Vec<StorageEntry>, Vec<StorageEntry>), BridgeError> {
            Ok((Vec::new(), Vec::new()))
        }
    }

    fn make_browser(state: Arc<Mutex<MockState>>) -> BrowserContext {
        let bridge: SharedBridge = Arc::new(AsyncMutex::new(
            Box::new(MockBackend::new(state)) as Box<dyn BrowserBackend + Send>
        ));
        BrowserContext::new(bridge)
    }

    fn effect_json(effect: ToolEffect) -> Value {
        match effect {
            ToolEffect::Reply(body) => serde_json::from_str(&body).unwrap(),
            other => panic!("expected reply effect, got {other:?}"),
        }
    }

    fn snapshot(elements: Vec<Value>) -> Value {
        Value::Object(
            [("elements".to_string(), Value::Array(elements))]
                .into_iter()
                .collect(),
        )
    }

    #[test]
    fn parses_selector_and_value() {
        let input = json!({"selector": "#country", "value": "US"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.selector, "#country");
        assert_eq!(parsed.value.as_deref(), Some("US"));
        assert_eq!(parsed.label, None);
        assert_eq!(parsed.index, None);
    }

    #[test]
    fn accepts_label_as_value() {
        let input = json!({"selector": "select.lang", "label": "English"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.value, None);
        assert_eq!(parsed.label.as_deref(), Some("English"));
    }

    #[test]
    fn accepts_index_and_list_mode() {
        let with_index = parse_input(&json!({"selector": "#country", "index": 2})).unwrap();
        assert_eq!(with_index.index, Some(2));

        let list_mode = parse_input(&json!({"selector": "#country"})).unwrap();
        assert_eq!(list_mode.value, None);
        assert_eq!(list_mode.label, None);
        assert_eq!(list_mode.index, None);
    }

    #[test]
    fn fails_without_selector() {
        let input = json!({"value": "US"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn select_option_response_includes_page_state() {
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Selected 'US' in #country",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
        assert!(!response["page_state"]["page_map"].is_null());
    }

    #[tokio::test]
    async fn check_is_native_select_recognizes_select_tag() {
        let state = Arc::new(Mutex::new(MockState {
            evaluate_results: VecDeque::from(vec![json!({"value": "SELECT"})]),
            ..MockState::default()
        }));
        let mut browser = make_browser(state);

        assert!(check_is_native_select(&mut browser, "#country")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn check_is_native_select_rejects_non_select_tag() {
        let state = Arc::new(Mutex::new(MockState {
            evaluate_results: VecDeque::from(vec![json!({"value": "DIV"})]),
            ..MockState::default()
        }));
        let mut browser = make_browser(state);

        assert!(!check_is_native_select(&mut browser, "#country")
            .await
            .unwrap());
    }

    #[test]
    fn detect_dropdown_opened_on_all_supported_signals() {
        let before = vec![RawElementFacts {
            tag: "button".to_string(),
            role: Some("combobox".to_string()),
            aria_expanded: Some("false".to_string()),
            aria_selected: None,
            aria_pressed: None,
            aria_controls: Some("city-list".to_string()),
            aria_owns: None,
            text: Some("Choose city".to_string()),
            aria_label: None,
            aria_labelledby_text: None,
            title: None,
            placeholder: None,
            name: None,
            visible: true,
            floating: false,
            selector: "#city-trigger".to_string(),
        }];

        let after_aria = vec![RawElementFacts {
            aria_expanded: Some("true".to_string()),
            ..before[0].clone()
        }];
        assert!(detect_dropdown_opened_from_snapshots(&before, &after_aria));

        let after_listbox = vec![
            before[0].clone(),
            RawElementFacts {
                tag: "div".to_string(),
                role: Some("listbox".to_string()),
                aria_expanded: None,
                aria_selected: None,
                aria_pressed: None,
                aria_controls: None,
                aria_owns: None,
                text: None,
                aria_label: None,
                aria_labelledby_text: None,
                title: None,
                placeholder: None,
                name: None,
                visible: true,
                floating: false,
                selector: "#city-list".to_string(),
            },
        ];
        assert!(detect_dropdown_opened_from_snapshots(
            &before,
            &after_listbox
        ));

        let after_option = vec![
            before[0].clone(),
            RawElementFacts {
                tag: "div".to_string(),
                role: Some("option".to_string()),
                aria_expanded: None,
                aria_selected: None,
                aria_pressed: None,
                aria_controls: None,
                aria_owns: None,
                text: Some("Paris".to_string()),
                aria_label: None,
                aria_labelledby_text: None,
                title: None,
                placeholder: None,
                name: None,
                visible: true,
                floating: false,
                selector: "#city-list-option-1".to_string(),
            },
        ];
        assert!(detect_dropdown_opened_from_snapshots(
            &before,
            &after_option
        ));

        let after_floating = vec![
            before[0].clone(),
            RawElementFacts {
                tag: "div".to_string(),
                role: None,
                aria_expanded: None,
                aria_selected: None,
                aria_pressed: None,
                aria_controls: None,
                aria_owns: None,
                text: None,
                aria_label: None,
                aria_labelledby_text: None,
                title: None,
                placeholder: None,
                name: None,
                visible: true,
                floating: true,
                selector: "#overlay".to_string(),
            },
        ];
        assert!(detect_dropdown_opened_from_snapshots(
            &before,
            &after_floating
        ));
    }

    #[test]
    fn parses_option_candidates_from_evaluate_json() {
        let raw = json!({
            "value": [
                {"name": "English", "selector": "#opt-en", "role": "option", "aria_selected": "true", "disabled": false},
                {"name": "French", "selector": "#opt-fr", "role": "option", "aria_selected": null, "disabled": true}
            ]
        });

        let options = parse_option_candidates(&raw);
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].name, "English");
        assert_eq!(options[0].selector, "#opt-en");
        assert_eq!(options[0].aria_selected.as_deref(), Some("true"));
        assert!(options[1].disabled);
    }

    #[test]
    fn option_search_scopes_prefers_aria_controls_and_keeps_document_fallback() {
        let before = vec![RawElementFacts {
            tag: "button".to_string(),
            role: Some("combobox".to_string()),
            aria_expanded: Some("false".to_string()),
            aria_selected: None,
            aria_pressed: None,
            aria_controls: Some("portal-list".to_string()),
            aria_owns: None,
            text: Some("Language".to_string()),
            aria_label: None,
            aria_labelledby_text: None,
            title: None,
            placeholder: None,
            name: None,
            visible: true,
            floating: false,
            selector: "#lang-trigger".to_string(),
        }];
        let after = vec![
            RawElementFacts {
                aria_expanded: Some("true".to_string()),
                ..before[0].clone()
            },
            RawElementFacts {
                tag: "div".to_string(),
                role: Some("listbox".to_string()),
                aria_expanded: None,
                aria_selected: None,
                aria_pressed: None,
                aria_controls: None,
                aria_owns: None,
                text: None,
                aria_label: None,
                aria_labelledby_text: None,
                title: None,
                placeholder: None,
                name: None,
                visible: true,
                floating: true,
                selector: "#portal-list".to_string(),
            },
        ];

        let scopes = option_search_scopes("#lang-trigger", &before, &after);
        assert_eq!(scopes.first(), Some(&Some("#portal-list".to_string())));
        assert_eq!(scopes.last(), Some(&None));
    }

    #[test]
    fn match_text_returns_best_option_selector() {
        let options = [
            OptionCandidate {
                name: "United States".to_string(),
                selector: "#opt-us".to_string(),
                role: Some("option".to_string()),
                aria_selected: None,
                disabled: false,
            },
            OptionCandidate {
                name: "United Kingdom".to_string(),
                selector: "#opt-uk".to_string(),
                role: Some("option".to_string()),
                aria_selected: None,
                disabled: false,
            },
        ];

        let pairs: Vec<(String, String)> = options
            .iter()
            .map(|option| (option.name.clone(), option.selector.clone()))
            .collect();
        let (best, _) = crate::semantic::match_text("united kingdom", &pairs, None).unwrap();
        assert_eq!(best, "#opt-uk");
    }

    #[tokio::test]
    async fn list_options_mode_returns_options_without_selecting() {
        let state = Arc::new(Mutex::new(MockState {
            evaluate_results: VecDeque::from(vec![
                json!({"value": "DIV"}),
                json!({"value": [
                    {"name": "English", "selector": "#opt-en", "role": "option", "aria_selected": null, "disabled": false},
                    {"name": "French", "selector": "#opt-fr", "role": "option", "aria_selected": null, "disabled": false}
                ]}),
            ]),
            snapshot_results: VecDeque::from(vec![
                snapshot(vec![json!({
                    "tag": "button",
                    "role": "combobox",
                    "aria_expanded": "false",
                    "aria_selected": null,
                    "aria_pressed": null,
                    "aria_controls": "lang-list",
                    "aria_owns": null,
                    "text": "Language",
                    "aria_label": null,
                    "aria_labelledby_text": null,
                    "title": null,
                    "placeholder": null,
                    "name": null,
                    "visible": true,
                    "floating": false,
                    "selector": "#lang-trigger"
                })]),
                snapshot(vec![
                    json!({
                        "tag": "button",
                        "role": "combobox",
                        "aria_expanded": "true",
                        "aria_selected": null,
                        "aria_pressed": null,
                        "aria_controls": "lang-list",
                        "aria_owns": null,
                        "text": "Language",
                        "aria_label": null,
                        "aria_labelledby_text": null,
                        "title": null,
                        "placeholder": null,
                        "name": null,
                        "visible": true,
                        "floating": false,
                        "selector": "#lang-trigger"
                    }),
                    json!({
                        "tag": "div",
                        "role": "listbox",
                        "aria_expanded": null,
                        "aria_selected": null,
                        "aria_pressed": null,
                        "aria_controls": null,
                        "aria_owns": null,
                        "text": null,
                        "aria_label": null,
                        "aria_labelledby_text": null,
                        "title": null,
                        "placeholder": null,
                        "name": null,
                        "visible": true,
                        "floating": true,
                        "selector": "#lang-list"
                    }),
                ]),
            ]),
            ..MockState::default()
        }));
        let mut browser = make_browser(state.clone());

        let effect = execute(
            &json!({"selector": "#lang-trigger"}),
            &mut browser,
            &CrawlState::default(),
        )
        .await
        .unwrap();
        let response = effect_json(effect);

        assert_eq!(response["mode"], "list");
        assert_eq!(response["options"].as_array().unwrap().len(), 2);
        assert_eq!(state.lock().unwrap().native_selects.len(), 0);
    }

    #[tokio::test]
    async fn try_keyboard_select_presses_enter_when_target_becomes_active() {
        let state = Arc::new(Mutex::new(MockState {
            evaluate_results: VecDeque::from(vec![json!({"value": false}), json!({"value": true})]),
            ..MockState::default()
        }));
        let mut browser = make_browser(state.clone());

        let selected = try_keyboard_select(&mut browser, "#combo", "#opt-uk", 0)
            .await
            .unwrap();

        assert!(selected);
        assert_eq!(
            state.lock().unwrap().keypresses,
            vec![
                ("ArrowDown".to_string(), Some("#combo".to_string())),
                ("Enter".to_string(), Some("#combo".to_string())),
            ]
        );
    }

    #[tokio::test]
    async fn custom_selection_falls_back_to_click_when_keyboard_path_fails() {
        let state = Arc::new(Mutex::new(MockState {
            evaluate_results: VecDeque::from(vec![
                json!({"value": "DIV"}),
                json!({"value": [
                    {"name": "English", "selector": "#opt-en", "role": "option", "aria_selected": null, "disabled": false},
                    {"name": "French", "selector": "#opt-fr", "role": "option", "aria_selected": null, "disabled": false}
                ]}),
                json!({"value": false}),
                json!({"value": false}),
                json!({"value": false}),
                json!({"value": true}),
            ]),
            snapshot_results: VecDeque::from(vec![
                snapshot(vec![json!({
                    "tag": "button",
                    "role": "combobox",
                    "aria_expanded": "false",
                    "aria_selected": null,
                    "aria_pressed": null,
                    "aria_controls": "lang-list",
                    "aria_owns": null,
                    "text": "Language",
                    "aria_label": null,
                    "aria_labelledby_text": null,
                    "title": null,
                    "placeholder": null,
                    "name": null,
                    "visible": true,
                    "floating": false,
                    "selector": "#lang-trigger"
                })]),
                snapshot(vec![
                    json!({
                        "tag": "button",
                        "role": "combobox",
                        "aria_expanded": "true",
                        "aria_selected": null,
                        "aria_pressed": null,
                        "aria_controls": "lang-list",
                        "aria_owns": null,
                        "text": "Language",
                        "aria_label": null,
                        "aria_labelledby_text": null,
                        "title": null,
                        "placeholder": null,
                        "name": null,
                        "visible": true,
                        "floating": false,
                        "selector": "#lang-trigger"
                    }),
                    json!({
                        "tag": "div",
                        "role": "listbox",
                        "aria_expanded": null,
                        "aria_selected": null,
                        "aria_pressed": null,
                        "aria_controls": null,
                        "aria_owns": null,
                        "text": null,
                        "aria_label": null,
                        "aria_labelledby_text": null,
                        "title": null,
                        "placeholder": null,
                        "name": null,
                        "visible": true,
                        "floating": true,
                        "selector": "#lang-list"
                    }),
                ]),
            ]),
            ..MockState::default()
        }));
        let mut browser = make_browser(state.clone());

        let effect = execute(
            &json!({"selector": "#lang-trigger", "label": "French"}),
            &mut browser,
            &CrawlState::default(),
        )
        .await
        .unwrap();
        let response = effect_json(effect);

        assert_eq!(response["success"], true);
        assert!(state
            .lock()
            .unwrap()
            .clicks
            .iter()
            .any(|sel| sel == "#opt-fr"));
    }

    #[tokio::test]
    async fn custom_selection_returns_options_when_verification_fails() {
        let state = Arc::new(Mutex::new(MockState {
            evaluate_results: VecDeque::from(vec![
                json!({"value": "DIV"}),
                json!({"value": [
                    {"name": "English", "selector": "#opt-en", "role": "option", "aria_selected": null, "disabled": false},
                    {"name": "French", "selector": "#opt-fr", "role": "option", "aria_selected": null, "disabled": false}
                ]}),
                json!({"value": false}),
                json!({"value": false}),
                json!({"value": false}),
                json!({"value": false}),
            ]),
            snapshot_results: VecDeque::from(vec![
                snapshot(vec![json!({
                    "tag": "button",
                    "role": "combobox",
                    "aria_expanded": "false",
                    "aria_selected": null,
                    "aria_pressed": null,
                    "aria_controls": "lang-list",
                    "aria_owns": null,
                    "text": "Language",
                    "aria_label": null,
                    "aria_labelledby_text": null,
                    "title": null,
                    "placeholder": null,
                    "name": null,
                    "visible": true,
                    "floating": false,
                    "selector": "#lang-trigger"
                })]),
                snapshot(vec![
                    json!({
                        "tag": "button",
                        "role": "combobox",
                        "aria_expanded": "true",
                        "aria_selected": null,
                        "aria_pressed": null,
                        "aria_controls": "lang-list",
                        "aria_owns": null,
                        "text": "Language",
                        "aria_label": null,
                        "aria_labelledby_text": null,
                        "title": null,
                        "placeholder": null,
                        "name": null,
                        "visible": true,
                        "floating": false,
                        "selector": "#lang-trigger"
                    }),
                    json!({
                        "tag": "div",
                        "role": "listbox",
                        "aria_expanded": null,
                        "aria_selected": null,
                        "aria_pressed": null,
                        "aria_controls": null,
                        "aria_owns": null,
                        "text": null,
                        "aria_label": null,
                        "aria_labelledby_text": null,
                        "title": null,
                        "placeholder": null,
                        "name": null,
                        "visible": true,
                        "floating": true,
                        "selector": "#lang-list"
                    }),
                ]),
            ]),
            ..MockState::default()
        }));
        let mut browser = make_browser(state.clone());

        let effect = execute(
            &json!({"selector": "#lang-trigger", "label": "French"}),
            &mut browser,
            &CrawlState::default(),
        )
        .await
        .unwrap();
        let response = effect_json(effect);

        assert_eq!(response["success"], false);
        assert_eq!(response["options"].as_array().unwrap().len(), 2);
        assert!(state
            .lock()
            .unwrap()
            .clicks
            .iter()
            .any(|sel| sel == "#opt-fr"));
    }
}
