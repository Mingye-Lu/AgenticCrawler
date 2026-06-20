use std::collections::HashMap;

use browser::WebSocketFrameEvent;
use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

const DEFAULT_LIMIT: usize = 30;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectionFilter {
    All,
    Sent,
    Received,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Newest,
    Oldest,
}

fn parse_since(input: &Value) -> Result<SinceBound, CrawlError> {
    match input.get("since") {
        None | Some(Value::Null) => Ok(SinceBound::Last),
        Some(Value::String(value)) if value == "all" => Ok(SinceBound::All),
        Some(Value::String(value)) if value == "last" => Ok(SinceBound::Last),
        Some(Value::Number(value)) => Ok(SinceBound::Seq(value.as_u64().ok_or_else(|| {
            CrawlError::new("'since' must be 'all', 'last', or a non-negative seq number")
        })?)),
        _ => Err(CrawlError::new(
            "'since' must be 'all', 'last', or a non-negative seq number",
        )),
    }
}

fn parse_until(input: &Value) -> Result<UntilBound, CrawlError> {
    match input.get("until") {
        None | Some(Value::Null) => Ok(UntilBound::Now),
        Some(Value::String(value)) if value == "now" => Ok(UntilBound::Now),
        Some(Value::Number(value)) => Ok(UntilBound::Seq(value.as_u64().ok_or_else(|| {
            CrawlError::new("'until' must be 'now' or a non-negative seq number")
        })?)),
        _ => Err(CrawlError::new(
            "'until' must be 'now' or a non-negative seq number",
        )),
    }
}

fn resolve_since(since: SinceBound, crawl_state: &CrawlState) -> u64 {
    match since {
        SinceBound::All => 0,
        SinceBound::Last => crawl_state.seq_counter.current().saturating_sub(1),
        SinceBound::Seq(value) => value,
    }
}

fn resolve_until(until: UntilBound) -> Option<u64> {
    match until {
        UntilBound::Now => None,
        UntilBound::Seq(value) => Some(value),
    }
}

