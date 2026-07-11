use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::timeout;

use acrawl_core::error::ToolExecutionError;

use crate::aria::{assign_refs, identity_key, parse_raw_tree, to_yaml, AriaNode};
use crate::page_fingerprint::PageFingerprint;
use crate::state::CrawlState;
use crate::BrowserContext;

use super::page_map::{apply_page_map_caps, normalize_url};

const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);
const FEEDBACK_TREE_DEPTH: usize = 5;
const DIFF_SNIPPET_DEPTH: usize = 2;
const MAX_DIFF_ITEMS: usize = 30;

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

fn extract_feedback_url(pm: &Value) -> &str {
    pm.get("url")
        .and_then(Value::as_str)
        .or_else(|| {
            pm.get("meta")
                .and_then(|meta| meta.get("url"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
}

fn node_name(node: &AriaNode) -> &str {
    node.name.as_deref().unwrap_or("")
}

fn identity_refs(ancestors: &[(String, String)]) -> Vec<(&str, &str)> {
    ancestors
        .iter()
        .map(|(role, name)| (role.as_str(), name.as_str()))
        .collect()
}

fn occurrence_key(base_key: &str, occurrence: usize) -> String {
    if occurrence == 0 {
        base_key.to_string()
    } else {
        format!("{base_key}#{occurrence}")
    }
}

fn keyed_children<'a>(
    nodes: &'a [AriaNode],
    ancestors: &[(String, String)],
) -> Vec<(String, &'a AriaNode)> {
    let refs = identity_refs(ancestors);
    let mut seen_per_key: HashMap<String, usize> = HashMap::new();

    nodes
        .iter()
        .map(|node| {
            let base_key = identity_key(&node.role, node_name(node), &refs);
            let occurrence = seen_per_key.entry(base_key.clone()).or_default();
            let key = occurrence_key(&base_key, *occurrence);
            *occurrence += 1;
            (key, node)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChange {
    pub ref_id: String,
    pub role: String,
    pub name: Option<String>,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AriaTreeDiff {
    pub added: Vec<(String, String)>,
    pub removed: Vec<(String, String)>,
    pub changed: Vec<StateChange>,
}

fn node_state_summary(node: &AriaNode) -> String {
    let mut parts = Vec::new();

    if node.states != crate::aria::AriaStates::default() {
        parts.push(format!("states={:?}", node.states));
    }
    if let Some(url) = &node.url {
        parts.push(format!("url={url}"));
    }
    if node.offscreen {
        parts.push("offscreen=true".to_string());
    }
    if node.omitted_children > 0 {
        parts.push(format!("omitted_children={}", node.omitted_children));
    }

    if parts.is_empty() {
        "default".to_string()
    } else {
        parts.join(", ")
    }
}

fn render_name(name: Option<&str>) -> String {
    serde_json::to_string(name.unwrap_or("")).unwrap_or_else(|_| "\"\"".to_string())
}

fn node_ref_or_identity(node: &AriaNode, ancestors: &[(String, String)]) -> String {
    node.ref_id
        .clone()
        .unwrap_or_else(|| identity_key(&node.role, node_name(node), &identity_refs(ancestors)))
}

fn same_identity(prev: &AriaNode, curr: &AriaNode, ancestors: &[(String, String)]) -> bool {
    let refs = identity_refs(ancestors);
    identity_key(&prev.role, node_name(prev), &refs)
        == identity_key(&curr.role, node_name(curr), &refs)
}

fn push_added_subtree(result: &mut AriaTreeDiff, node: &AriaNode, ancestors: &[(String, String)]) {
    result.added.push((
        node_ref_or_identity(node, ancestors),
        to_yaml(node, Some(DIFF_SNIPPET_DEPTH)),
    ));
}

fn push_removed_subtree(
    result: &mut AriaTreeDiff,
    node: &AriaNode,
    ancestors: &[(String, String)],
) {
    result.removed.push((
        node_ref_or_identity(node, ancestors),
        to_yaml(node, Some(DIFF_SNIPPET_DEPTH)),
    ));
}

fn diff_node(
    prev: &AriaNode,
    curr: &AriaNode,
    ancestors: &mut Vec<(String, String)>,
    result: &mut AriaTreeDiff,
) {
    let before = node_state_summary(prev);
    let after = node_state_summary(curr);
    if before != after {
        result.changed.push(StateChange {
            ref_id: node_ref_or_identity(curr, ancestors),
            role: curr.role.clone(),
            name: curr.name.clone(),
            before,
            after,
        });
    }

    ancestors.push((curr.role.clone(), node_name(curr).to_string()));
    let prev_children = keyed_children(&prev.children, ancestors);
    let curr_children = keyed_children(&curr.children, ancestors);

    let prev_map: HashMap<String, &AriaNode> = prev_children
        .iter()
        .map(|(key, node)| (key.clone(), *node))
        .collect();
    let curr_map: HashMap<String, &AriaNode> = curr_children
        .iter()
        .map(|(key, node)| (key.clone(), *node))
        .collect();

    for (key, prev_child) in &prev_children {
        if !curr_map.contains_key(key) {
            push_removed_subtree(result, prev_child, ancestors);
        }
    }

    for (key, curr_child) in &curr_children {
        if !prev_map.contains_key(key) {
            push_added_subtree(result, curr_child, ancestors);
        }
    }

    for (key, curr_child) in &curr_children {
        if let Some(prev_child) = prev_map.get(key) {
            diff_node(prev_child, curr_child, ancestors, result);
        }
    }

    ancestors.pop();
}

#[must_use]
pub fn diff_trees(prev: &AriaNode, curr: &AriaNode) -> AriaTreeDiff {
    let mut result = AriaTreeDiff::default();
    diff_node(prev, curr, &mut Vec::new(), &mut result);
    result
}

fn render_diff(diff: &AriaTreeDiff) -> String {
    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        return "no visible change".to_string();
    }

    let mut lines = Vec::new();

    for (ref_id, snippet) in &diff.added {
        lines.push(format!("+ [ref={ref_id}] added:"));
        for line in snippet.lines() {
            lines.push(format!("  {line}"));
        }
    }

    for (ref_id, snippet) in &diff.removed {
        lines.push(format!("- [ref={ref_id}] removed:"));
        for line in snippet.lines() {
            lines.push(format!("  {line}"));
        }
    }

    for change in &diff.changed {
        lines.push(format!(
            "~ [ref={}] changed: {} {}",
            change.ref_id,
            change.role,
            render_name(change.name.as_deref())
        ));
        lines.push(format!("  before: {}", change.before));
        lines.push(format!("  after: {}", change.after));
    }

    lines.join("\n")
}

fn should_fallback(diff: &AriaTreeDiff) -> bool {
    diff.added.len() + diff.removed.len() + diff.changed.len() > MAX_DIFF_ITEMS
}

fn full_snapshot_value(root: &AriaNode) -> Value {
    Value::String(to_yaml(root, Some(FEEDBACK_TREE_DEPTH)))
}

fn diff_summary_value(diff: &AriaTreeDiff) -> Value {
    let changed = !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty();
    json!({
        "changed": changed,
        "added": diff.added.len(),
        "removed": diff.removed.len(),
        "changed_states": diff.changed.len(),
    })
}

fn page_replaced_summary_value() -> Value {
    json!({
        "changed": true,
        "page_replaced": true,
        "added": 0,
        "removed": 0,
        "changed_states": 0,
    })
}

#[must_use]
pub fn build_diff_page_state(
    prev: Option<&AriaNode>,
    curr: Option<&AriaNode>,
    widen: bool,
) -> Value {
    match (prev, curr) {
        (None, None) => {
            if widen {
                Value::String("no visible change".to_string())
            } else {
                json!({"changed": false, "added": 0, "removed": 0, "changed_states": 0})
            }
        }
        (None, Some(curr)) => {
            let mut diff = AriaTreeDiff::default();
            push_added_subtree(&mut diff, curr, &[]);
            if widen {
                Value::String(render_diff(&diff))
            } else {
                diff_summary_value(&diff)
            }
        }
        (Some(prev), None) => {
            let mut diff = AriaTreeDiff::default();
            push_removed_subtree(&mut diff, prev, &[]);
            if widen {
                Value::String(render_diff(&diff))
            } else {
                diff_summary_value(&diff)
            }
        }
        (Some(prev), Some(curr)) => {
            if !same_identity(prev, curr, &[]) {
                if widen {
                    let mut diff = AriaTreeDiff::default();
                    push_removed_subtree(&mut diff, prev, &[]);
                    push_added_subtree(&mut diff, curr, &[]);
                    return Value::String(render_diff(&diff));
                }
                return page_replaced_summary_value();
            }

            let diff = diff_trees(prev, curr);
            if widen {
                if should_fallback(&diff) {
                    full_snapshot_value(curr)
                } else {
                    Value::String(render_diff(&diff))
                }
            } else {
                diff_summary_value(&diff)
            }
        }
    }
}

fn fallback_value() -> Value {
    json!({
        "url": "unknown",
        "title": "unknown",
        "page_map": null
    })
}

fn active_dialog_label(snapshot: &Value) -> Option<String> {
    let dialog = snapshot.get("active_dialog")?;
    if dialog
        .get("visible")
        .and_then(Value::as_bool)
        .is_some_and(|visible| !visible)
    {
        return None;
    }

    dialog
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
}

fn has_active_dialog(snapshot: Option<&Value>) -> bool {
    snapshot
        .and_then(|value| value.get("active_dialog"))
        .and_then(Value::as_object)
        .is_some_and(|dialog| {
            dialog
                .get("visible")
                .and_then(Value::as_bool)
                .is_none_or(|visible| visible)
        })
}

fn find_node_by_ref<'a>(node: &'a AriaNode, ref_id: &str) -> Option<&'a AriaNode> {
    if node.ref_id.as_deref() == Some(ref_id) {
        return Some(node);
    }

    node.children
        .iter()
        .find_map(|child| find_node_by_ref(child, ref_id))
}

fn find_dialog_node<'a>(node: &'a AriaNode, label: Option<&str>) -> Option<&'a AriaNode> {
    let is_dialog = matches!(node.role.as_str(), "dialog" | "alertdialog");
    let label_matches = label.is_none_or(|expected| node.name.as_deref() == Some(expected));
    if is_dialog && label_matches {
        return Some(node);
    }

    node.children
        .iter()
        .find_map(|child| find_dialog_node(child, label))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffScope {
    Full,
    Dialog(Option<String>),
    Ref(String),
}

fn resolve_diff_scope(
    ref_map: &browser::RefMap,
    previous_snapshot: Option<&Value>,
    current_snapshot: &Value,
    interacted_selector: Option<&str>,
    widen: bool,
) -> DiffScope {
    if widen {
        return DiffScope::Full;
    }

    if has_active_dialog(Some(current_snapshot)) || has_active_dialog(previous_snapshot) {
        return DiffScope::Dialog(
            active_dialog_label(current_snapshot)
                .or_else(|| previous_snapshot.and_then(active_dialog_label)),
        );
    }

    interacted_selector
        .and_then(|selector| ref_map.ref_id_for_query(selector))
        .map_or(DiffScope::Full, |ref_id| DiffScope::Ref(ref_id.to_string()))
}

fn scoped_tree<'a>(
    tree: &'a AriaNode,
    _snapshot: Option<&Value>,
    scope: &DiffScope,
) -> Option<&'a AriaNode> {
    match scope {
        DiffScope::Full => Some(tree),
        DiffScope::Dialog(label) => find_dialog_node(tree, label.as_deref()),
        DiffScope::Ref(ref_id) => find_node_by_ref(tree, ref_id),
    }
}

fn page_state_from_feedback_map(
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
    interacted_selector: Option<&str>,
    widen: bool,
    mut pm: Value,
) -> Value {
    // Enrich in place before caching so stored snapshots preserve regions and
    // active_dialog for later scoped interactions.
    super::page_map::enrich_semantic_sections(&mut pm);

    let full_url = extract_feedback_url(&pm).to_string();
    let cache_key = normalize_url(&full_url).to_string();

    let previous_snapshot = browser.page_snapshot_for_url(&cache_key, None).cloned();
    let url_changed = browser
        .snapshot_url()
        .is_some_and(|prev_url| prev_url != cache_key.as_str());
    if url_changed {
        browser.ref_map_mut().clear();
    }

    let Some(mut current_tree) = pm.get("tree").and_then(parse_raw_tree) else {
        let page_state = build_page_state_from_map(pm.clone());
        browser.set_page_snapshot(&cache_key, None, pm);
        return page_state;
    };
    browser.ref_map_mut().begin_snapshot();
    assign_refs(
        &mut current_tree,
        browser.ref_map_mut(),
        None,
        &mut Vec::new(),
        None,
    );

    // Prefer the cached snapshot's tree, but when the snapshot lacks one
    // (navigate caches url-only snapshots) fall back to the last full tree so
    // the first post-action feedback after navigate is a diff, not a dump.
    let previous_tree = previous_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get("tree").and_then(parse_raw_tree))
        .or_else(|| {
            if url_changed {
                None
            } else {
                crawl_state.last_aria_tree.clone()
            }
        });

    let scope = resolve_diff_scope(
        browser.ref_map(),
        previous_snapshot.as_ref(),
        &pm,
        interacted_selector,
        widen,
    );

    let page_state = if let Some(previous_tree) = previous_tree.as_ref() {
        build_diff_page_state(
            scoped_tree(previous_tree, previous_snapshot.as_ref(), &scope),
            scoped_tree(&current_tree, Some(&pm), &scope),
            widen,
        )
    } else {
        let scoped = scoped_tree(&current_tree, Some(&pm), &scope);
        if widen {
            scoped.map_or_else(|| full_snapshot_value(&current_tree), full_snapshot_value)
        } else {
            json!({"changed": true, "first_snapshot": true, "url": full_url, "added": 1, "removed": 0, "changed_states": 0})
        }
    };

    browser.set_page_snapshot(&cache_key, None, pm);
    // Feedback snapshots are always full-page, so the freshly walked tree is
    // the new diff/fingerprint baseline.
    crawl_state.last_aria_tree = Some(current_tree);
    page_state
}

