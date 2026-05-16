use crate::error::ApiError;
use crate::types::StreamEvent;

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ApiError> {
        let mut events = Vec::new();

        for frame in self.push_frames(chunk)? {
            if let Some(event) = parse_frame(&frame)? {
                events.push(event);
            }
        }

        Ok(events)
    }

    pub fn push_frames(&mut self, chunk: &[u8]) -> Result<Vec<String>, ApiError> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();

        while let Some(frame) = self.next_frame()? {
            frames.push(frame);
        }

        Ok(frames)
    }

    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, ApiError> {
        self.finish_frames()?
            .into_iter()
            .map(|frame| parse_frame(&frame))
            .collect::<Result<Vec<_>, _>>()
            .map(|events| events.into_iter().flatten().collect())
    }

    pub fn finish_frames(&mut self) -> Result<Vec<String>, ApiError> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }

        let trailing = std::mem::take(&mut self.buffer);
        // Reject invalid UTF-8 instead of silently substituting U+FFFD so
        // malformed transport bytes can't quietly corrupt the JSON payload
        // passed downstream (where deserialisation would then succeed-but-wrong).
        let frame = String::from_utf8(trailing)
            .map_err(|_| ApiError::InvalidSseFrame("trailing buffer contained invalid utf-8"))?;
        Ok(vec![frame])
    }

    fn next_frame(&mut self) -> Result<Option<String>, ApiError> {
        let Some((position, separator_len)) = self
            .buffer
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|position| (position, 2))
            .or_else(|| {
                self.buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| (position, 4))
            })
        else {
            return Ok(None);
        };

        let frame = self
            .buffer
            .drain(..position + separator_len)
            .collect::<Vec<_>>();
        let frame_len = frame.len().saturating_sub(separator_len);
        let text = String::from_utf8(frame[..frame_len].to_vec())
            .map_err(|_| ApiError::InvalidSseFrame("frame contained invalid utf-8"))?;
        Ok(Some(text))
    }
}

pub fn parse_frame(frame: &str) -> Result<Option<StreamEvent>, ApiError> {
    let trimmed = frame.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut data_lines = Vec::new();
    let mut event_name: Option<&str> = None;

    for line in trimmed.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(name) = line.strip_prefix("event:") {
            event_name = Some(name.trim());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start());
        }
    }

    if matches!(event_name, Some("ping")) {
        return Ok(None);
    }

    if data_lines.is_empty() {
        return Ok(None);
    }

    let payload = data_lines.join("\n");
    if payload == "[DONE]" {
        return Ok(None);
    }

    serde_json::from_str::<StreamEvent>(&payload)
        .map(Some)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::{parse_frame, SseParser};
    use crate::types::{ContentBlockDelta, MessageDelta, OutputContentBlock, StreamEvent, Usage};

    #[test]
    fn parses_single_frame() {
        let frame = concat!(
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Hi\"}}\n\n"
        );

        let event = parse_frame(frame).expect("frame should parse");
        assert_eq!(
            event,
            Some(StreamEvent::ContentBlockStart(
                crate::types::ContentBlockStartEvent {
                    index: 0,
                    content_block: OutputContentBlock::Text {
                        text: "Hi".to_string(),
                    },
                },
            ))
        );
    }

    #[test]
    fn parses_chunked_stream() {
        let mut parser = SseParser::new();
        let first = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel";
        let second = b"lo\"}}\n\n";

        assert!(parser
            .push(first)
            .expect("first chunk should buffer")
            .is_empty());
        let events = parser.push(second).expect("second chunk should parse");

        assert_eq!(
            events,
            vec![StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta {
                        text: "Hello".to_string(),
                    },
                }
            )]
        );
    }

    #[test]
    fn ignores_ping_and_done() {
        let mut parser = SseParser::new();
        let payload = concat!(
            ": keepalive\n",
            "event: ping\n",
            "data: {\"type\":\"ping\"}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
            "data: [DONE]\n\n"
        );

        let events = parser
            .push(payload.as_bytes())
            .expect("parser should succeed");
        assert_eq!(
            events,
            vec![
                StreamEvent::MessageDelta(crate::types::MessageDeltaEvent {
                    delta: MessageDelta {
                        stop_reason: Some("tool_use".to_string()),
                        stop_sequence: None,
                    },
                    usage: Usage {
                        input_tokens: 1,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        output_tokens: 2,
                    },
                }),
                StreamEvent::MessageStop(crate::types::MessageStopEvent {}),
            ]
        );
    }

    #[test]
    fn ignores_data_less_event_frames() {
        let frame = "event: ping\n\n";
        let event = parse_frame(frame).expect("frame without data should be ignored");
        assert_eq!(event, None);
    }

    #[test]
    fn invalid_utf8_in_frame_errors_instead_of_silently_lossy() {
        let mut parser = SseParser::new();
        // Lone continuation byte 0x80 mid-frame: invalid UTF-8.
        let chunk = b"event: x\n\xFFdata: {}\n\n";

        let err = parser
            .push(chunk)
            .expect_err("invalid utf-8 must produce an error");
        assert!(
            matches!(err, crate::error::ApiError::InvalidSseFrame(_)),
            "expected InvalidSseFrame, got {err:?}"
        );
    }

    #[test]
    fn invalid_utf8_in_trailing_buffer_errors() {
        let mut parser = SseParser::new();
        // No frame separator → bytes land in the trailing buffer; that
        // trailing flush must also reject invalid UTF-8.
        assert!(parser.push(b"event: x\ndata: \xFF").unwrap().is_empty());
        let err = parser
            .finish()
            .expect_err("trailing invalid utf-8 must error");
        assert!(matches!(err, crate::error::ApiError::InvalidSseFrame(_)));
    }

    #[test]
    fn parses_split_json_across_data_lines() {
        let frame = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\n",
            "data: \"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n"
        );

        let event = parse_frame(frame).expect("frame should parse");
        assert_eq!(
            event,
            Some(StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta {
                        text: "Hello".to_string(),
                    },
                }
            ))
        );
    }
}