fn filter_by_window(
    events: &[WebSocketFrameEvent],
    since: u64,
    until: Option<u64>,
) -> Vec<&WebSocketFrameEvent> {
    events
        .iter()
        .filter(|event| {
            event.seq_at_initiation >= since
                && until.is_none_or(|upper| event.seq_at_initiation < upper)
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ConnectionInfo {
    url: String,
    status: String,
    sent_count: usize,
    received_count: usize,
    total_bytes: u64,
    first_timestamp_ms: u64,
    last_timestamp_ms: u64,
    messages: Vec<WebSocketFrameEvent>,
}

pub async fn list_websocket_activity(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let since = parse_since(input)?;
    let until = parse_until(input)?;

    let polled = browser
        .acquire_bridge()
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?
        .poll_observations()
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?;

    crawl_state.ingest_observations(polled);

    let since_val = resolve_since(since, crawl_state);
    let until_val = resolve_until(until);
    let matching = filter_by_window(&crawl_state.websocket_frame_events, since_val, until_val);

    let mut connections: HashMap<String, ConnectionInfo> = HashMap::new();
    for event in &matching {
        let entry = connections
            .entry(event.connection_id.clone())
            .or_insert_with(|| ConnectionInfo {
                url: event.url.clone(),
                status: event.connection_status.clone(),
                sent_count: 0,
                received_count: 0,
                total_bytes: 0,
                first_timestamp_ms: event.timestamp_ms,
                last_timestamp_ms: event.timestamp_ms,
                messages: Vec::new(),
            });

        match event.direction.as_str() {
            "sent" => entry.sent_count += 1,
            _ => entry.received_count += 1,
        }
        entry.total_bytes = entry.total_bytes.saturating_add(event.size_bytes);
        entry.status.clone_from(&event.connection_status);
        if event.timestamp_ms < entry.first_timestamp_ms {
            entry.first_timestamp_ms = event.timestamp_ms;
        }
        if event.timestamp_ms > entry.last_timestamp_ms {
            entry.last_timestamp_ms = event.timestamp_ms;
        }
        entry.messages.push((*event).clone());
    }

    let mut conn_list: Vec<(String, ConnectionInfo)> = connections.into_iter().collect();
    conn_list.sort_by_key(|(_, info)| info.first_timestamp_ms);

    crawl_state.websocket_connection_refs.clear();
    let mut active_count = 0_usize;
    let mut closed_count = 0_usize;
    let mut total_messages = 0_usize;
    let mut total_bytes = 0_u64;

    let connection_rows: Vec<Value> = conn_list
        .into_iter()
        .enumerate()
        .map(|(index, (connection_id, info))| {
            let id = format!("@ws{}", index + 1);
            let duration_ms = info
                .last_timestamp_ms
                .saturating_sub(info.first_timestamp_ms);
            let msg_count = info.sent_count + info.received_count;

            if info.status == "open" {
                active_count += 1;
            } else {
                closed_count += 1;
            }
            total_messages += msg_count;
            total_bytes = total_bytes.saturating_add(info.total_bytes);

            crawl_state.websocket_connection_refs.insert(
                id.clone(),
                WebSocketConnectionRef {
                    connection_id,
                    messages: info.messages,
                },
            );

            json!({
                "id": id,
                "url": info.url,
                "status": info.status,
                "sent_count": info.sent_count,
                "received_count": info.received_count,
                "total_bytes": info.total_bytes,
                "duration_ms": duration_ms,
            })
        })
        .collect();

    Ok(ToolEffect::reply_json(&json!({
        "connections": connection_rows,
        "summary": {
            "active": active_count,
            "closed": closed_count,
            "total_messages": total_messages,
            "total_bytes": total_bytes,
        }
    })))
}

pub fn inspect_websocket(
    input: &Value,
    _browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolExecutionError::new("missing required field: id"))?;

    let direction = match input
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("all")
    {
        "all" => DirectionFilter::All,
        "sent" => DirectionFilter::Sent,
        "received" => DirectionFilter::Received,
        _ => {
            return Err(ToolExecutionError::new(
                "'direction' must be one of: all, sent, received",
            ));
        }
    };

    let sort_by = match input
        .get("sort_by")
        .and_then(Value::as_str)
        .unwrap_or("newest")
    {
        "newest" => SortKey::Newest,
        "oldest" => SortKey::Oldest,
        _ => {
            return Err(ToolExecutionError::new(
                "'sort_by' must be one of: newest, oldest",
            ));
        }
    };

    #[allow(clippy::cast_possible_truncation)]
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LIMIT, |value| value as usize);

    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());

    let conn_ref = crawl_state
        .websocket_connection_refs
        .get(id)
        .ok_or_else(|| ToolExecutionError::new(format!("unknown websocket id: `{id}`")))?;

    let mut filtered: Vec<&WebSocketFrameEvent> = conn_ref
        .messages
        .iter()
        .filter(|msg| match direction {
            DirectionFilter::All => true,
            DirectionFilter::Sent => msg.direction == "sent",
            DirectionFilter::Received => msg.direction == "received",
        })
        .filter(|msg| pattern.is_none_or(|pat| msg.data.contains(pat)))
        .collect();

    let total_matching = filtered.len();

    match sort_by {
        SortKey::Newest => filtered.sort_by_key(|b| std::cmp::Reverse(b.timestamp_ms)),
        SortKey::Oldest => filtered.sort_by_key(|a| a.timestamp_ms),
    }

    let truncated = filtered.len() > limit;
    let visible: Vec<&WebSocketFrameEvent> = filtered.into_iter().take(limit).collect();

    let url = conn_ref
        .messages
        .first()
        .map_or("unknown", |m| m.url.as_str());
    let status = conn_ref
        .messages
        .last()
        .map_or("unknown", |m| m.connection_status.as_str());

    let messages: Vec<Value> = visible
        .iter()
        .enumerate()
        .map(|(index, msg)| {
            json!({
                "id": format!("@m{}", index + 1),
                "direction": msg.direction,
                "data": msg.data,
                "size_bytes": msg.size_bytes,
                "timestamp_ms": msg.timestamp_ms,
            })
        })
        .collect();

    Ok(ToolEffect::reply_json(&json!({
        "url": url,
        "status": status,
        "messages": messages,
        "summary": {
            "returned": visible.len(),
            "total_matching": total_matching,
        },
        "truncated": truncated,
    })))
}

