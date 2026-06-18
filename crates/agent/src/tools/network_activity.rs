use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use browser::{NetworkRequestEvent, ObservationEvent, RequestState};
use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestFilter {
    All,
    Xhr,
    Failed,
    Pending,
    Aborted,
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
        _ => {
            return Err(CrawlError::new(
                "'filter' must be one of: all, xhr, failed, pending, aborted",
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

    #[allow(clippy::cast_possible_truncation)]
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LIMIT, |value| value as usize);

    Ok(ListInput {
        since,
        until,
        filter,
        pattern,
        sort_by,
        limit,
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

fn is_internal_observation_request(url: &str) -> bool {
    url.contains("__acrawl_poll") || url.contains("poll_observations")
}

fn request_matches_filter(event: &NetworkRequestEvent, filter: RequestFilter) -> bool {
    match filter {
        RequestFilter::All => true,
        RequestFilter::Xhr => normalize_request_type(&event.request_type) == "xhr",
        RequestFilter::Failed => event.state == RequestState::Failed,
        RequestFilter::Pending => event.state == RequestState::Pending,
        RequestFilter::Aborted => event.state == RequestState::Aborted,
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

fn drain_network_events(polled: Vec<ObservationEvent>) -> Vec<NetworkRequestEvent> {
    polled
        .into_iter()
        .filter_map(|event| match event {
            ObservationEvent::NetworkRequest(network)
                if !is_internal_observation_request(&network.url) =>
            {
                Some(network)
            }
            _ => None,
        })
        .collect()
}

fn latest_requests(events: &[NetworkRequestEvent]) -> Vec<NetworkRequestEvent> {
    let mut by_request_id = HashMap::new();
    for event in events {
        by_request_id.insert(event.request_id.clone(), event.clone());
    }
    by_request_id.into_values().collect()
}

fn matching_requests(crawl_state: &CrawlState, input: &ListInput) -> Vec<NetworkRequestEvent> {
    let since = match input.since {
        SinceBound::All => 0,
        SinceBound::Last => crawl_state.seq_counter.current().saturating_sub(1),
        SinceBound::Seq(value) => value,
    };
    let until = match input.until {
        UntilBound::Now => None,
        UntilBound::Seq(value) => Some(value),
    };

    let mut requests = latest_requests(&crawl_state.network_request_events)
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
        .collect::<Vec<_>>();

    requests.sort_by(|left, right| compare_requests(left, right, &input.sort_by));
    requests
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

fn build_request_row(event: &NetworkRequestEvent, id: &str, now_ms: u64) -> Value {
    json!({
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
        "initiated_ms_ago": (event.state == RequestState::Pending)
            .then_some(now_ms.saturating_sub(event.timestamp_ms)),
    })
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

    crawl_state
        .network_request_events
        .extend(drain_network_events(polled));

    let all_matching = matching_requests(crawl_state, &input);
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
            build_request_row(event, &id, now_ms)
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

    let request_body = Value::Null;
    let response_body = Value::Null;
    let _ = include_body; // Bodies not yet captured in observation buffer

    Ok(ToolEffect::reply_json(&json!({
        "url": event.url,
        "method": event.method,
        "status": event.status,
        "state": state_name(event.state),
        "request_headers": Value::Null,
        "response_headers": Value::Null,
        "request_body": request_body,
        "response_body": response_body,
        "timing": {
            "dns_ms": Value::Null,
            "connect_ms": Value::Null,
            "tls_ms": Value::Null,
            "ttfb_ms": Value::Null,
            "download_ms": Value::Null,
            "total_duration_ms": event.duration_ms,
        },
        "initiator": {
            "type": event.initiator_type,
            "file": Value::Null,
            "line": Value::Null,
        },
        "from_service_worker": event.from_service_worker,
        "note": "Headers and bodies are not captured in the current observation buffer implementation.",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn parse_input_defaults() {
        let input = parse_list_input(&json!({})).expect("input should parse");
        assert_eq!(input.since, SinceBound::Last);
        assert_eq!(input.until, UntilBound::Now);
        assert_eq!(input.filter, RequestFilter::All);
        assert_eq!(input.sort_by, vec![SortKey::Oldest]);
        assert_eq!(input.limit, DEFAULT_LIMIT);
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

        let requests = matching_requests(&state, &input);
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

        let ids = matching_requests(&state, &input)
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
}
