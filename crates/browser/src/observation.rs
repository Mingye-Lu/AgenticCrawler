use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

const DEFAULT_OBSERVATION_BUFFER_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationEvent {
    NetworkRequest(NetworkRequestEvent),
    ConsoleMessage(ConsoleMessageEvent),
    WebSocketFrame(WebSocketFrameEvent),
}

impl ObservationEvent {
    #[must_use]
    pub fn seq_at_initiation(&self) -> u64 {
        match self {
            Self::NetworkRequest(event) => event.seq_at_initiation,
            Self::ConsoleMessage(event) => event.seq_at_initiation,
            Self::WebSocketFrame(event) => event.seq_at_initiation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRequestEvent {
    pub timestamp_ms: u64,
    pub tab_index: usize,
    pub seq_at_initiation: u64,
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub state: RequestState,
    pub size_bytes: Option<u64>,
    pub duration_ms: Option<u64>,
    pub request_type: String,
    pub from_service_worker: bool,
    pub initiator_type: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleMessageEvent {
    pub timestamp_ms: u64,
    pub tab_index: usize,
    pub seq_at_initiation: u64,
    pub level: String,
    pub message_type: ConsoleMessageType,
    pub text: String,
    pub source_url: Option<String>,
    pub source_line: Option<u32>,
    pub source_column: Option<u32>,
    pub stack: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSocketFrameEvent {
    pub timestamp_ms: u64,
    pub tab_index: usize,
    pub seq_at_initiation: u64,
    pub connection_id: String,
    pub url: String,
    pub direction: String,
    pub data: String,
    pub size_bytes: u64,
    pub connection_status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestState {
    Pending,
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsoleMessageType {
    Console,
    Exception,
    PromiseRejection,
    CspViolation,
}

#[derive(Debug, Clone)]
pub struct ObservationBuffer {
    pub max_bytes: usize,
    pub current_bytes: usize,
    pub events: VecDeque<ObservationEvent>,
}

impl Default for ObservationBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_OBSERVATION_BUFFER_BYTES)
    }
}

impl ObservationBuffer {
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            current_bytes: 0,
            events: VecDeque::new(),
        }
    }

    pub fn push(&mut self, event: ObservationEvent) {
        let event_size = Self::byte_size_estimate(&event);
        if event_size > self.max_bytes {
            self.events.clear();
            self.current_bytes = 0;
            return;
        }

        self.current_bytes += event_size;
        self.events.push_back(event);

        while self.current_bytes > self.max_bytes {
            let Some(evicted) = self.events.pop_front() else {
                self.current_bytes = 0;
                break;
            };
            self.current_bytes = self
                .current_bytes
                .saturating_sub(Self::byte_size_estimate(&evicted));
        }
    }

    pub fn drain(&mut self) -> Vec<ObservationEvent> {
        self.current_bytes = 0;
        self.events.drain(..).collect()
    }

    #[must_use]
    pub fn events_since(&self, seq: u64) -> Vec<&ObservationEvent> {
        self.events_between(seq, None)
    }

    #[must_use]
    pub fn events_between(&self, since: u64, until: Option<u64>) -> Vec<&ObservationEvent> {
        self.events
            .iter()
            .filter(|event| {
                let event_seq = event.seq_at_initiation();
                event_seq >= since && until.is_none_or(|upper_bound| event_seq < upper_bound)
            })
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub fn byte_size_estimate(event: &ObservationEvent) -> usize {
        match event {
            ObservationEvent::NetworkRequest(network) => {
                std::mem::size_of::<NetworkRequestEvent>()
                    + network.request_id.len()
                    + network.url.len()
                    + network.method.len()
                    + network.request_type.len()
                    + network.initiator_type.as_ref().map_or(0, String::len)
                    + network.reason.as_ref().map_or(0, String::len)
            }
            ObservationEvent::ConsoleMessage(console) => {
                std::mem::size_of::<ConsoleMessageEvent>()
                    + console.level.len()
                    + console.text.len()
                    + console.source_url.as_ref().map_or(0, String::len)
                    + console.stack.as_ref().map_or(0, String::len)
            }
            ObservationEvent::WebSocketFrame(frame) => {
                std::mem::size_of::<WebSocketFrameEvent>()
                    + frame.connection_id.len()
                    + frame.url.len()
                    + frame.direction.len()
                    + frame.data.len()
                    + frame.connection_status.len()
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct SeqCounter {
    counter: AtomicU64,
}

impl SeqCounter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            counter: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub fn next(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    #[must_use]
    pub fn current(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network_event(
        seq_at_initiation: u64,
        request_id: &str,
        url_suffix: &str,
    ) -> ObservationEvent {
        ObservationEvent::NetworkRequest(NetworkRequestEvent {
            timestamp_ms: seq_at_initiation,
            tab_index: 0,
            seq_at_initiation,
            request_id: request_id.to_string(),
            url: format!("https://example.com/{url_suffix}"),
            method: "GET".to_string(),
            status: Some(200),
            state: RequestState::Completed,
            size_bytes: Some(128),
            duration_ms: Some(16),
            request_type: "fetch".to_string(),
            from_service_worker: false,
            initiator_type: Some("script".to_string()),
            reason: None,
        })
    }

    #[test]
    fn buffer_eviction_keeps_size_within_cap_and_removes_oldest_entries() {
        let event_one = network_event(1, "req-1", "one");
        let event_two = network_event(2, "req-2", "two");
        let event_three = network_event(3, "req-3", "three");
        let event_two_size = ObservationBuffer::byte_size_estimate(&event_two);
        let event_three_size = ObservationBuffer::byte_size_estimate(&event_three);
        let max_bytes = event_two_size + event_three_size;

        let mut buffer = ObservationBuffer::new(max_bytes);
        buffer.push(event_one);
        buffer.push(event_two.clone());
        buffer.push(event_three.clone());

        assert!(buffer.current_bytes <= buffer.max_bytes);
        assert_eq!(buffer.len(), 2);
        assert!(buffer
            .events
            .iter()
            .all(|event| event.seq_at_initiation() >= 2));
        assert!(buffer.events.iter().any(|event| event == &event_two));
        assert!(buffer.events.iter().any(|event| event == &event_three));
    }

    #[test]
    fn seq_counter_increments_from_one() {
        let counter = SeqCounter::new();

        let values: Vec<_> = (0..5).map(|_| counter.next()).collect();

        assert_eq!(values, vec![1, 2, 3, 4, 5]);
        assert_eq!(counter.current(), 5);
    }

    #[test]
    fn events_between_uses_half_open_interval() {
        let event_one = network_event(1, "req-1", "one");
        let event_two = network_event(2, "req-2", "two");
        let event_three = network_event(3, "req-3", "three");
        let mut buffer = ObservationBuffer::default();
        buffer.push(event_one.clone());
        buffer.push(event_two.clone());
        buffer.push(event_three.clone());

        let between = buffer.events_between(1, Some(3));
        let since = buffer.events_between(1, None);

        assert_eq!(between, vec![&event_one, &event_two]);
        assert_eq!(since, vec![&event_one, &event_two, &event_three]);
    }

    #[test]
    fn drain_empties_buffer() {
        let mut buffer = ObservationBuffer::default();
        buffer.push(network_event(1, "req-1", "one"));
        buffer.push(network_event(2, "req-2", "two"));

        let drained = buffer.drain();

        assert_eq!(drained.len(), 2);
        assert!(buffer.is_empty());
        assert_eq!(buffer.current_bytes, 0);
    }
}
