use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::timeout;

use acrawl_core::error::ToolExecutionError;

use crate::page_fingerprint::PageFingerprint;
use crate::state::CrawlState;
use crate::BrowserContext;

use super::page_map::{apply_page_map_caps, normalize_url};

const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);

/// Error message returned when a silent reCAPTCHA v3 submission is detected.
///
/// IMPORTANT: this exact string is pinned by the `failure_classifier` test — it MUST contain
/// "reCAPTCHA" (for `CaptchaDetected` routing) and MUST NOT contain "blocked".
pub(crate) const RECAPTCHA_V3_SILENT_SUBMISSION_MESSAGE: &str =
    "A submit request was sent but the page did not change, and this page uses reCAPTCHA v3 \
     (invisible, score-based). Headless browsers often score too low and the server may silently \
     reject the submission — though this could also be a client-side validation error or a \
     successful inline update. acrawl cannot read the server-side score. Report this to the user \
     and do not retry the same submit; a human can re-run with `acrawl config set headless false` \
     (or `--headed`), or use the extension bridge (`/extension`) to operate in a real browser \
     session.";

/// JavaScript snippet evaluated to detect reCAPTCHA v3 presence.
/// Returns true if v3 scripts are present AND there is no visible v2 widget.
/// Fail-open: wrapped in try/catch, any exception returns false.
const RECAPTCHA_V3_PROBE_JS: &str = "(() => { try { return (typeof grecaptcha !== 'undefined' || \
     /recaptcha\\/(api\\.js|releases)/i.test(document.documentElement.innerHTML)) && \
     !document.querySelector('.g-recaptcha'); } catch (e) { return false; } })()";

/// Hint for `post_action_page_state` indicating whether the caller might have triggered a form
/// submission. Used by the silent-submit audit (implemented separately) to narrow detection to
/// submit-capable interaction tools only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InteractionKind {
    /// The interaction could have triggered a form submission (click, `fill_form` with
    /// `submit:true`, `press_key` on a submit-capable element).
    PossibleSubmit,
    /// The interaction cannot submit a form (`hover`, `scroll`, `switch_tab`, `set_device`,
    /// `go_back`, `refresh`, `wait`, `select_option`, `fill_form` without submit).
    Passive,
}