#[derive(Debug, Clone)]
pub struct WebSocketConnectionRef {
    pub connection_id: String,
    pub messages: Vec<WebSocketFrameEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser::ObservationEvent;

    #[allow(clippy::too_many_arguments)]
    fn ws_event(
        connection_id: &str,
        url: &str,
        direction: &str,
        data: &str,
        size_bytes: u64,
        timestamp_ms: u64,
        seq_at_initiation: u64,
        connection_status: &str,
    ) -> WebSocketFrameEvent {
        WebSocketFrameEvent {
            timestamp_ms,
            tab_index: 0,
            seq_at_initiation,
            connection_id: connection_id.to_string(),
            url: url.to_string(),
            direction: direction.to_string(),
            data: data.to_string(),
            size_bytes,
            connection_status: connection_status.to_string(),
        }
    }

    #[tokio::test]
    async fn list_websocket_activity_surfaces_connection_through_bridge() {
        use crate::tools::test_support::browser_with_observations;

        let observations = vec![ObservationEvent::WebSocketFrame(ws_event(
            "ws-1",
            "wss://api.test/socket",
            "received",
            "hello",
            5,
            10,
            1,
            "open",
        ))];
        let mut browser = browser_with_observations(observations);
        let mut state = CrawlState::default();

        let effect = list_websocket_activity(&json!({ "since": "all" }), &mut browser, &mut state)
            .await
            .expect("list_websocket_activity should succeed");
        let rendered = format!("{effect:?}");

        assert!(rendered.contains("wss://api.test/socket"));
        assert!(!state.websocket_connection_refs.is_empty());
    }

    #[test]
    fn parse_since_defaults_to_last() {
        assert_eq!(parse_since(&json!({})).unwrap(), SinceBound::Last);
    }

    #[test]
    fn parse_since_accepts_all() {
        assert_eq!(
            parse_since(&json!({"since": "all"})).unwrap(),
            SinceBound::All
        );
    }

    #[test]
    fn parse_since_accepts_seq_number() {
        assert_eq!(
            parse_since(&json!({"since": 5})).unwrap(),
            SinceBound::Seq(5)
        );
    }

    #[test]
    fn parse_until_defaults_to_now() {
        assert_eq!(parse_until(&json!({})).unwrap(), UntilBound::Now);
    }

    #[test]
    fn parse_until_accepts_seq_number() {
        assert_eq!(
            parse_until(&json!({"until": 10})).unwrap(),
            UntilBound::Seq(10)
        );
    }

    #[test]
    fn filter_by_window_uses_half_open_interval() {
        let events = vec![
            ws_event("c1", "wss://ex.com", "received", "a", 1, 100, 1, "open"),
            ws_event("c1", "wss://ex.com", "received", "b", 2, 200, 2, "open"),
            ws_event("c1", "wss://ex.com", "received", "c", 3, 300, 3, "open"),
        ];

        let result = filter_by_window(&events, 2, Some(3));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].data, "b");
    }

    #[test]
    fn filter_by_window_no_upper_bound() {
        let events = vec![
            ws_event("c1", "wss://ex.com", "received", "a", 1, 100, 1, "open"),
            ws_event("c1", "wss://ex.com", "received", "b", 2, 200, 5, "open"),
        ];

        let result = filter_by_window(&events, 3, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].data, "b");
    }
}