/// Best-effort post-action page state for interaction tool responses.
///
/// Calls `page_map_feedback` on the bridge and applies Rust-side differential
/// comparison against the cached snapshot.
pub(crate) async fn post_action_page_state(
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
    interaction_kind: InteractionKind,
    interacted_selector: Option<&str>,
    widen: bool,
) -> Result<Value, ToolExecutionError> {
    let result = timeout(FEEDBACK_TIMEOUT, async {
        let mut bridge = browser.acquire_bridge().await?;
        let page_map = bridge.page_map_feedback(None).await?;
        Ok::<_, browser::BridgeError>(page_map)
    })
    .await;

    match result {
        Ok(Ok(pm)) => {
            let page_state =
                page_state_from_feedback_map(browser, crawl_state, interacted_selector, widen, pm);
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

    let unchanged = page_state.as_str() == Some("no visible change")
        || page_state.get("changed").and_then(Value::as_bool) == Some(false);
    if !unchanged {
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
        BridgeError, BrowserBackend, BrowserState, PageInfo, ScreenshotOptions, StorageEntry,
        StorageType,
    };
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use super::{
        build_diff_page_state, build_page_state_from_map, fallback_value, post_action_page_state,
        InteractionKind, FEEDBACK_TREE_DEPTH, RECAPTCHA_V3_SILENT_SUBMISSION_MESSAGE,
    };
    use crate::aria::{to_yaml, AriaNode, AriaStates};
    use crate::state::CrawlState;
    use crate::BrowserContext;

    #[derive(Debug, Default)]
    struct FeedbackMockState {
        page_maps: HashMap<String, Value>,
        requested_scopes: Vec<Option<String>>,
        requested_depths: Vec<Option<usize>>,
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
                html: Some(String::new()),
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
            depth: Option<usize>,
        ) -> Result<Value, BridgeError> {
            let mut state = self.state.lock().expect("mock state poisoned");
            state.requested_scopes.push(scope.map(str::to_string));
            state.requested_depths.push(depth);
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
                if script.contains(substring) {
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

    fn node(
        role: &str,
        name: Option<&str>,
        ref_id: Option<&str>,
        children: Vec<AriaNode>,
    ) -> AriaNode {
        AriaNode {
            role: role.to_string(),
            name: name.map(str::to_string),
            states: AriaStates::default(),
            ref_id: ref_id.map(str::to_string),
            url: None,
            frame_id: None,
            offscreen: false,
            children,
            omitted_children: 0,
        }
    }

    fn node_with_states(
        role: &str,
        name: Option<&str>,
        ref_id: Option<&str>,
        states: AriaStates,
        children: Vec<AriaNode>,
    ) -> AriaNode {
        AriaNode {
            role: role.to_string(),
            name: name.map(str::to_string),
            states,
            ref_id: ref_id.map(str::to_string),
            url: None,
            frame_id: None,
            offscreen: false,
            children,
            omitted_children: 0,
        }
    }

    fn document(children: Vec<AriaNode>) -> AriaNode {
        node("document", Some(""), Some("e1"), children)
    }

    fn raw_states(states: &AriaStates) -> Value {
        let mut map = serde_json::Map::new();
        if states.active {
            map.insert("active".into(), Value::Bool(true));
        }
        if states.checked {
            map.insert("checked".into(), Value::Bool(true));
        }
        if states.disabled {
            map.insert("disabled".into(), Value::Bool(true));
        }
        if let Some(expanded) = states.expanded {
            map.insert("expanded".into(), Value::Bool(expanded));
        }
        if states.invalid {
            map.insert("invalid".into(), Value::Bool(true));
        }
        if let Some(level) = states.level {
            map.insert("level".into(), json!(level));
        }
        if let Some(pressed) = states.pressed {
            map.insert("pressed".into(), Value::Bool(pressed));
        }
        if states.selected {
            map.insert("selected".into(), Value::Bool(true));
        }
        Value::Object(map)
    }

    fn raw_tree_value(node: &AriaNode) -> Value {
        json!({
            "role": node.role,
            "name": node.name.as_deref().unwrap_or(""),
            "states": raw_states(&node.states),
            "refId": node.ref_id,
            "url": node.url,
            "frameId": node.frame_id,
            "offscreen": node.offscreen,
            "omittedChildren": node.omitted_children,
            "children": node.children.iter().map(raw_tree_value).collect::<Vec<_>>()
        })
    }

    fn feedback_snapshot(
        url: &str,
        title: &str,
        tree: &AriaNode,
        active_dialog_label: Option<&str>,
    ) -> Value {
        json!({
            "tree": raw_tree_value(tree),
            "headings": [],
            "landmarks": [],
            "links": [],
            "forms": [],
            "interactive": {"elements": []},
            "active_dialog": active_dialog_label.map_or(Value::Null, |label| json!({
                "selector": "#dialog",
                "label": label,
                "visible": true
            })),
            "meta": {"title": title, "url": url, "description": ""},
            "url": url,
            "title": title
        })
    }

    fn page_map_fixture(url: &str, title: &str) -> Value {
        json!({
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
                "title": title,
                "url": url,
                "description": "A test page"
            }
        })
    }

    #[test]
    fn feedback_fallback_value_has_correct_shape() {
        let val = fallback_value();
        assert_eq!(val["url"], "unknown");
        assert_eq!(val["title"], "unknown");
        assert!(val["page_map"].is_null());
    }

    #[test]
    fn feedback_success_value_from_mock_page_map() {
        let result =
            build_page_state_from_map(page_map_fixture("https://example.com/page", "Example Page"));

        assert_eq!(result["url"], "https://example.com/page");
        assert_eq!(result["title"], "Example Page");
        assert!(result["page_map"]["headings"].is_array());
        assert!(result["page_map"].get("forms").is_none());
        assert!(result["page_map"].get("interactive").is_none());
        assert!(result["page_map"].get("controls").is_none());
    }

    #[test]
    fn tree_diff_modal_open_returns_added_subtree() {
        let prev = document(vec![node("main", Some(""), Some("e2"), vec![])]);
        let curr = document(vec![
            node("main", Some(""), Some("e2"), vec![]),
            node(
                "dialog",
                Some("Confirm delete"),
                Some("e3"),
                vec![node("button", Some("Delete"), Some("e4"), vec![])],
            ),
        ]);

        let result = build_diff_page_state(Some(&prev), Some(&curr), true);
        let rendered = result.as_str().expect("tree diff should be a string");

        assert!(rendered.contains("added:"));
        assert!(rendered.contains("dialog \"Confirm delete\""));
    }

    #[test]
    fn tree_diff_modal_close_returns_removed_subtree() {
        let prev = document(vec![
            node("main", Some(""), Some("e2"), vec![]),
            node(
                "dialog",
                Some("Confirm delete"),
                Some("e3"),
                vec![node("button", Some("Delete"), Some("e4"), vec![])],
            ),
        ]);
        let curr = document(vec![node("main", Some(""), Some("e2"), vec![])]);

        let result = build_diff_page_state(Some(&prev), Some(&curr), true);
        let rendered = result.as_str().expect("tree diff should be a string");

        assert!(rendered.contains("removed:"));
        assert!(rendered.contains("dialog \"Confirm delete\""));
    }

    #[test]
    fn tree_diff_state_toggle_returns_changed_entry() {
        let prev = document(vec![node_with_states(
            "button",
            Some("Submit"),
            Some("e2"),
            AriaStates::default(),
            vec![],
        )]);
        let curr = document(vec![node_with_states(
            "button",
            Some("Submit"),
            Some("e9"),
            AriaStates {
                disabled: true,
                ..AriaStates::default()
            },
            vec![],
        )]);

        let result = build_diff_page_state(Some(&prev), Some(&curr), true);
        let rendered = result.as_str().expect("tree diff should be a string");

        assert!(rendered.contains("changed:"));
        assert!(rendered.contains("button \"Submit\""));
        assert!(rendered.contains("states=AriaStates"));
    }

    #[test]
    fn tree_diff_unchanged_page_returns_no_visible_change() {
        let prev = document(vec![node("button", Some("Submit"), Some("e2"), vec![])]);
        let curr = document(vec![node("button", Some("Submit"), Some("e9"), vec![])]);

        let result = build_diff_page_state(Some(&prev), Some(&curr), false);
        assert_eq!(
            result,
            json!({"changed": false, "added": 0, "removed": 0, "changed_states": 0})
        );
    }

    #[test]
    fn tree_diff_ignores_ref_id_churn_for_same_identity() {
        let prev = document(vec![node("button", Some("Save"), Some("e2"), vec![])]);
        let curr = document(vec![node("button", Some("Save"), Some("e99"), vec![])]);

        let result = build_diff_page_state(Some(&prev), Some(&curr), false);
        assert_eq!(
            result,
            json!({"changed": false, "added": 0, "removed": 0, "changed_states": 0})
        );
    }

    #[test]
    fn tree_diff_large_change_falls_back_to_full_yaml_snapshot() {
        let prev = document(vec![node("main", Some(""), Some("e2"), vec![])]);
        let curr = document(
            (0..31)
                .map(|idx| {
                    node(
                        "button",
                        Some(&format!("Action {idx}")),
                        Some(&format!("e{}", idx + 2)),
                        vec![],
                    )
                })
                .collect(),
        );

        assert_eq!(
            build_diff_page_state(Some(&prev), Some(&curr), true),
            Value::String(to_yaml(&curr, Some(FEEDBACK_TREE_DEPTH)))
        );
    }

    #[test]
    fn tree_diff_root_identity_change_replaces_whole_subtree() {
        let prev = node(
            "dialog",
            Some("Login"),
            Some("e1"),
            vec![node("button", Some("Sign in"), Some("e2"), vec![])],
        );
        let curr = node(
            "main",
            Some(""),
            Some("e1"),
            vec![node("heading", Some("Welcome"), Some("e2"), vec![])],
        );

        let result = build_diff_page_state(Some(&prev), Some(&curr), true);

        let expected = [
            "+ [ref=e1] added:",
            "  - main \"\" [ref=e1]:",
            "    - heading \"Welcome\" [ref=e2]:",
            "- [ref=e1] removed:",
            "  - dialog \"Login\" [ref=e1]:",
            "    - button \"Sign in\" [ref=e2]:",
        ]
        .join("\n");

        assert_eq!(result, Value::String(expected));
    }

    #[test]
    fn tree_diff_root_identity_change_summary_marks_page_replaced() {
        let prev = node(
            "dialog",
            Some("Login"),
            Some("e1"),
            vec![node("button", Some("Sign in"), Some("e2"), vec![])],
        );
        let curr = node(
            "main",
            Some(""),
            Some("e1"),
            vec![node("heading", Some("Welcome"), Some("e2"), vec![])],
        );

        let result = build_diff_page_state(Some(&prev), Some(&curr), false);

        assert_eq!(
            result,
            json!({"changed": true, "page_replaced": true, "added": 0, "removed": 0, "changed_states": 0})
        );
    }

    #[test]
    fn url_without_hash_strips_fragment() {
        use super::super::page_map::normalize_url;

        assert_eq!(
            normalize_url("https://example.com/page#section"),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_url("https://app.com/#/dashboard"),
            "https://app.com/#/dashboard"
        );
    }

    #[tokio::test]
    async fn post_action_page_state_scopes_to_active_dialog_subtree() {
        let url = "https://example.com/modal";
        let prev_tree = document(vec![
            node("main", Some(""), Some("e2"), vec![]),
            node("button", Some("Open modal"), Some("e3"), vec![]),
        ]);
        let curr_tree = document(vec![
            node("main", Some(""), Some("e2"), vec![]),
            node("button", Some("Open modal"), Some("e3"), vec![]),
            node(
                "dialog",
                Some("Confirm delete"),
                Some("e4"),
                vec![node("button", Some("Delete"), Some("e5"), vec![])],
            ),
        ]);

        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(
                    String::new(),
                    feedback_snapshot(url, "Modal", &curr_tree, Some("Confirm delete")),
                )]),
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, feedback_snapshot(url, "Modal", &prev_tree, None));

        let result = post_action_page_state(
            &mut browser,
            &mut CrawlState::default(),
            InteractionKind::Passive,
            Some("#open-modal"),
            false,
        )
        .await
        .unwrap();

        assert_eq!(result["changed"], true);
        assert_eq!(result["added"], 1);

        let state = state.lock().expect("mock state poisoned");
        assert_eq!(state.requested_scopes, vec![None]);
        // Feedback snapshots always use the walk's default depth.
        assert_eq!(state.requested_depths, vec![None]);
    }

    #[tokio::test]
    async fn post_action_after_navigate_diffs_against_last_aria_tree() {
        let url = "https://example.com/nav";
        let prev_tree = document(vec![node("button", Some("Open modal"), Some("e2"), vec![])]);
        let curr_tree = document(vec![
            node("button", Some("Open modal"), Some("e2"), vec![]),
            node("dialog", Some("Confirm"), Some("e3"), vec![]),
        ]);

        let (mut browser, _state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(
                    String::new(),
                    feedback_snapshot(url, "Nav", &curr_tree, None),
                )]),
                ..FeedbackMockState::default()
            },
            url,
        );
        // navigate caches a url-only snapshot without a tree; the last full
        // tree lives in crawl_state.last_aria_tree.
        browser.set_page_snapshot(url, None, json!({ "meta": { "url": url } }));
        let mut crawl_state = CrawlState {
            last_aria_tree: Some(prev_tree),
            ..CrawlState::default()
        };

        let result = post_action_page_state(
            &mut browser,
            &mut crawl_state,
            InteractionKind::Passive,
            None,
            true,
        )
        .await
        .unwrap();

        let rendered = result.as_str().expect("page state should be a string");
        assert!(
            rendered.contains("added:"),
            "expected a diff, got: {rendered}"
        );
        assert!(rendered.contains("dialog \"Confirm\""));

        // The freshly walked tree becomes the new diff baseline.
        let refreshed = crawl_state
            .last_aria_tree
            .expect("last_aria_tree should be refreshed");
        assert_eq!(refreshed.children.len(), 2);
    }

    #[tokio::test]
    async fn post_action_page_state_large_change_falls_back_to_full_yaml() {
        let url = "https://example.com/fallback";
        let prev_tree = document(vec![node("main", Some(""), Some("e2"), vec![])]);
        let curr_tree = document(
            (0..31)
                .map(|idx| {
                    node(
                        "button",
                        Some(&format!("Action {idx}")),
                        Some(&format!("e{}", idx + 2)),
                        vec![],
                    )
                })
                .collect(),
        );

        let (mut browser, _state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(
                    String::new(),
                    feedback_snapshot(url, "Fallback", &curr_tree, None),
                )]),
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(
            url,
            None,
            feedback_snapshot(url, "Fallback", &prev_tree, None),
        );

        let result = post_action_page_state(
            &mut browser,
            &mut CrawlState::default(),
            InteractionKind::Passive,
            Some("#trigger"),
            true,
        )
        .await
        .unwrap();

        println!(
            "{}",
            result.as_str().expect("page state should be a string")
        );
        assert_eq!(
            result,
            Value::String(to_yaml(&curr_tree, Some(FEEDBACK_TREE_DEPTH)))
        );
    }

    #[tokio::test]
    async fn silent_submit_fires_on_silent_v3_rejection() {
        let url = "https://example.com/form";
        let tree = document(vec![node("form", Some("Signup"), Some("e2"), vec![])]);
        let snapshot = feedback_snapshot(url, "Form", &tree, None);

        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(String::new(), snapshot.clone())]),
                evaluate_script_results: vec![("grecaptcha".to_string(), json!({"value": true}))],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, snapshot);

        let result = post_action_page_state(
            &mut browser,
            &mut CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await;

        let err = result.expect_err("silent v3 rejection should error");
        assert_eq!(err.to_string(), RECAPTCHA_V3_SILENT_SUBMISSION_MESSAGE);

        let state = state.lock().expect("mock state poisoned");
        assert_eq!(state.evaluate_scripts.len(), 1);
    }

    #[tokio::test]
    async fn silent_submit_does_not_fire_when_page_changed() {
        let url = "https://example.com/form";
        let prev_tree = document(vec![node("form", Some("Signup"), Some("e2"), vec![])]);
        let curr_tree = document(vec![node("heading", Some("Thanks"), Some("e2"), vec![])]);

        let (mut browser, state) = browser_with_feedback_backend(
            FeedbackMockState {
                page_maps: HashMap::from([(
                    String::new(),
                    feedback_snapshot(url, "Form", &curr_tree, None),
                )]),
                evaluate_script_results: vec![("grecaptcha".to_string(), json!({"value": true}))],
                ..FeedbackMockState::default()
            },
            url,
        );
        browser.set_page_snapshot(url, None, feedback_snapshot(url, "Form", &prev_tree, None));

        let result = post_action_page_state(
            &mut browser,
            &mut CrawlState::default(),
            InteractionKind::PossibleSubmit,
            None,
            false,
        )
        .await
        .unwrap();

        let changed = result["changed"]
            .as_bool()
            .expect("summary should have changed field");
        assert!(changed, "page should have changed");

        let state = state.lock().expect("mock state poisoned");
        assert!(state.evaluate_scripts.is_empty());
    }
}