/// Build structured page state from a raw `page_map` value (full, no diff).
///
/// Applies caps, extracts url/title from meta, preserves `regions` and
/// `active_dialog`, and strips `forms`, `interactive`, and `controls` to keep
/// interaction responses concise.
pub fn build_page_state_from_map(mut pm: Value) -> Value {
    super::page_map::enrich_semantic_sections(&mut pm);
    apply_page_map_caps(&mut pm);

    let url = pm
        .get("meta")
        .and_then(|m| m.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let title = pm
        .get("meta")
        .and_then(|m| m.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    if let Some(obj) = pm.as_object_mut() {
        obj.remove("forms");
        obj.remove("interactive");
        obj.remove("controls");
    }

    json!({
        "url": url,
        "title": title,
        "page_map": pm
    })
}

fn extract_url(pm: &Value) -> &str {
    pm.get("meta")
        .and_then(|m| m.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn extract_title(pm: &Value) -> &str {
    pm.get("meta")
        .and_then(|m| m.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn heading_key(h: &Value) -> Option<String> {
    let text = h.get("text")?.as_str()?;
    let level = h.get("level")?.as_u64()?;
    Some(format!("{level}:{text}"))
}

fn link_key(l: &Value) -> Option<String> {
    let href = l.get("href")?.as_str()?;
    let text = l.get("text").and_then(Value::as_str).unwrap_or("");
    Some(format!("{text}\x00{href}"))
}

fn landmark_key(lm: &Value) -> Option<String> {
    let tag = lm.get("tag")?.as_str()?;
    let role = lm.get("role").and_then(Value::as_str).unwrap_or("");
    let id = lm.get("id").and_then(Value::as_str).unwrap_or("");
    Some(format!("{tag}\x00{role}\x00{id}"))
}

fn diff_array(
    prev_items: &[Value],
    curr_items: &[Value],
    key_fn: fn(&Value) -> Option<String>,
) -> (Vec<Value>, Vec<Value>) {
    let mut prev_counts: HashMap<String, usize> = HashMap::new();
    for item in prev_items {
        if let Some(k) = key_fn(item) {
            *prev_counts.entry(k).or_default() += 1;
        }
    }

    let mut curr_counts: HashMap<String, usize> = HashMap::new();
    for item in curr_items {
        if let Some(k) = key_fn(item) {
            *curr_counts.entry(k).or_default() += 1;
        }
    }

    let mut added_budget: HashMap<&str, usize> = HashMap::new();
    for (k, &curr_n) in &curr_counts {
        let prev_n = prev_counts.get(k.as_str()).copied().unwrap_or(0);
        if curr_n > prev_n {
            added_budget.insert(k.as_str(), curr_n - prev_n);
        }
    }

    let mut removed_budget: HashMap<&str, usize> = HashMap::new();
    for (k, &prev_n) in &prev_counts {
        let curr_n = curr_counts.get(k.as_str()).copied().unwrap_or(0);
        if prev_n > curr_n {
            removed_budget.insert(k.as_str(), prev_n - curr_n);
        }
    }

    let added: Vec<Value> = curr_items
        .iter()
        .filter(|item| {
            if let Some(k) = key_fn(item) {
                if let Some(budget) = added_budget.get_mut(k.as_str()) {
                    if *budget > 0 {
                        *budget -= 1;
                        return true;
                    }
                }
            }
            false
        })
        .cloned()
        .collect();

    let removed: Vec<Value> = prev_items
        .iter()
        .filter(|item| {
            if let Some(k) = key_fn(item) {
                if let Some(budget) = removed_budget.get_mut(k.as_str()) {
                    if *budget > 0 {
                        *budget -= 1;
                        return true;
                    }
                }
            }
            false
        })
        .cloned()
        .collect();

    (added, removed)
}

fn get_array<'a>(pm: &'a Value, key: &str) -> &'a [Value] {
    pm.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn get_interactive_elements(pm: &Value) -> &[Value] {
    pm.get("interactive")
        .and_then(|i| i.get("elements"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

const STATE_FIELDS: &[&str] = &[
    "disabled",
    "checked",
    "value",
    "aria_pressed",
    "aria_expanded",
    "aria_selected",
];

const MAX_INTERACTIVE_DIFF: usize = 5;

struct InteractiveDiff {
    added: Vec<Value>,
    removed: Vec<Value>,
    modified: Vec<Value>,
}

fn interactive_entry_brief(el: &Value) -> Value {
    let mut entry = serde_json::Map::new();
    if let Some(selector) = el.get("selector") {
        entry.insert("selector".into(), selector.clone());
    }
    if let Some(tag) = el.get("tag") {
        entry.insert("tag".into(), tag.clone());
    }
    if let Some(text) = el.get("text") {
        entry.insert("text".into(), text.clone());
    }
    if let Some(role) = el.get("role") {
        entry.insert("role".into(), role.clone());
    }
    if let Some(ref_val) = el.get("ref") {
        entry.insert("ref".into(), ref_val.clone());
    }
    Value::Object(entry)
}

fn diff_interactive(prev_elements: &[Value], curr_elements: &[Value]) -> InteractiveDiff {
    let prev_by_selector: HashMap<&str, &Value> = prev_elements
        .iter()
        .filter_map(|el| el.get("selector").and_then(Value::as_str).map(|s| (s, el)))
        .collect();

    let curr_by_selector: HashMap<&str, &Value> = curr_elements
        .iter()
        .filter_map(|el| el.get("selector").and_then(Value::as_str).map(|s| (s, el)))
        .collect();

    let added: Vec<Value> = curr_elements
        .iter()
        .filter(|el| {
            el.get("selector")
                .and_then(Value::as_str)
                .is_some_and(|s| !prev_by_selector.contains_key(s))
        })
        .take(MAX_INTERACTIVE_DIFF)
        .map(interactive_entry_brief)
        .collect();

    let removed: Vec<Value> = prev_elements
        .iter()
        .filter(|el| {
            el.get("selector")
                .and_then(Value::as_str)
                .is_some_and(|s| !curr_by_selector.contains_key(s))
        })
        .take(MAX_INTERACTIVE_DIFF)
        .map(interactive_entry_brief)
        .collect();

    let mut modified = Vec::new();

    for el in curr_elements {
        let Some(selector) = el.get("selector").and_then(Value::as_str) else {
            continue;
        };
        let Some(prev_el) = prev_by_selector.get(selector) else {
            continue;
        };

        let mut changed_fields = serde_json::Map::new();
        for &field in STATE_FIELDS {
            let prev_val = prev_el.get(field);
            let curr_val = el.get(field);
            if prev_val != curr_val {
                changed_fields.insert(field.to_string(), curr_val.cloned().unwrap_or(Value::Null));
            }
        }

        if !changed_fields.is_empty() {
            let mut entry = serde_json::Map::new();
            entry.insert("selector".into(), json!(selector));
            if let Some(tag) = el.get("tag") {
                entry.insert("tag".into(), tag.clone());
            }
            if let Some(text) = el.get("text") {
                entry.insert("text".into(), text.clone());
            }
            entry.insert("state_changes".into(), Value::Object(changed_fields));
            modified.push(Value::Object(entry));
        }
    }

    InteractiveDiff {
        added,
        removed,
        modified,
    }
}

pub fn build_diff_page_state(prev: &Value, current: &mut Value) -> Value {
    apply_page_map_caps(current);

    let url = extract_url(current).to_string();
    let title = extract_title(current).to_string();

    let (added_headings, removed_headings) = diff_array(
        get_array(prev, "headings"),
        get_array(current, "headings"),
        heading_key,
    );
    let (added_links, removed_links) = diff_array(
        get_array(prev, "links"),
        get_array(current, "links"),
        link_key,
    );
    let (added_landmarks, removed_landmarks) = diff_array(
        get_array(prev, "landmarks"),
        get_array(current, "landmarks"),
        landmark_key,
    );
    let interactive_diff = diff_interactive(
        get_interactive_elements(prev),
        get_interactive_elements(current),
    );

    let has_changes = !added_headings.is_empty()
        || !removed_headings.is_empty()
        || !added_links.is_empty()
        || !removed_links.is_empty()
        || !added_landmarks.is_empty()
        || !removed_landmarks.is_empty()
        || !interactive_diff.added.is_empty()
        || !interactive_diff.removed.is_empty()
        || !interactive_diff.modified.is_empty();

    if !has_changes {
        return json!({
            "url": url,
            "title": title,
            "changed": false
        });
    }

    let total_prev = get_array(prev, "headings").len()
        + get_array(prev, "links").len()
        + get_array(prev, "landmarks").len();
    let total_changed = added_headings.len()
        + removed_headings.len()
        + added_links.len()
        + removed_links.len()
        + added_landmarks.len()
        + removed_landmarks.len();

    if total_prev > 0 && total_changed > total_prev {
        return build_page_state_from_map(current.clone());
    }

    let mut changes = serde_json::Map::new();
    if !added_headings.is_empty() {
        changes.insert("added_headings".into(), Value::Array(added_headings));
    }
    if !removed_headings.is_empty() {
        changes.insert("removed_headings".into(), Value::Array(removed_headings));
    }
    if !added_links.is_empty() {
        changes.insert("added_links".into(), Value::Array(added_links));
    }
    if !removed_links.is_empty() {
        changes.insert("removed_links".into(), Value::Array(removed_links));
    }
    if !added_landmarks.is_empty() {
        changes.insert("added_landmarks".into(), Value::Array(added_landmarks));
    }
    if !removed_landmarks.is_empty() {
        changes.insert("removed_landmarks".into(), Value::Array(removed_landmarks));
    }
    if !interactive_diff.added.is_empty() {
        changes.insert(
            "added_interactive".into(),
            Value::Array(interactive_diff.added),
        );
    }
    if !interactive_diff.removed.is_empty() {
        changes.insert(
            "removed_interactive".into(),
            Value::Array(interactive_diff.removed),
        );
    }
    if !interactive_diff.modified.is_empty() {
        changes.insert(
            "modified_interactive".into(),
            Value::Array(interactive_diff.modified),
        );
    }

    json!({
        "url": url,
        "title": title,
        "changed": true,
        "changes": changes
    })
}

fn fallback_value() -> Value {
    json!({
        "url": "unknown",
        "title": "unknown",
        "page_map": null
    })
}

fn evaluate_payload(value: &Value) -> &Value {
    value.get("value").unwrap_or(value)
}

fn active_dialog_scope(snapshot: &Value) -> Option<String> {
    let dialog = snapshot.get("active_dialog")?;
    if dialog
        .get("visible")
        .and_then(Value::as_bool)
        .is_some_and(|visible| !visible)
    {
        return None;
    }

    dialog
        .get("selector")
        .and_then(Value::as_str)
        .filter(|selector| !selector.is_empty())
        .map(str::to_string)
}

async fn resolve_interacted_scope(
    dialog_scope: Option<String>,
    interacted_selector: Option<&str>,
    widen: bool,
    bridge: &mut (dyn browser::BrowserBackend + Send),
) -> Option<String> {
    if widen {
        return None;
    }

    if let Some(scope) = dialog_scope {
        return Some(scope);
    }

    let selector = interacted_selector?;
    let selector_json = serde_json::to_string(selector).ok()?;
    let script = format!(
        r"(() => {{
            const el = document.querySelector({selector_json});
            if (!el) return null;
            let cur = el;
            while (cur && cur !== document.body) {{
                const role = cur.getAttribute('role');
                if (
                    ['dialog', 'alertdialog', 'region', 'main', 'complementary', 'navigation', 'form'].includes(role) ||
                    ['DIALOG', 'MAIN', 'ASIDE', 'NAV', 'FORM', 'SECTION', 'ARTICLE'].includes(cur.tagName)
                ) {{
                    if (cur.id) return '#' + CSS.escape(cur.id);
                    if (role === 'dialog' || role === 'alertdialog') {{
                        return '[role=' + role + ']';
                    }}
                    return null;
                }}
                cur = cur.parentElement;
            }}
            return null;
        }})()"
    );

    bridge
        .evaluate(&script)
        .await
        .ok()
        .and_then(|value| evaluate_payload(&value).as_str().map(str::to_string))
}

fn page_state_from_feedback_map(
    browser: &mut BrowserContext,
    scope: Option<&str>,
    mut pm: Value,
) -> Value {
    // Enrich in place before caching so stored snapshots preserve regions and
    // active_dialog for later scoped interactions.
    super::page_map::enrich_semantic_sections(&mut pm);

    let full_url = extract_url(&pm).to_string();
    let cache_key = normalize_url(&full_url).to_string();

    let response = match browser.page_snapshot_for_url(&cache_key, scope) {
        Some(prev) => build_diff_page_state(prev, &mut pm),
        None => build_page_state_from_map(pm.clone()),
    };

    browser.set_page_snapshot(&cache_key, scope, pm);
    response
}

/// Best-effort post-action page state for interaction tool responses.
///
/// Calls `page_map_feedback` on the bridge and applies Rust-side differential
/// comparison against the cached snapshot.
pub(crate) async fn post_action_page_state(
    browser: &mut BrowserContext,
    _crawl_state: &CrawlState,
    interaction_kind: InteractionKind,
    interacted_selector: Option<&str>,
    widen: bool,
) -> Result<Value, ToolExecutionError> {
    let dialog_scope = if widen {
        None
    } else {
        browser.last_page_snapshot().and_then(active_dialog_scope)
    };

    let result = timeout(FEEDBACK_TIMEOUT, async {
        let mut bridge = browser.acquire_bridge().await?;
        let scope =
            resolve_interacted_scope(dialog_scope, interacted_selector, widen, &mut **bridge).await;
        let page_map = bridge.page_map_feedback(scope.as_deref()).await?;
        Ok::<_, browser::BridgeError>((scope, page_map))
    })
    .await;

    match result {
        Ok(Ok((scope, pm))) => {
            let page_state = page_state_from_feedback_map(browser, scope.as_deref(), pm);
            if let Some(msg) = audit_silent_submission(browser, interaction_kind, &page_state).await
            {
                return Err(ToolExecutionError::new(msg.to_string()));
            }
            Ok(page_state)
        }
        _ => Ok(fallback_value()),
    }
}

/// Audit a just-completed interaction for a likely silent reCAPTCHA v3 rejection.
///
/// Returns `Some(RECAPTCHA_V3_SILENT_SUBMISSION_MESSAGE)` **only** when ALL three gates pass:
/// 1. `interaction_kind == InteractionKind::PossibleSubmit` — passive actions never trigger.
/// 2. Page did not navigate or structurally change (same URL, no headings/links/landmarks diff).
/// 3. reCAPTCHA v3 is present on the page (`RECAPTCHA_V3_PROBE_JS` returns `true`).
///
/// Fail-open: any error (bridge acquire, evaluate timeout, ambiguous result) returns `None`.
async fn audit_silent_submission(
    browser: &mut BrowserContext,
    interaction_kind: InteractionKind,
    page_state: &Value,
) -> Option<&'static str> {
    if interaction_kind != InteractionKind::PossibleSubmit {
        return None;
    }

    if !matches!(page_state.get("changed"), Some(Value::Bool(false))) {
        return None;
    }

    let v3_present = {
        let Ok(mut bridge) = browser.acquire_bridge().await else {
            return None;
        };
        match bridge.evaluate(RECAPTCHA_V3_PROBE_JS).await {
            Ok(result) => result
                .get("value")
                .and_then(Value::as_bool)
                .or_else(|| result.as_bool())
                .unwrap_or(false),
            Err(_) => return None,
        }
    };

    if v3_present {
        Some(RECAPTCHA_V3_SILENT_SUBMISSION_MESSAGE)
    } else {
        None
    }
}

/// Record a page fingerprint into `CrawlState` if the `page_fingerprinting`
/// optimization flag is enabled in settings.
pub fn record_page_fingerprint(url: &str, _page_map: &Value, crawl_state: &mut CrawlState) {
    let settings = runtime::load_settings();
    if !runtime::settings_get_page_fingerprinting(&settings) {
        return;
    }
    let fingerprint = if let Some(tree) = crawl_state.last_aria_tree.as_ref() {
        PageFingerprint::compute(url, tree)
    } else {
        let empty = crate::aria::AriaNode {
            role: "document".to_string(),
            name: None,
            states: crate::aria::AriaStates::default(),
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: vec![],
            omitted_children: 0,
        };
        PageFingerprint::compute(url, &empty)
    };
    crawl_state.page_fingerprints.push(fingerprint);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use browser::{
        BridgeError, BrowserState, PageInfo, ScreenshotOptions, StorageEntry, StorageType,
    };
    use tokio::sync::Mutex;

    use serde_json::{json, Value};

    use super::{
        build_diff_page_state, build_page_state_from_map, fallback_value,
        page_state_from_feedback_map, post_action_page_state, InteractionKind,
        RECAPTCHA_V3_SILENT_SUBMISSION_MESSAGE,
    };
    use crate::state::CrawlState;
    use crate::BrowserContext;
    use browser::BrowserBackend;

    #[derive(Debug, Default)]
    struct FeedbackMockState {
        page_maps: HashMap<String, Value>,
        requested_scopes: Vec<Option<String>>,
        evaluate_result: Value,
        evaluate_script_results: Vec<(String, Value)>,
        evaluate_error_substrings: Vec<String>,
        evaluate_scripts: Vec<String>,
    }

    #[derive(Debug)]
    struct FeedbackMockBackend {
        state: Arc<StdMutex<FeedbackMockState>>,
    }

    #[async_trait]
    impl BrowserBackend for FeedbackMockBackend {
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
            scope: Option<&str>,
            _compound_enrichment: bool,
        ) -> Result<Value, BridgeError> {
            let mut state = self.state.lock().expect("mock state poisoned");
            state.requested_scopes.push(scope.map(str::to_string));
            let key = scope.unwrap_or("").to_string();
            state
                .page_maps
                .get(&key)
                .cloned()
                .ok_or_else(|| BridgeError::Protocol(format!("missing page_map for scope '{key}'")))
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

        async fn select_option(
            &mut self,
            _selector: &str,
            _value: &str,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn evaluate(&mut self, script: &str) -> Result<Value, BridgeError> {
            let mut state = self.state.lock().expect("mock state poisoned");
            state.evaluate_scripts.push(script.to_string());
            if state
                .evaluate_error_substrings
                .iter()
                .any(|substring| script.contains(substring))
            {
                return Err(BridgeError::Protocol("mock evaluate failure".to_string()));
            }
            for (substring, result) in &state.evaluate_script_results {
                if script.contains(substring.as_str()) {
                    return Ok(result.clone());
                }
            }
            Ok(state.evaluate_result.clone())
        }

        async fn hover(&mut self, _selector: &str) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn press_key(
            &mut self,
            _key: &str,
            _selector: Option<&str>,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn switch_tab(&mut self, _index: i64) -> Result<Value, BridgeError> {
            Ok(json!({ "ok": true }))
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

        async fn click(&mut self, _selector: &str) -> Result<(), BridgeError> {
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

        async fn poll_observations(
            &mut self,
        ) -> Result<Vec<browser::ObservationEvent>, BridgeError> {
            Ok(Vec::new())
        }

        async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn get_storage(
            &mut self,
            _target: StorageType,
        ) -> Result<(Vec<StorageEntry>, Vec<StorageEntry>), BridgeError> {
            Ok((Vec::new(), Vec::new()))
        }
    }

    fn browser_with_feedback_backend(
        state: FeedbackMockState,
        url: &str,
    ) -> (BrowserContext, Arc<StdMutex<FeedbackMockState>>) {
        let state = Arc::new(StdMutex::new(state));
        let bridge = Arc::new(Mutex::new(Box::new(FeedbackMockBackend {
            state: Arc::clone(&state),
        }) as Box<dyn BrowserBackend + Send>));
        let mut browser = BrowserContext::new(bridge);
        browser.set_navigated_url(url, true);
        (browser, state)
    }

    fn minimal_page_map(url: &str, title: &str) -> Value {
        json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {"elements": []},
            "meta": {"title": title, "url": url, "description": ""}
        })
    }

    fn unchanged_response(url: &str, title: &str) -> Value {
        json!({
            "url": url,
            "title": title,
            "changed": false
        })
    }

    #[test]
    fn feedback_fallback_value_has_correct_shape() {
        let val = fallback_value();

        assert_eq!(val["url"], "unknown");
        assert_eq!(val["title"], "unknown");
        assert!(val["page_map"].is_null(), "page_map should be null");

        let obj = val.as_object().expect("fallback should be an object");
        assert!(obj.contains_key("url"));
        assert!(obj.contains_key("title"));
        assert!(obj.contains_key("page_map"));
        assert_eq!(obj.len(), 3, "fallback should have exactly 3 keys");
    }

    #[test]
    fn feedback_success_value_from_mock_page_map() {
        let mock_page_map = json!({
            "headings": [
                {
                    "level": 1,
                    "text": "Welcome",
                    "id": "welcome",
                    "selector": "#welcome",
                    "char_count": 100,
                    "preview": "Welcome to the site"
                }
            ],
            "landmarks": [
                {
                    "tag": "nav",
                    "role": "navigation",
                    "id": "nav",
                    "selector": "#nav",
                    "text_preview": "Main nav"
                }
            ],
            "forms": [
                {
                    "action": "/login",
                    "method": "post",
                    "id": "login-form",
                    "selector": "#login-form",
                    "fields": [{"name": "email", "type": "email"}]
                }
            ],
            "links": [
                {
                    "text": "Home",
                    "href": "https://example.com",
                    "selector": "a.home"
                }
            ],
            "regions": [
                {
                    "kind": "Main",
                    "label": "main panel",
                    "handle": "@r1",
                    "selector": "main",
                    "visible": true,
                    "children": []
                }
            ],
            "active_dialog": {
                "handle": "@r2",
                "selector": "#confirm-dialog",
                "label": "Confirm"
            },
            "controls": [
                {
                    "label": "Search",
                    "role": "textbox",
                    "selector": "#search",
                    "value": null,
                    "required": false,
                    "disabled": false
                }
            ],
            "interactive": {
                "buttons": 3,
                "inputs": 2,
                "selects": 1,
                "textareas": 0
            },
            "meta": {
                "title": "Example Page",
                "url": "https://example.com/page",
                "description": "A test page"
            }
        });

        let result = build_page_state_from_map(mock_page_map);

        assert_eq!(result["url"], "https://example.com/page");
        assert_eq!(result["title"], "Example Page");
        assert!(!result["page_map"].is_null(), "page_map should not be null");

        let pm = &result["page_map"];
        assert!(pm["headings"].is_array());
        assert!(pm["landmarks"].is_array());
        assert!(pm["links"].is_array());
        assert!(pm["regions"].is_array());
        assert!(pm.get("active_dialog").is_some());
        assert!(pm["meta"].is_object());

        assert!(
            pm.get("forms").is_none(),
            "forms should be removed from page_map"
        );
        assert!(
            pm.get("interactive").is_none(),
            "interactive should be removed from page_map"
        );
        assert!(
            pm.get("controls").is_none(),
            "controls should be removed from page_map"
        );

        assert!(pm.get("truncated_links").is_some());
        assert!(pm.get("truncated_forms").is_some());
        assert!(pm.get("truncated_landmarks").is_some());
    }

    fn base_page_map() -> serde_json::Value {
        json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"},
                {"text": "About", "href": "https://example.com/about", "selector": "a.about"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        })
    }

    #[test]
    fn diff_no_changes_returns_changed_false() {
        let prev = base_page_map();
        let mut current = base_page_map();

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], false);
        assert_eq!(result["url"], "https://example.com/page");
        assert_eq!(result["title"], "Test Page");
        assert!(result.get("changes").is_none());
    }

    #[test]
    fn diff_modal_added_shows_only_new_elements() {
        let prev = base_page_map();
        let mut current = json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"},
                {"level": 2, "text": "Sign Up", "selector": "div.modal > h2", "char_count": 50, "preview": "Create account"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"},
                {"tag": "dialog", "role": "dialog", "id": "signup", "selector": "#signup", "text_preview": "Sign up form"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"},
                {"text": "About", "href": "https://example.com/about", "selector": "a.about"},
                {"text": "Login instead", "href": "https://example.com/login", "selector": "div.modal a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        assert_eq!(changes["added_headings"].as_array().unwrap().len(), 1);
        assert_eq!(changes["added_headings"][0]["text"], "Sign Up");
        assert_eq!(changes["added_links"].as_array().unwrap().len(), 1);
        assert_eq!(changes["added_links"][0]["text"], "Login instead");
        assert_eq!(changes["added_landmarks"].as_array().unwrap().len(), 1);
        assert_eq!(changes["added_landmarks"][0]["tag"], "dialog");
        assert!(changes.get("removed_headings").is_none());
        assert!(changes.get("removed_links").is_none());
        assert!(changes.get("removed_landmarks").is_none());
    }

    #[test]
    fn diff_modal_closed_shows_removed_elements() {
        let prev = json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"},
                {"level": 2, "text": "Sign Up", "selector": "div.modal > h2", "char_count": 50, "preview": "Create"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"},
                {"text": "Login instead", "href": "https://example.com/login", "selector": "div.modal a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        });

        let mut current = json!({
            "headings": [
                {"level": 1, "text": "Welcome", "selector": "#welcome", "char_count": 100, "preview": "Hello"}
            ],
            "landmarks": [
                {"tag": "nav", "role": "navigation", "id": "main-nav", "selector": "nav", "text_preview": "Nav"}
            ],
            "links": [
                {"text": "Home", "href": "https://example.com/", "selector": "a.home"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Test Page", "url": "https://example.com/page", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        assert_eq!(changes["removed_headings"].as_array().unwrap().len(), 1);
        assert_eq!(changes["removed_headings"][0]["text"], "Sign Up");
        assert_eq!(changes["removed_links"].as_array().unwrap().len(), 1);
        assert_eq!(
            changes["removed_links"][0]["href"],
            "https://example.com/login"
        );
        assert!(changes.get("added_headings").is_none());
    }

    #[test]
    fn diff_ignores_selector_changes_for_same_content() {
        let prev = json!({
            "headings": [
                {"level": 1, "text": "Title", "selector": "div:nth-of-type(1) > h1", "char_count": 10, "preview": "T"}
            ],
            "landmarks": [],
            "links": [
                {"text": "Link", "href": "https://example.com/page", "selector": "div:nth-of-type(1) > a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": "https://example.com/page", "description": ""}
        });

        let mut current = json!({
            "headings": [
                {"level": 1, "text": "Title", "selector": "div:nth-of-type(2) > h1", "char_count": 10, "preview": "T"}
            ],
            "landmarks": [],
            "links": [
                {"text": "Link", "href": "https://example.com/page", "selector": "div:nth-of-type(2) > a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": "https://example.com/page", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], false);
    }

    #[test]
    fn diff_too_large_falls_back_to_full_page_map() {
        let prev = json!({
            "headings": [
                {"level": 1, "text": "Home", "selector": "h1", "char_count": 10, "preview": "Home"},
                {"level": 2, "text": "Features", "selector": "h2", "char_count": 10, "preview": "Feat"}
            ],
            "landmarks": [
                {"tag": "main", "role": "main", "id": "content", "selector": "#content", "text_preview": "Main"}
            ],
            "links": [
                {"text": "Link A", "href": "https://example.com/a", "selector": "a"},
                {"text": "Link B", "href": "https://example.com/b", "selector": "a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Home", "url": "https://example.com/", "description": ""}
        });

        let mut current = json!({
            "headings": [
                {"level": 1, "text": "About Us", "selector": "h1", "char_count": 20, "preview": "About"},
                {"level": 2, "text": "Team", "selector": "h2.team", "char_count": 15, "preview": "Team"},
                {"level": 2, "text": "Mission", "selector": "h2.mission", "char_count": 15, "preview": "Mission"}
            ],
            "landmarks": [
                {"tag": "main", "role": "main", "id": "about", "selector": "#about", "text_preview": "About"}
            ],
            "links": [
                {"text": "Contact", "href": "https://example.com/contact", "selector": "a"},
                {"text": "Join Us", "href": "https://example.com/careers", "selector": "a"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "About", "url": "https://example.com/", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert!(
            result.get("page_map").is_some(),
            "should fall back to full page_map when diff is too large"
        );
        assert_eq!(result["url"], "https://example.com/");
    }

    #[test]
    fn url_without_hash_strips_fragment() {
        use super::super::page_map::normalize_url;

        assert_eq!(
            normalize_url("https://example.com/page#section"),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_url("https://example.com/page"),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_url("https://app.com/#/dashboard"),
            "https://app.com/#/dashboard"
        );
        assert_eq!(
            normalize_url("https://app.com/#!/billing"),
            "https://app.com/#!/billing"
        );
        assert_eq!(normalize_url("unknown"), "unknown");
    }

    #[test]
    fn diff_interactive_state_changes_detected() {
        let prev = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 2, "inputs": 0, "selects": 0, "textareas": 0, "total": 2},
                "elements": [
                    {"tag": "button", "text": "Menu", "selector": "#menu-btn", "aria_expanded": "false"},
                    {"tag": "button", "text": "Submit", "selector": "#submit-btn", "disabled": true}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let mut current = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 2, "inputs": 0, "selects": 0, "textareas": 0, "total": 2},
                "elements": [
                    {"tag": "button", "text": "Menu", "selector": "#menu-btn", "aria_expanded": "true"},
                    {"tag": "button", "text": "Submit", "selector": "#submit-btn"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        let modified = changes["modified_interactive"].as_array().unwrap();
        assert_eq!(modified.len(), 2);

        let menu = modified
            .iter()
            .find(|e| e["selector"] == "#menu-btn")
            .unwrap();
        assert_eq!(menu["state_changes"]["aria_expanded"], "true");

        let submit = modified
            .iter()
            .find(|e| e["selector"] == "#submit-btn")
            .unwrap();
        assert!(submit["state_changes"]["disabled"].is_null());
    }

    #[test]
    fn diff_duplicate_links_counted_correctly() {
        let prev = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Read more", "href": "https://example.com/post/1", "selector": "a:nth-of-type(1)"},
                {"text": "Read more", "href": "https://example.com/post/1", "selector": "a:nth-of-type(2)"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let mut current = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Read more", "href": "https://example.com/post/1", "selector": "a:nth-of-type(1)"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        let removed = changes["removed_links"].as_array().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0]["text"], "Read more");
    }

    #[test]
    fn diff_added_interactive_elements_detected() {
        let prev = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 1, "inputs": 0, "selects": 0, "textareas": 0, "total": 1},
                "elements": [
                    {"tag": "button", "text": "Add Element", "selector": "#add-btn", "type": "submit"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let mut current = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 2, "inputs": 0, "selects": 0, "textareas": 0, "total": 2},
                "elements": [
                    {"tag": "button", "text": "Add Element", "selector": "#add-btn", "type": "submit"},
                    {"tag": "button", "text": "Delete", "selector": "#del-btn", "type": "submit"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        let added = changes["added_interactive"].as_array().unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0]["text"], "Delete");
        assert_eq!(added[0]["selector"], "#del-btn");
    }

    #[test]
    fn extension_style_raw_page_map_uses_same_diff_as_bridge_path() {
        let prev = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 1, "inputs": 0, "selects": 0, "textareas": 0, "total": 1},
                "elements": [
                    {"tag": "button", "text": "Add Element", "selector": "#add-btn", "type": "submit"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });
        let mut current = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 2, "inputs": 0, "selects": 0, "textareas": 0, "total": 2},
                "elements": [
                    {"tag": "button", "text": "Add Element", "selector": "#add-btn", "type": "submit"},
                    {"tag": "button", "text": "Delete", "selector": "#del-btn", "type": "submit"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let bridge = Arc::new(Mutex::new(Box::new(
            crate::tools::test_support::ObservationMockBackend::default(),
        ) as Box<dyn BrowserBackend + Send>));
        let mut browser = BrowserContext::new(bridge);
        browser.set_page_snapshot("https://example.com/", None, prev.clone());

        let extension_path = page_state_from_feedback_map(&mut browser, None, current.clone());
        let bridge_path = build_diff_page_state(&prev, &mut current);

        assert_eq!(extension_path, bridge_path);
        assert_eq!(extension_path["changed"], true);
        let added = extension_path["changes"]["added_interactive"]
            .as_array()
            .expect("added_interactive should be present");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0]["selector"], "#del-btn");
    }

    #[test]
    fn diff_removed_interactive_elements_detected() {
        let prev = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 2, "inputs": 0, "selects": 0, "textareas": 0, "total": 2},
                "elements": [
                    {"tag": "button", "text": "Add Element", "selector": "#add-btn", "type": "submit"},
                    {"tag": "button", "text": "Delete", "selector": "#del-btn", "type": "submit"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let mut current = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 1, "inputs": 0, "selects": 0, "textareas": 0, "total": 1},
                "elements": [
                    {"tag": "button", "text": "Add Element", "selector": "#add-btn", "type": "submit"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        let removed = changes["removed_interactive"].as_array().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0]["text"], "Delete");
    }

    #[test]
    fn diff_select_value_change_detected() {
        let prev = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 0, "inputs": 0, "selects": 1, "textareas": 0, "total": 1},
                "elements": [
                    {"tag": "select", "text": "Option 1\nOption 2", "selector": "#dropdown", "type": "select-one", "value": "Please select"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let mut current = json!({
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {
                "counts": {"buttons": 0, "inputs": 0, "selects": 1, "textareas": 0, "total": 1},
                "elements": [
                    {"tag": "select", "text": "Option 1\nOption 2", "selector": "#dropdown", "type": "select-one", "value": "Option 1"}
                ]
            },
            "meta": {"title": "Page", "url": "https://example.com/", "description": ""}
        });

        let result = build_diff_page_state(&prev, &mut current);

        assert_eq!(result["changed"], true);
        let changes = &result["changes"];
        let modified = changes["modified_interactive"].as_array().unwrap();
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0]["selector"], "#dropdown");
        assert_eq!(modified[0]["state_changes"]["value"], "Option 1");
    }

    #[test]
    fn scoped_diff_filters_to_container() {
        let url = "https://example.com/settings";
        let prev_full = json!({
            "headings": [
                {"level": 1, "text": "Settings", "selector": "#settings h1", "char_count": 20, "preview": "Settings"}
            ],
            "landmarks": [
                {"tag": "main", "role": "main", "id": "content", "selector": "main", "text_preview": "Content"}
            ],
            "links": [
                {"text": "Profile", "href": "https://example.com/profile", "selector": "header a.profile"},
                {"text": "Save", "href": "https://example.com/save", "selector": "#settings a.save"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Settings", "url": url, "description": ""}
        });
        let mut current_full = json!({
            "headings": [
                {"level": 1, "text": "Settings", "selector": "#settings h1", "char_count": 20, "preview": "Settings"}
            ],
            "landmarks": [
                {"tag": "main", "role": "main", "id": "content", "selector": "main", "text_preview": "Content"}
            ],
            "links": [
                {"text": "Profile", "href": "https://example.com/profile", "selector": "header a.profile"},
                {"text": "Billing", "href": "https://example.com/billing", "selector": "header a.billing"},
                {"text": "Save", "href": "https://example.com/save", "selector": "#settings a.save"},
                {"text": "Reset", "href": "https://example.com/reset", "selector": "#settings a.reset"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Settings", "url": url, "description": ""}
        });
        let prev_scoped = json!({
            "headings": [
                {"level": 1, "text": "Settings", "selector": "#settings h1", "char_count": 20, "preview": "Settings"}
            ],
            "landmarks": [],
            "links": [
                {"text": "Save", "href": "https://example.com/save", "selector": "#settings a.save"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Settings", "url": url, "description": ""}
        });
        let current_scoped = json!({
            "headings": [
                {"level": 1, "text": "Settings", "selector": "#settings h1", "char_count": 20, "preview": "Settings"}
            ],
            "landmarks": [],
            "links": [
                {"text": "Save", "href": "https://example.com/save", "selector": "#settings a.save"},
                {"text": "Reset", "href": "https://example.com/reset", "selector": "#settings a.reset"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Settings", "url": url, "description": ""}
        });

        let full_diff = build_diff_page_state(&prev_full, &mut current_full);

        let bridge = Arc::new(Mutex::new(Box::new(
            crate::tools::test_support::ObservationMockBackend::default(),
        ) as Box<dyn BrowserBackend + Send>));
        let mut browser = BrowserContext::new(bridge);
        browser.set_page_snapshot(url, Some("#settings"), prev_scoped);

        let scoped_diff =
            page_state_from_feedback_map(&mut browser, Some("#settings"), current_scoped);

        assert_eq!(
            full_diff["changes"]["added_links"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            scoped_diff["changes"]["added_links"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(scoped_diff["changes"]["added_links"][0]["text"], "Reset");
    }

    #[tokio::test]
    async fn active_dialog_scope_uses_dialog_baseline() {
        let url = "https://example.com/page";
        let prev_dialog = json!({
            "headings": [
                {"level": 2, "text": "Confirm", "selector": "#confirm-dialog h2", "char_count": 10, "preview": "Confirm"}
            ],
            "landmarks": [
                {"tag": "dialog", "role": "dialog", "id": "confirm-dialog", "selector": "#confirm-dialog", "text_preview": "Confirm dialog"}
            ],
            "links": [
                {"text": "Cancel", "href": "https://example.com/cancel", "selector": "#confirm-dialog a.cancel"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Dialog", "url": url, "description": ""}
        });
        let current_dialog = json!({
            "headings": [
                {"level": 2, "text": "Confirm", "selector": "#confirm-dialog h2", "char_count": 10, "preview": "Confirm"}
            ],
            "landmarks": [
                {"tag": "dialog", "role": "dialog", "id": "confirm-dialog", "selector": "#confirm-dialog", "text_preview": "Confirm dialog"}
            ],
            "links": [
                {"text": "Cancel", "href": "https://example.com/cancel", "selector": "#confirm-dialog a.cancel"},
                {"text": "Delete", "href": "https://example.com/delete", "selector": "#confirm-dialog a.delete"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Dialog", "url": url, "description": ""}
        });

        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([("#confirm-dialog".to_string(), current_dialog)]),
                evaluate_result: json!("section"),
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, Some("#confirm-dialog"), prev_dialog);
        browser.set_page_snapshot(
            url,
            None,
            json!({
                "headings": [],
                "landmarks": [],
                "links": [],
                "forms": [],
                "interactive": {},
                "active_dialog": {"selector": "#confirm-dialog", "visible": true},
                "meta": {"title": "Page", "url": url, "description": ""}
            }),
        );

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::Passive,
            Some("#outside"),
            false,
        )
        .await
        .unwrap();
        let state = state.lock().expect("mock state poisoned");

        assert_eq!(
            state.requested_scopes,
            vec![Some("#confirm-dialog".to_string())]
        );
        assert!(state.evaluate_scripts.is_empty());
        assert_eq!(result["changes"]["added_links"][0]["text"], "Delete");
    }

    #[tokio::test]
    async fn widen_true_returns_full_page_diff() {
        let url = "https://example.com/page";
        let prev_full = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Account", "href": "https://example.com/account", "selector": "header a.account"},
                {"text": "Save", "href": "https://example.com/save", "selector": "#panel a.save"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": url, "description": ""}
        });
        let prev_scoped = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Save", "href": "https://example.com/save", "selector": "#panel a.save"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": url, "description": ""}
        });
        let current_full = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Account", "href": "https://example.com/account", "selector": "header a.account"},
                {"text": "Billing", "href": "https://example.com/billing", "selector": "header a.billing"},
                {"text": "Save", "href": "https://example.com/save", "selector": "#panel a.save"},
                {"text": "Reset", "href": "https://example.com/reset", "selector": "#panel a.reset"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": url, "description": ""}
        });
        let current_scoped = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Save", "href": "https://example.com/save", "selector": "#panel a.save"},
                {"text": "Reset", "href": "https://example.com/reset", "selector": "#panel a.reset"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": url, "description": ""}
        });

        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([
                    (String::new(), current_full),
                    ("section".to_string(), current_scoped),
                ]),
                evaluate_result: json!("section"),
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, prev_full);
        browser.set_page_snapshot(url, Some("section"), prev_scoped);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::Passive,
            Some("#trigger"),
            true,
        )
        .await
        .unwrap();
        let state = state.lock().expect("mock state poisoned");

        assert_eq!(state.requested_scopes, vec![None]);
        assert!(state.evaluate_scripts.is_empty());
        assert_eq!(
            result["changes"]["added_links"].as_array().unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn no_container_falls_back_to_full_page() {
        let url = "https://example.com/page";
        let prev_full = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Account", "href": "https://example.com/account", "selector": "header a.account"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": url, "description": ""}
        });
        let current_full = json!({
            "headings": [],
            "landmarks": [],
            "links": [
                {"text": "Account", "href": "https://example.com/account", "selector": "header a.account"},
                {"text": "Billing", "href": "https://example.com/billing", "selector": "header a.billing"}
            ],
            "forms": [],
            "interactive": {},
            "meta": {"title": "Page", "url": url, "description": ""}
        });

        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), current_full)]),
                evaluate_result: Value::Null,
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, prev_full);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::Passive,
            Some("#trigger"),
            false,
        )
        .await
        .unwrap();
        let state = state.lock().expect("mock state poisoned");

        assert_eq!(state.requested_scopes, vec![None]);
        assert_eq!(state.evaluate_scripts.len(), 1);
        assert_eq!(result["changes"]["added_links"][0]["text"], "Billing");
    }

    #[tokio::test]
    async fn silent_submit_fires_on_silent_v3_rejection() {
        let url = "https://example.com/form";
        let title = "Form";
        let page_map = minimal_page_map(url, title);
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map.clone())]),
                evaluate_script_results: vec![("grecaptcha".to_string(), json!({"value": true}))],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, page_map);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await;

        let err = result.expect_err("silent v3 rejection should error");
        assert_eq!(err.to_string(), RECAPTCHA_V3_SILENT_SUBMISSION_MESSAGE);
        assert!(err.to_string().contains("reCAPTCHA"));
        assert!(!err.to_string().to_lowercase().contains("blocked"));

        let state = state.lock().expect("mock state poisoned");
        assert_eq!(state.evaluate_scripts.len(), 1);
    }

    #[tokio::test]
    async fn silent_submit_does_not_fire_without_v3_probe_match() {
        let url = "https://example.com/form";
        let title = "Form";
        let page_map = minimal_page_map(url, title);
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map.clone())]),
                evaluate_script_results: vec![("grecaptcha".to_string(), json!({"value": false}))],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, page_map);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await
        .unwrap();

        assert_eq!(result, unchanged_response(url, title));
        let state = state.lock().expect("mock state poisoned");
        assert_eq!(state.evaluate_scripts.len(), 1);
    }

    #[tokio::test]
    async fn silent_submit_does_not_fire_when_page_changed() {
        let url = "https://example.com/form";
        let prev = minimal_page_map(url, "Form");
        let current = json!({
            "headings": [
                {"level": 1, "text": "Thanks", "id": "thanks", "selector": "#thanks", "char_count": 6, "preview": "Thanks"}
            ],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {"elements": []},
            "meta": {"title": "Form", "url": url, "description": ""}
        });
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), current)]),
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, prev);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await
        .unwrap();

        assert_eq!(result["changed"], Value::Bool(true));
        let state = state.lock().expect("mock state poisoned");
        assert!(state.evaluate_scripts.is_empty());
    }

    #[tokio::test]
    async fn silent_submit_does_not_fire_for_passive_interaction() {
        let url = "https://example.com/form";
        let title = "Form";
        let page_map = minimal_page_map(url, title);
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map.clone())]),
                evaluate_script_results: vec![("grecaptcha".to_string(), json!({"value": true}))],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, page_map);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::Passive,
            None,
            false,
        )
        .await
        .unwrap();

        assert_eq!(result, unchanged_response(url, title));
        let state = state.lock().expect("mock state poisoned");
        assert!(state.evaluate_scripts.is_empty());
    }

    #[tokio::test]
    async fn silent_submit_does_not_fire_on_first_interaction_without_changed_key() {
        let url = "https://example.com/form";
        let title = "Form";
        let page_map = minimal_page_map(url, title);
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map)]),
                evaluate_script_results: vec![("grecaptcha".to_string(), json!({"value": true}))],
                ..FeedbackMockState::default()
            },
            url,
        );

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await
        .unwrap();

        assert!(result.get("changed").is_none());
        let state = state.lock().expect("mock state poisoned");
        assert!(state.evaluate_scripts.is_empty());
    }

    #[tokio::test]
    async fn silent_submit_fail_opens_when_probe_evaluate_errors() {
        let url = "https://example.com/form";
        let title = "Form";
        let page_map = minimal_page_map(url, title);
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map.clone())]),
                evaluate_error_substrings: vec!["grecaptcha".to_string()],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, page_map);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await
        .unwrap();

        assert_eq!(result, unchanged_response(url, title));
        let state = state.lock().expect("mock state poisoned");
        assert_eq!(state.evaluate_scripts.len(), 1);
    }

    #[tokio::test]
    async fn silent_submit_fail_opens_when_probe_returns_non_bool() {
        let url = "https://example.com/form";
        let title = "Form";
        let page_map = minimal_page_map(url, title);
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map.clone())]),
                evaluate_script_results: vec![("grecaptcha".to_string(), Value::Null)],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, page_map);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await
        .unwrap();

        assert_eq!(result, unchanged_response(url, title));
        let state = state.lock().expect("mock state poisoned");
        assert_eq!(state.evaluate_scripts.len(), 1);
    }

    #[tokio::test]
    async fn silent_submit_no_double_submit_probe_runs_once() {
        let url = "https://example.com/form";
        let page_map = minimal_page_map(url, "Form");
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map.clone())]),
                evaluate_script_results: vec![("grecaptcha".to_string(), json!({"value": true}))],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, page_map);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await;

        assert!(result.is_err());
        let state = state.lock().expect("mock state poisoned");
        // The audit probe must run exactly once — no retries.  We assert on the
        // *total* evaluate call count (not just a filtered subset) to prove that
        // nothing re-invoked the probe after the `Err` was returned.
        //
        // Broader no-double-submit guarantee: the `Err(CaptchaDetected)` return
        // maps to `RetryStrategy::NoRetry`, which prevents `implementation/mod.rs`
        // self-healing from re-invoking the interaction tool (and therefore the
        // submit action).  That path is independently covered by
        // `silent_submit_recaptcha_v3_message_classifies_as_captcha_detected`.
        assert_eq!(
            state.evaluate_scripts.len(),
            1,
            "exactly one evaluate call total — audit probe must not be re-invoked"
        );
        assert!(
            state.evaluate_scripts[0].contains("grecaptcha"),
            "the single evaluate call must be the v3 reCAPTCHA probe"
        );
    }

    #[tokio::test]
    async fn silent_submit_passive_action_byte_identical_regression() {
        let url = "https://example.com/form";
        let title = "Form";
        let page_map = minimal_page_map(url, title);
        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), page_map.clone())]),
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, page_map);

        let result = post_action_page_state(
            &mut browser,
            &CrawlState::default(),
            InteractionKind::Passive,
            None,
            false,
        )
        .await
        .unwrap();

        let expected = unchanged_response(url, title);
        assert_eq!(
            serde_json::to_vec(&result).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        let state = state.lock().expect("mock state poisoned");
        assert!(state.evaluate_scripts.is_empty());
    }
}
