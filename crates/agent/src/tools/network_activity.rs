use std::cmp::Ordering;
use std::collections::{hash_map::Entry, BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use browser::{NetworkRequestEvent, RequestState};
use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestFilter {
    All,
    Xhr,
    Failed,
    Pending,
    Aborted,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Slowest,
    Fastest,
    Largest,
    Smallest,
    Newest,
    Oldest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SinceBound {
    All,
    Last,
    Seq(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UntilBound {
    Now,
    Seq(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListInput {
    since: SinceBound,
    until: UntilBound,
    filter: RequestFilter,
    pattern: Option<String>,
    sort_by: Vec<SortKey>,
    limit: usize,
    unique_urls: bool,
    method: Option<String>,
    min_size_kb: Option<u64>,
    max_size_kb: Option<u64>,
}

fn parse_list_input(input: &Value) -> Result<ListInput, CrawlError> {
    let since = match input.get("since") {
        None | Some(Value::Null) => SinceBound::Last,
        Some(Value::String(value)) if value == "all" => SinceBound::All,
        Some(Value::String(value)) if value == "last" => SinceBound::Last,
        Some(Value::Number(value)) => SinceBound::Seq(value.as_u64().ok_or_else(|| {
            CrawlError::new("'since' must be 'all', 'last', or a non-negative seq number")
        })?),
        _ => {
            return Err(CrawlError::new(
                "'since' must be 'all', 'last', or a non-negative seq number",
            ));
        }
    };

    let until = match input.get("until") {
        None | Some(Value::Null) => UntilBound::Now,
        Some(Value::String(value)) if value == "now" => UntilBound::Now,
        Some(Value::Number(value)) => UntilBound::Seq(value.as_u64().ok_or_else(|| {
            CrawlError::new("'until' must be 'now' or a non-negative seq number")
        })?),
        _ => {
            return Err(CrawlError::new(
                "'until' must be 'now' or a non-negative seq number",
            ))
        }
    };

    let filter = match input.get("filter").and_then(Value::as_str).unwrap_or("all") {
        "all" => RequestFilter::All,
        "xhr" => RequestFilter::Xhr,
        "failed" => RequestFilter::Failed,
        "pending" => RequestFilter::Pending,
        "aborted" => RequestFilter::Aborted,
        "media" => RequestFilter::Media,
        _ => {
            return Err(CrawlError::new(
                "'filter' must be one of: all, xhr, failed, pending, aborted, media",
            ));
        }
    };

    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.is_empty());

    let sort_by = input
        .get("sort_by")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| match item.as_str() {
                    Some("slowest") => Ok(SortKey::Slowest),
                    Some("fastest") => Ok(SortKey::Fastest),
                    Some("largest") => Ok(SortKey::Largest),
                    Some("smallest") => Ok(SortKey::Smallest),
                    Some("newest") => Ok(SortKey::Newest),
                    Some("oldest") => Ok(SortKey::Oldest),
                    _ => Err(CrawlError::new(
                        "'sort_by' entries must be one of: slowest, fastest, largest, smallest, newest, oldest",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| vec![SortKey::Oldest]);

    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LIMIT, |value| {
            usize::try_from(value).unwrap_or(MAX_LIMIT).min(MAX_LIMIT)
        });

    let unique_urls = input
        .get("unique_urls")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let method = input
        .get("method")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_uppercase())
        .filter(|value| !value.is_empty());

    let min_size_kb = input.get("min_size_kb").and_then(Value::as_u64);
    let max_size_kb = input.get("max_size_kb").and_then(Value::as_u64);

    if let (Some(min), Some(max)) = (min_size_kb, max_size_kb) {
        if min > max {
            return Err(CrawlError::new("min_size_kb cannot exceed max_size_kb"));
        }
    }

    Ok(ListInput {
        since,
        until,
        filter,
        pattern,
        sort_by,
        limit,
        unique_urls,
        method,
        min_size_kb,
        max_size_kb,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            #[allow(clippy::cast_possible_truncation)]
            let ms = duration.as_millis() as u64;
            ms
        })
}

fn normalize_request_type(request_type: &str) -> String {
    match request_type.to_ascii_lowercase().as_str() {
        "fetch" | "xhr" => "xhr".to_string(),
        "script" => "script".to_string(),
        "stylesheet" => "stylesheet".to_string(),
        "image" => "image".to_string(),
        "font" => "font".to_string(),
        "document" => "document".to_string(),
        _ => "other".to_string(),
    }
}

fn request_matches_filter(event: &NetworkRequestEvent, filter: RequestFilter) -> bool {
    match filter {
        RequestFilter::All => true,
        RequestFilter::Xhr => normalize_request_type(&event.request_type) == "xhr",
        RequestFilter::Failed => event.state == RequestState::Failed,
        RequestFilter::Pending => event.state == RequestState::Pending,
        RequestFilter::Aborted => event.state == RequestState::Aborted,
        RequestFilter::Media => event.response_headers.as_ref().is_some_and(|headers| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
                .is_some_and(|(_, value)| {
                    let value = value.to_ascii_lowercase();
                    value.starts_with("video/") || value.starts_with("audio/")
                })
        }),
    }
}

fn state_name(state: RequestState) -> &'static str {
    match state {
        RequestState::Pending => "pending",
        RequestState::Completed => "completed",
        RequestState::Failed => "failed",
        RequestState::Aborted => "aborted",
    }
}

fn compare_requests(
    left: &NetworkRequestEvent,
    right: &NetworkRequestEvent,
    sort_by: &[SortKey],
) -> Ordering {
    for key in sort_by {
        let ordering = match key {
            SortKey::Slowest => right.duration_ms.cmp(&left.duration_ms),
            SortKey::Fastest => left.duration_ms.cmp(&right.duration_ms),
            SortKey::Largest => right.size_bytes.cmp(&left.size_bytes),
            SortKey::Smallest => left.size_bytes.cmp(&right.size_bytes),
            SortKey::Newest => right.timestamp_ms.cmp(&left.timestamp_ms),
            SortKey::Oldest => left.timestamp_ms.cmp(&right.timestamp_ms),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    left.request_id.cmp(&right.request_id)
}

fn latest_requests(events: &[NetworkRequestEvent]) -> Vec<NetworkRequestEvent> {
    let mut by_request_id = HashMap::new();
    for event in events {
        by_request_id.insert(event.request_id.clone(), event.clone());
    }
    by_request_id.into_values().collect()
}

fn within_size_bounds(
    event: &NetworkRequestEvent,
    min_size_kb: Option<u64>,
    max_size_kb: Option<u64>,
) -> bool {
    let size = event.size_bytes.unwrap_or(0);
    if let Some(min) = min_size_kb {
        if size < min.saturating_mul(1024) {
            return false;
        }
    }
    if let Some(max) = max_size_kb {
        if size > max.saturating_mul(1024) {
            return false;
        }
    }
    true
}

fn deduplicate_by_url(
    events: Vec<NetworkRequestEvent>,
) -> (Vec<NetworkRequestEvent>, HashMap<String, u32>) {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut by_url: HashMap<String, NetworkRequestEvent> = HashMap::new();
    for event in events {
        *counts.entry(event.url.clone()).or_insert(0) += 1;
        match by_url.entry(event.url.clone()) {
            Entry::Occupied(mut occupied) => {
                let existing = occupied.get_mut();
                let max_size = existing.size_bytes.max(event.size_bytes);
                let event_is_later = (event.seq_at_initiation, event.timestamp_ms)
                    > (existing.seq_at_initiation, existing.timestamp_ms);
                if event_is_later {
                    *existing = event;
                }
                existing.size_bytes = max_size;
            }
            Entry::Vacant(vacant) => {
                vacant.insert(event);
            }
        }
    }
    (by_url.into_values().collect(), counts)
}

fn matching_requests(
    crawl_state: &CrawlState,
    input: &ListInput,
) -> (Vec<NetworkRequestEvent>, HashMap<String, u32>) {
    let since = match input.since {
        SinceBound::All => 0,
        SinceBound::Last => crawl_state.seq_counter.current().saturating_sub(1),
        SinceBound::Seq(value) => value,
    };
    let until = match input.until {
        UntilBound::Now => None,
        UntilBound::Seq(value) => Some(value),
    };

    let filtered = latest_requests(&crawl_state.network_request_events)
        .into_iter()
        .filter(|event| {
            event.seq_at_initiation >= since
                && until.is_none_or(|upper_bound| event.seq_at_initiation < upper_bound)
        })
        .filter(|event| request_matches_filter(event, input.filter))
        .filter(|event| {
            input
                .pattern
                .as_deref()
                .is_none_or(|pattern| event.url.contains(pattern))
        })
        .filter(|event| {
            input
                .method
                .as_deref()
                .is_none_or(|method| event.method.eq_ignore_ascii_case(method))
        })
        .filter(|event| within_size_bounds(event, input.min_size_kb, input.max_size_kb))
        .collect::<Vec<_>>();

    let (mut requests, request_counts) = if input.unique_urls {
        deduplicate_by_url(filtered)
    } else {
        (filtered, HashMap::new())
    };

    requests.sort_by(|left, right| compare_requests(left, right, &input.sort_by));
    (requests, request_counts)
}

fn build_by_type(events: &[NetworkRequestEvent]) -> BTreeMap<String, usize> {
    let mut by_type = BTreeMap::new();
    for event in events {
        *by_type
            .entry(normalize_request_type(&event.request_type))
            .or_insert(0) += 1;
    }
    by_type
}

fn build_request_row(
    event: &NetworkRequestEvent,
    id: &str,
    now_ms: u64,
    request_count: Option<u32>,
) -> Value {
    let content_type = event
        .response_headers
        .as_ref()
        .and_then(|headers| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        })
        .map_or(Value::Null, |(_, value)| json!(value));

    let mut row = json!({
        "id": id,
        "url": event.url,
        "method": event.method,
        "status": event.status,
        "size_kb": event.size_bytes.map(|value| {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let kb = ((value as f64) / 1024.0).round() as u64;
            kb
        }),
        "duration_ms": event.duration_ms,
        "type": normalize_request_type(&event.request_type),
        "state": state_name(event.state),
        "content_type": content_type,
        "initiated_ms_ago": (event.state == RequestState::Pending)
            .then_some(now_ms.saturating_sub(event.timestamp_ms)),
    });

    if let Some(count) = request_count {
        if let Some(object) = row.as_object_mut() {
            object.insert("request_count".to_string(), json!(count));
        }
    }

    row
}

pub async fn list_network_activity(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let input = parse_list_input(input)?;
    let polled = browser
        .acquire_bridge()
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?
        .poll_observations()
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?;

    crawl_state.ingest_observations(polled);

    let (all_matching, request_counts) = matching_requests(crawl_state, &input);
    let truncated = all_matching.len() > input.limit;
    let visible = all_matching
        .iter()
        .take(input.limit)
        .cloned()
        .collect::<Vec<_>>();
    let now_ms = now_ms();

    crawl_state.network_request_refs.clear();
    let requests = visible
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let id = format!("@r{}", index + 1);
            crawl_state
                .network_request_refs
                .insert(id.clone(), event.clone());
            let request_count = if input.unique_urls {
                request_counts.get(&event.url).copied()
            } else {
                None
            };
            build_request_row(event, &id, now_ms, request_count)
        })
        .collect::<Vec<_>>();

    let mut completed = 0;
    let mut failed = 0;
    let mut pending = 0;
    let mut aborted = 0;
    let mut total_transfer_bytes = 0_u64;
    for event in &all_matching {
        total_transfer_bytes = total_transfer_bytes.saturating_add(event.size_bytes.unwrap_or(0));
        match event.state {
            RequestState::Completed => completed += 1,
            RequestState::Failed => failed += 1,
            RequestState::Pending => pending += 1,
            RequestState::Aborted => aborted += 1,
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let total_transfer_kb = ((total_transfer_bytes as f64) / 1024.0).round() as u64;

    Ok(ToolEffect::reply_json(&json!({
        "summary": {
            "total": all_matching.len(),
            "completed": completed,
            "failed": failed,
            "pending": pending,
            "aborted": aborted,
            "total_transfer_kb": total_transfer_kb,
        },
        "by_type": build_by_type(&all_matching),
        "requests": requests,
        "truncated": truncated,
    })))
}

pub fn inspect_request(
    input: &Value,
    _browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolExecutionError::new("missing required field: id"))?;
    let include_body = input
        .get("include_body")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let event = crawl_state
        .network_request_refs
        .get(id)
        .ok_or_else(|| ToolExecutionError::new(format!("unknown request id: `{id}`")))?;

    let headers_to_json = |headers: &Option<BTreeMap<String, String>>| -> Value {
        headers
            .as_ref()
            .and_then(|map| serde_json::to_value(map).ok())
            .unwrap_or(Value::Null)
    };

    let (request_body, response_body) = if include_body {
        (
            event
                .request_body
                .clone()
                .map_or(Value::Null, Value::String),
            event
                .response_body
                .clone()
                .map_or(Value::Null, Value::String),
        )
    } else {
        (Value::Null, Value::Null)
    };

    let timing = event.timing.as_ref().map_or_else(
        || {
            json!({
                "dns_ms": Value::Null,
                "connect_ms": Value::Null,
                "tls_ms": Value::Null,
                "ttfb_ms": Value::Null,
                "download_ms": Value::Null,
                "total_duration_ms": event.duration_ms,
            })
        },
        |t| {
            json!({
                "dns_ms": t.dns_ms,
                "connect_ms": t.connect_ms,
                "tls_ms": t.tls_ms,
                "ttfb_ms": t.ttfb_ms,
                "download_ms": t.download_ms,
                "total_duration_ms": event.duration_ms,
            })
        },
    );

    Ok(ToolEffect::reply_json(&json!({
        "url": event.url,
        "method": event.method,
        "status": event.status,
        "state": state_name(event.state),
        "request_headers": headers_to_json(&event.request_headers),
        "response_headers": headers_to_json(&event.response_headers),
        "request_body": request_body,
        "response_body": response_body,
        "timing": timing,
        "initiator": {
            "type": event.initiator_type,
            "file": Value::Null,
            "line": Value::Null,
        },
        "from_service_worker": event.from_service_worker,
        "note": "Headers and timing are captured at request completion. Bodies require include_body=true and are captured only for textual responses (truncated).",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser::ObservationEvent;

    fn event(
        request_id: &str,
        seq_at_initiation: u64,
        timestamp_ms: u64,
        duration_ms: Option<u64>,
        size_bytes: Option<u64>,
        request_type: &str,
        state: RequestState,
    ) -> NetworkRequestEvent {
        NetworkRequestEvent {
            timestamp_ms,
            tab_index: 0,
            seq_at_initiation,
            request_id: request_id.to_string(),
            url: format!("https://example.com/{request_id}"),
            method: "GET".to_string(),
            status: Some(200),
            state,
            size_bytes,
            duration_ms,
            request_type: request_type.to_string(),
            from_service_worker: false,
            initiator_type: Some("script".to_string()),
            reason: None,
            timing: None,
            request_headers: None,
            response_headers: None,
            request_body: None,
            response_body: None,
        }
    }

    #[tokio::test]
    async fn list_network_activity_surfaces_request_through_bridge() {
        use crate::tools::test_support::browser_with_observations;

        let observations = vec![ObservationEvent::NetworkRequest(Box::new(event(
            "data",
            1,
            10,
            Some(16),
            Some(128),
            "fetch",
            RequestState::Completed,
        )))];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_network_activity(&json!({ "since": "all" }), &mut browser, &mut state)
            .await
            .expect("list_network_activity should succeed");
        let rendered = format!("{effect:?}");

        assert!(rendered.contains("https://example.com/data"));
        assert!(rendered.contains("@r1"));
        assert!(state.network_request_refs.contains_key("@r1"));
    }

    #[test]
    fn parse_input_defaults() {
        let input = parse_list_input(&json!({})).expect("input should parse");
        assert_eq!(input.since, SinceBound::Last);
        assert_eq!(input.until, UntilBound::Now);
        assert_eq!(input.filter, RequestFilter::All);
        assert_eq!(input.sort_by, vec![SortKey::Oldest]);
        assert_eq!(input.limit, DEFAULT_LIMIT);
        assert!(!input.unique_urls);
        assert_eq!(input.method, None);
        assert_eq!(input.min_size_kb, None);
        assert_eq!(input.max_size_kb, None);
    }

    #[test]
    fn matching_requests_uses_half_open_interval() {
        let state = CrawlState {
            network_request_events: vec![
                event(
                    "one",
                    1,
                    10,
                    Some(10),
                    Some(100),
                    "fetch",
                    RequestState::Completed,
                ),
                event(
                    "two",
                    2,
                    20,
                    Some(20),
                    Some(200),
                    "fetch",
                    RequestState::Completed,
                ),
                event(
                    "three",
                    3,
                    30,
                    Some(30),
                    Some(300),
                    "fetch",
                    RequestState::Completed,
                ),
            ],
            ..CrawlState::default()
        };
        let input = parse_list_input(&json!({"since": 2, "until": 3})).unwrap();

        let (requests, _counts) = matching_requests(&state, &input);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request_id, "two");
    }

    #[test]
    fn matching_requests_sorts_by_adjective_pairs() {
        let state = CrawlState {
            network_request_events: vec![
                event(
                    "a",
                    1,
                    100,
                    Some(100),
                    Some(1000),
                    "fetch",
                    RequestState::Completed,
                ),
                event(
                    "b",
                    1,
                    300,
                    Some(100),
                    Some(500),
                    "fetch",
                    RequestState::Completed,
                ),
                event(
                    "c",
                    1,
                    200,
                    Some(50),
                    Some(2000),
                    "fetch",
                    RequestState::Completed,
                ),
            ],
            ..CrawlState::default()
        };
        let input = parse_list_input(&json!({"sort_by": ["slowest", "newest"]})).unwrap();

        let (requests, _counts) = matching_requests(&state, &input);
        let ids = requests
            .into_iter()
            .map(|request| request.request_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }

    #[test]
    fn latest_requests_keeps_latest_state_per_request() {
        let requests = latest_requests(&[
            event("dup", 1, 100, None, None, "fetch", RequestState::Pending),
            event(
                "dup",
                1,
                200,
                Some(50),
                Some(128),
                "fetch",
                RequestState::Completed,
            ),
        ]);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].state, RequestState::Completed);
        assert_eq!(requests[0].duration_ms, Some(50));
    }

    #[test]
    fn normalize_request_type_maps_fetch_and_unknown() {
        assert_eq!(normalize_request_type("fetch"), "xhr");
        assert_eq!(normalize_request_type("xhr"), "xhr");
        assert_eq!(normalize_request_type("beacon"), "other");
    }

    #[test]
    fn inspect_request_surfaces_timing_headers_and_gated_bodies() {
        use crate::tools::test_support::browser_with_observations;

        let mut request_headers = BTreeMap::new();
        request_headers.insert("accept".to_string(), "application/json".to_string());
        let mut response_headers = BTreeMap::new();
        response_headers.insert("content-type".to_string(), "application/json".to_string());

        let mut req_event = event(
            "x",
            1,
            10,
            Some(20),
            Some(128),
            "fetch",
            RequestState::Completed,
        );
        req_event.timing = Some(browser::RequestTiming {
            dns_ms: Some(1),
            connect_ms: Some(2),
            tls_ms: Some(3),
            ttfb_ms: Some(4),
            download_ms: Some(5),
        });
        req_event.request_headers = Some(request_headers);
        req_event.response_headers = Some(response_headers);
        req_event.request_body = Some("REQ_BODY_MARKER".to_string());
        req_event.response_body = Some("RESP_BODY_MARKER".to_string());

        let mut browser = browser_with_observations(vec![]);
        let mut state = CrawlState::default();
        state
            .network_request_refs
            .insert("@r1".to_string(), req_event);

        let with_body = inspect_request(
            &json!({ "id": "@r1", "include_body": true }),
            &mut browser,
            &mut state,
        )
        .expect("inspect_request should succeed");
        let rendered = format!("{with_body:?}");
        assert!(rendered.contains("ttfb_ms"));
        assert!(rendered.contains("application/json"));
        assert!(rendered.contains("REQ_BODY_MARKER"));
        assert!(rendered.contains("RESP_BODY_MARKER"));

        let without_body = inspect_request(&json!({ "id": "@r1" }), &mut browser, &mut state)
            .expect("inspect_request should succeed");
        let rendered_no_body = format!("{without_body:?}");
        assert!(rendered_no_body.contains("application/json"));
        assert!(rendered_no_body.contains("ttfb_ms"));
        assert!(!rendered_no_body.contains("REQ_BODY_MARKER"));
        assert!(!rendered_no_body.contains("RESP_BODY_MARKER"));
    }

    fn reply_payload(effect: &ToolEffect) -> Value {
        match effect {
            ToolEffect::Reply(body) => {
                serde_json::from_str(body).expect("reply body should be valid json")
            }
            other => panic!("expected reply effect, got {other:?}"),
        }
    }

    fn row_urls(payload: &Value) -> Vec<String> {
        payload["requests"]
            .as_array()
            .expect("requests should be an array")
            .iter()
            .map(|row| {
                row["url"]
                    .as_str()
                    .expect("row url should be a string")
                    .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn media_filter_matches_video_and_audio_content_types() {
        use crate::tools::test_support::browser_with_observations;

        let mut video = event(
            "vid",
            1,
            10,
            Some(10),
            Some(1000),
            "media",
            RequestState::Completed,
        );
        video.response_headers = Some(BTreeMap::from([(
            "Content-Type".to_string(),
            "video/mp4".to_string(),
        )]));
        let mut audio = event(
            "aud",
            2,
            20,
            Some(10),
            Some(1000),
            "media",
            RequestState::Completed,
        );
        audio.response_headers = Some(BTreeMap::from([(
            "content-type".to_string(),
            "audio/ogg".to_string(),
        )]));
        let mut html = event(
            "doc",
            3,
            30,
            Some(10),
            Some(1000),
            "document",
            RequestState::Completed,
        );
        html.response_headers = Some(BTreeMap::from([(
            "content-type".to_string(),
            "text/html".to_string(),
        )]));
        let no_headers = event(
            "bare",
            4,
            40,
            Some(10),
            Some(1000),
            "fetch",
            RequestState::Completed,
        );

        let observations = vec![
            ObservationEvent::NetworkRequest(Box::new(video)),
            ObservationEvent::NetworkRequest(Box::new(audio)),
            ObservationEvent::NetworkRequest(Box::new(html)),
            ObservationEvent::NetworkRequest(Box::new(no_headers)),
        ];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_network_activity(
            &json!({ "since": "all", "filter": "media" }),
            &mut browser,
            &mut state,
        )
        .await
        .expect("list_network_activity should succeed");
        let payload = reply_payload(&effect);

        let urls = row_urls(&payload);
        assert_eq!(urls.len(), 2, "only video and audio responses match media");
        assert!(urls.iter().any(|url| url.ends_with("/vid")));
        assert!(urls.iter().any(|url| url.ends_with("/aud")));
        assert!(!urls.iter().any(|url| url.ends_with("/doc")));
        assert!(
            !urls.iter().any(|url| url.ends_with("/bare")),
            "events without response_headers fail closed"
        );
    }

    #[tokio::test]
    async fn method_filter_is_case_insensitive() {
        use crate::tools::test_support::browser_with_observations;

        let mut post = event(
            "p",
            1,
            10,
            Some(10),
            Some(100),
            "fetch",
            RequestState::Completed,
        );
        post.method = "POST".to_string();
        let get_one = event(
            "g1",
            2,
            20,
            Some(10),
            Some(100),
            "fetch",
            RequestState::Completed,
        );
        let get_two = event(
            "g2",
            3,
            30,
            Some(10),
            Some(100),
            "fetch",
            RequestState::Completed,
        );

        let observations = vec![
            ObservationEvent::NetworkRequest(Box::new(post)),
            ObservationEvent::NetworkRequest(Box::new(get_one)),
            ObservationEvent::NetworkRequest(Box::new(get_two)),
        ];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_network_activity(
            &json!({ "since": "all", "method": "post" }),
            &mut browser,
            &mut state,
        )
        .await
        .expect("list_network_activity should succeed");
        let payload = reply_payload(&effect);

        let rows = payload["requests"]
            .as_array()
            .expect("requests should be an array");
        assert_eq!(rows.len(), 1, "lowercase method input matches POST events");
        assert_eq!(rows[0]["method"].as_str(), Some("POST"));
        assert!(rows[0]["url"].as_str().unwrap().ends_with("/p"));
    }

    #[tokio::test]
    async fn size_range_filter_bounds_requests() {
        use crate::tools::test_support::browser_with_observations;

        let below = event(
            "below",
            1,
            10,
            Some(10),
            Some(50_000),
            "fetch",
            RequestState::Completed,
        );
        let inside_low = event(
            "low",
            2,
            20,
            Some(10),
            Some(200_000),
            "fetch",
            RequestState::Completed,
        );
        let inside_high = event(
            "high",
            3,
            30,
            Some(10),
            Some(300_000),
            "fetch",
            RequestState::Completed,
        );
        let above = event(
            "above",
            4,
            40,
            Some(10),
            Some(600_000),
            "fetch",
            RequestState::Completed,
        );

        let observations = vec![
            ObservationEvent::NetworkRequest(Box::new(below)),
            ObservationEvent::NetworkRequest(Box::new(inside_low)),
            ObservationEvent::NetworkRequest(Box::new(inside_high)),
            ObservationEvent::NetworkRequest(Box::new(above)),
        ];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_network_activity(
            &json!({ "since": "all", "min_size_kb": 100, "max_size_kb": 500 }),
            &mut browser,
            &mut state,
        )
        .await
        .expect("list_network_activity should succeed");
        let payload = reply_payload(&effect);

        let urls = row_urls(&payload);
        assert_eq!(urls.len(), 2, "only in-range sizes survive");
        assert!(urls.iter().any(|url| url.ends_with("/low")));
        assert!(urls.iter().any(|url| url.ends_with("/high")));
    }

    #[test]
    fn size_range_rejects_inverted_bounds() {
        let result = parse_list_input(&json!({ "min_size_kb": 500, "max_size_kb": 100 }));
        let error = result.expect_err("inverted size bounds should be rejected");
        assert!(
            error
                .to_string()
                .contains("min_size_kb cannot exceed max_size_kb"),
            "unexpected error message: {error}"
        );
    }

    #[tokio::test]
    async fn inverted_size_bounds_make_handler_error() {
        use crate::tools::test_support::browser_with_observations;

        let mut browser = browser_with_observations(vec![]);
        let mut state = CrawlState::default();

        let result = list_network_activity(
            &json!({ "min_size_kb": 500, "max_size_kb": 100 }),
            &mut browser,
            &mut state,
        )
        .await;

        assert!(result.is_err(), "handler must surface the validation error");
    }

    #[tokio::test]
    async fn unique_urls_collapses_same_url_keeping_latest_and_max_size() {
        use crate::tools::test_support::browser_with_observations;

        let mut first = event(
            "a",
            1,
            100,
            Some(10),
            Some(1000),
            "fetch",
            RequestState::Completed,
        );
        first.url = "https://example.com/same".to_string();
        let mut second = event(
            "b",
            2,
            200,
            Some(10),
            Some(3000),
            "fetch",
            RequestState::Completed,
        );
        second.url = "https://example.com/same".to_string();
        let mut third = event(
            "c",
            3,
            300,
            Some(10),
            Some(2000),
            "fetch",
            RequestState::Completed,
        );
        third.url = "https://example.com/same".to_string();

        let observations = vec![
            ObservationEvent::NetworkRequest(Box::new(first)),
            ObservationEvent::NetworkRequest(Box::new(second)),
            ObservationEvent::NetworkRequest(Box::new(third)),
        ];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_network_activity(
            &json!({ "since": "all", "unique_urls": true }),
            &mut browser,
            &mut state,
        )
        .await
        .expect("list_network_activity should succeed");
        let payload = reply_payload(&effect);

        let rows = payload["requests"]
            .as_array()
            .expect("requests should be an array");
        assert_eq!(rows.len(), 1, "same-url events collapse to one row");
        assert_eq!(rows[0]["request_count"].as_u64(), Some(3));
        // max of {1000, 3000, 2000} bytes = 3000 -> round(3000 / 1024) = 3 KB
        assert_eq!(rows[0]["size_kb"].as_u64(), Some(3), "dedup keeps max size");

        let representative = state
            .network_request_refs
            .get("@r1")
            .expect("@r1 should resolve to the representative event");
        assert_eq!(
            representative.request_id, "c",
            "representative keeps the latest event's request_id"
        );
    }

    #[tokio::test]
    async fn content_type_present_for_typed_response_and_null_otherwise() {
        use crate::tools::test_support::browser_with_observations;

        let mut typed = event(
            "typed",
            1,
            10,
            Some(10),
            Some(100),
            "fetch",
            RequestState::Completed,
        );
        typed.response_headers = Some(BTreeMap::from([(
            "Content-Type".to_string(),
            "application/json".to_string(),
        )]));
        let untyped = event(
            "untyped",
            2,
            20,
            Some(10),
            Some(100),
            "fetch",
            RequestState::Completed,
        );

        let observations = vec![
            ObservationEvent::NetworkRequest(Box::new(typed)),
            ObservationEvent::NetworkRequest(Box::new(untyped)),
        ];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_network_activity(
            &json!({ "since": "all", "sort_by": ["oldest"] }),
            &mut browser,
            &mut state,
        )
        .await
        .expect("list_network_activity should succeed");
        let payload = reply_payload(&effect);

        let rows = payload["requests"]
            .as_array()
            .expect("requests should be an array");
        let typed_row = rows
            .iter()
            .find(|row| row["url"].as_str().unwrap().ends_with("/typed"))
            .expect("typed row present");
        let untyped_row = rows
            .iter()
            .find(|row| row["url"].as_str().unwrap().ends_with("/untyped"))
            .expect("untyped row present");

        assert_eq!(
            typed_row["content_type"].as_str(),
            Some("application/json"),
            "content_type surfaces inline"
        );
        assert!(
            untyped_row.get("content_type").is_some(),
            "content_type key is always present"
        );
        assert!(
            untyped_row["content_type"].is_null(),
            "content_type is null when response_headers are absent"
        );
    }

    #[tokio::test]
    async fn defaults_omit_request_count_field() {
        use crate::tools::test_support::browser_with_observations;

        let observations = vec![ObservationEvent::NetworkRequest(Box::new(event(
            "data",
            1,
            10,
            Some(16),
            Some(128),
            "fetch",
            RequestState::Completed,
        )))];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_network_activity(&json!({ "since": "all" }), &mut browser, &mut state)
            .await
            .expect("list_network_activity should succeed");
        let payload = reply_payload(&effect);

        let rows = payload["requests"]
            .as_array()
            .expect("requests should be an array");
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].get("request_count").is_none(),
            "request_count is omitted without unique_urls"
        );
        assert_eq!(payload["summary"]["total"].as_u64(), Some(1));
    }
}
