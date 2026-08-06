use std::collections::VecDeque;
use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use base64::Engine;
use serde_json::Value;

use crate::client::default_http_client;
use crate::error::ApiError;
use crate::types::{MessageRequest, StreamEvent};

const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
const BEDROCK_SERVICE: &str = "bedrock-runtime";
const BEDROCK_ACCEPT: &str = "application/vnd.amazon.eventstream";

#[derive(Debug, Clone)]
enum BedrockAuth {
    SigV4 {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
    BearerToken(String),
}

#[derive(Debug, Clone)]
pub struct BedrockClient {
    http: reqwest::Client,
    region: String,
    auth: BedrockAuth,
}

impl BedrockClient {
    #[must_use]
    pub fn new(access_key_id: String, secret_access_key: String, region: String) -> Self {
        Self {
            http: default_http_client(),
            region,
            auth: BedrockAuth::SigV4 {
                access_key_id,
                secret_access_key,
                session_token: None,
            },
        }
    }

    #[must_use]
    pub fn from_bearer_token(token: String, region: String) -> Self {
        Self {
            http: default_http_client(),
            region,
            auth: BedrockAuth::BearerToken(token),
        }
    }

    #[must_use]
    pub fn with_session_token(mut self, token: String) -> Self {
        if let BedrockAuth::SigV4 {
            ref mut session_token,
            ..
        } = self.auth
        {
            *session_token = Some(token);
        }
        self
    }

    #[must_use]
    pub fn endpoint_url(&self, model: &str) -> String {
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{model}/invoke-with-response-stream",
            self.region
        )
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<BedrockMessageStream, ApiError> {
        let body = Self::serialize_request_body(request, true)?;
        let req = match &self.auth {
            BedrockAuth::SigV4 { .. } => {
                self.signed_request(&request.model, &body, SystemTime::now())?
            }
            BedrockAuth::BearerToken(token) => self.bearer_request(&request.model, &body, token)?,
        };
        let response = self.http.execute(req).await.map_err(ApiError::from)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Api {
                status,
                error_type: None,
                message: None,
                body,
                retryable: matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504),
            });
        }

        Ok(BedrockMessageStream {
            response,
            buffer: Vec::new(),
            pending: VecDeque::new(),
            done: false,
        })
    }

    fn serialize_request_body(
        request: &MessageRequest,
        _streaming: bool,
    ) -> Result<String, ApiError> {
        let mut payload = serde_json::to_value(request).map_err(ApiError::from)?;
        let Some(map) = payload.as_object_mut() else {
            return Err(ApiError::Auth(
                "bedrock request payload must be a JSON object".into(),
            ));
        };

        map.remove("model");
        map.remove("stream");
        map.remove("reasoning_effort");
        map.insert(
            "anthropic_version".into(),
            Value::String(BEDROCK_ANTHROPIC_VERSION.to_string()),
        );

        serde_json::to_string(&payload).map_err(ApiError::from)
    }

    fn credentials(&self) -> Credentials {
        match &self.auth {
            BedrockAuth::SigV4 {
                access_key_id,
                secret_access_key,
                session_token,
            } => Credentials::new(
                access_key_id,
                secret_access_key,
                session_token.clone(),
                None,
                "acrawl-bedrock",
            ),
            BedrockAuth::BearerToken(_) => {
                Credentials::new("", "", None, None, "acrawl-bedrock-bearer")
            }
        }
    }

    fn bearer_request(
        &self,
        model: &str,
        body: &str,
        token: &str,
    ) -> Result<reqwest::Request, ApiError> {
        let endpoint = self.endpoint_url(model);
        self.http
            .post(&endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .body(body.to_string())
            .build()
            .map_err(ApiError::from)
    }

    fn signed_request(
        &self,
        model: &str,
        body: &str,
        time: SystemTime,
    ) -> Result<reqwest::Request, ApiError> {
        let endpoint = self.endpoint_url(model);
        let mut request = self
            .http
            .post(&endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, BEDROCK_ACCEPT)
            .body(body.to_string())
            .build()
            .map_err(ApiError::from)?;

        let signable_headers = request
            .headers()
            .iter()
            .map(|(name, value)| {
                value
                    .to_str()
                    .map(|value| (name.as_str().to_string(), value.to_string()))
                    .map_err(|error| {
                        ApiError::Auth(format!("bedrock header is not valid UTF-8: {error}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let identity = self.credentials().into();
        let signing_params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name(BEDROCK_SERVICE)
            .time(time)
            .settings(SigningSettings::default())
            .build()
            .map_err(|error| {
                ApiError::Auth(format!("failed to build Bedrock signing params: {error}"))
            })?;

        let signable_request = SignableRequest::new(
            request.method().as_str(),
            request.url().as_str(),
            signable_headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            SignableBody::Bytes(body.as_bytes()),
        )
        .map_err(|error| {
            ApiError::Auth(format!(
                "failed to create Bedrock signable request: {error}"
            ))
        })?;

        let (instructions, _signature) = sign(signable_request, &signing_params.into())
            .map_err(|error| ApiError::Auth(format!("failed to sign Bedrock request: {error}")))?
            .into_parts();

        for (name, value) in instructions.headers() {
            let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| ApiError::Auth(format!("invalid signed header name: {error}")))?;
            let header_value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|error| ApiError::Auth(format!("invalid signed header value: {error}")))?;
            request.headers_mut().insert(header_name, header_value);
        }

        Ok(request)
    }
}

#[derive(Debug)]
pub struct BedrockMessageStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    pending: VecDeque<StreamEvent>,
    done: bool,
}

impl BedrockMessageStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            if self.done {
                return Ok(None);
            }

            match self.response.chunk().await? {
                Some(chunk) => {
                    self.buffer.extend_from_slice(&chunk);
                    self.drain_frames()?;
                }
                None => {
                    self.done = true;
                }
            }
        }
    }

    fn drain_frames(&mut self) -> Result<(), ApiError> {
        while let Some((headers, payload, consumed)) = parse_event_frame(&self.buffer) {
            self.buffer.drain(..consumed);
            if let Some(error) = event_frame_error(&headers, &payload) {
                return Err(error);
            }
            if payload.is_empty() {
                continue;
            }
            if let Some(event) = parse_stream_event_payload(&payload) {
                self.pending.push_back(event);
            }
        }
        Ok(())
    }
}

fn parse_stream_event_payload(payload: &[u8]) -> Option<StreamEvent> {
    serde_json::from_slice::<StreamEvent>(payload)
        .ok()
        .or_else(|| {
            let wrapper: serde_json::Value = serde_json::from_slice(payload).ok()?;
            let b64 = wrapper.get("bytes")?.as_str()?;
            let decoded = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
            serde_json::from_slice::<StreamEvent>(&decoded).ok()
        })
}

/// A single `vnd.amazon.eventstream` header (name, value-as-string).
///
/// AWS's event-stream framing signals mid-stream errors (throttling,
/// validation, internal errors, etc.) via `:message-type` /
/// `:exception-type` / `:error-code` headers rather than the JSON payload,
/// so these must be parsed rather than skipped over.
type EventStreamHeader = (String, String);

/// Parses a `vnd.amazon.eventstream` frame: 4-byte total length, 4-byte
/// headers length, 4-byte prelude CRC, headers block, payload, 4-byte
/// message CRC. Returns `(headers, payload, total_bytes_consumed)`.
fn parse_event_frame(data: &[u8]) -> Option<(Vec<EventStreamHeader>, Vec<u8>, usize)> {
    if data.len() < 12 {
        return None;
    }

    let total_len = u32::from_be_bytes(data[0..4].try_into().ok()?) as usize;
    if total_len < 16 || data.len() < total_len {
        return None;
    }

    let headers_len = u32::from_be_bytes(data[4..8].try_into().ok()?) as usize;
    let payload_start = 12 + headers_len;
    let payload_end = total_len.checked_sub(4)?;
    if payload_start > payload_end || payload_start > data.len() {
        return None;
    }

    let headers = parse_event_stream_headers(&data[12..payload_start]);

    Some((
        headers,
        data[payload_start..payload_end].to_vec(),
        total_len,
    ))
}

/// Parses the header block of a `vnd.amazon.eventstream` frame into
/// `(name, value)` pairs. Each header is: 1-byte name length, name bytes,
/// 1-byte value-type, then a type-dependent value. Unrecognized/malformed
/// entries stop parsing (returning whatever headers were parsed so far)
/// rather than panicking or misreading subsequent bytes.
fn parse_event_stream_headers(mut data: &[u8]) -> Vec<EventStreamHeader> {
    let mut headers = Vec::new();

    while !data.is_empty() {
        let Some((&name_len, rest)) = data.split_first() else {
            break;
        };
        let name_len = name_len as usize;
        if rest.len() < name_len + 1 {
            break;
        }
        let name = String::from_utf8_lossy(&rest[..name_len]).into_owned();
        let value_type = rest[name_len];
        let mut rest = &rest[name_len + 1..];

        let value = match value_type {
            0 => "true".to_string(),
            1 => "false".to_string(),
            2 if !rest.is_empty() => {
                let value = rest[0].to_string();
                rest = &rest[1..];
                value
            }
            3 if rest.len() >= 2 => {
                let value = i16::from_be_bytes([rest[0], rest[1]]).to_string();
                rest = &rest[2..];
                value
            }
            4 if rest.len() >= 4 => {
                let value = i32::from_be_bytes(rest[0..4].try_into().unwrap_or_default());
                rest = &rest[4..];
                value.to_string()
            }
            5 if rest.len() >= 8 => {
                let value = i64::from_be_bytes(rest[0..8].try_into().unwrap_or_default());
                rest = &rest[8..];
                value.to_string()
            }
            6 | 7 if rest.len() >= 2 => {
                let len = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                rest = &rest[2..];
                if rest.len() < len {
                    break;
                }
                let value = if value_type == 7 {
                    String::from_utf8_lossy(&rest[..len]).into_owned()
                } else {
                    base64::engine::general_purpose::STANDARD.encode(&rest[..len])
                };
                rest = &rest[len..];
                value
            }
            8 if rest.len() >= 8 => {
                rest = &rest[8..];
                String::new()
            }
            9 if rest.len() >= 16 => {
                rest = &rest[16..];
                String::new()
            }
            _ => break,
        };

        headers.push((name, value));
        data = rest;
    }

    headers
}

/// If a frame's headers mark it as a Bedrock exception/error frame (per
/// `vnd.amazon.eventstream`'s `:message-type` header), turns it into an
/// `ApiError` instead of letting it be silently dropped.
fn event_frame_error(headers: &[EventStreamHeader], payload: &[u8]) -> Option<ApiError> {
    let message_type = headers
        .iter()
        .find(|(name, _)| name == ":message-type")
        .map(|(_, value)| value.as_str());
    if !matches!(message_type, Some("exception" | "error")) {
        return None;
    }

    let exception_type = headers
        .iter()
        .find(|(name, _)| name == ":exception-type" || name == ":error-code")
        .map(|(_, value)| value.clone());

    let payload_json: Option<Value> = serde_json::from_slice(payload).ok();
    let message = payload_json
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            headers
                .iter()
                .find(|(name, _)| name == ":error-message")
                .map(|(_, value)| value.clone())
        })
        .or_else(|| {
            let text = String::from_utf8_lossy(payload).into_owned();
            (!text.is_empty()).then_some(text)
        });

    let retryable = matches!(
        exception_type.as_deref(),
        Some(
            "throttlingException"
                | "modelTimeoutException"
                | "serviceUnavailableException"
                | "internalServerException"
                | "modelStreamErrorException"
        )
    );

    Some(ApiError::Api {
        status: bedrock_exception_status(exception_type.as_deref()),
        error_type: exception_type,
        message,
        body: String::from_utf8_lossy(payload).into_owned(),
        retryable,
    })
}

/// Maps a Bedrock event-stream `:exception-type`/`:error-code` value to the
/// HTTP status code that best represents it.
fn bedrock_exception_status(exception_type: Option<&str>) -> reqwest::StatusCode {
    match exception_type {
        Some("validationException") => reqwest::StatusCode::BAD_REQUEST,
        Some("throttlingException") => reqwest::StatusCode::TOO_MANY_REQUESTS,
        Some("modelTimeoutException") => reqwest::StatusCode::GATEWAY_TIMEOUT,
        Some("serviceUnavailableException") => reqwest::StatusCode::SERVICE_UNAVAILABLE,
        _ => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn build_frame(payload: &[u8]) -> Vec<u8> {
        build_frame_with_headers(&[], payload)
    }

    /// Builds a `vnd.amazon.eventstream` frame with the given string-typed
    /// headers (name, value) encoded ahead of the payload.
    fn build_frame_with_headers(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
        let mut header_bytes = Vec::new();
        for (name, value) in headers {
            header_bytes.push(u8::try_from(name.len()).expect("header name fits in u8"));
            header_bytes.extend_from_slice(name.as_bytes());
            header_bytes.push(7); // value type: string
            header_bytes.extend_from_slice(
                &u16::try_from(value.len())
                    .expect("header value fits in u16")
                    .to_be_bytes(),
            );
            header_bytes.extend_from_slice(value.as_bytes());
        }

        let total_len = 12 + header_bytes.len() + payload.len() + 4;
        let mut frame = Vec::with_capacity(total_len);
        frame.extend_from_slice(
            &u32::try_from(total_len)
                .expect("frame length should fit into u32")
                .to_be_bytes(),
        );
        frame.extend_from_slice(
            &u32::try_from(header_bytes.len())
                .expect("headers length should fit into u32")
                .to_be_bytes(),
        );
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame.extend_from_slice(&header_bytes);
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame
    }

    #[test]
    fn test_bedrock_request_url_format() {
        let client = BedrockClient::new("key".into(), "secret".into(), "us-east-1".into());
        let url = client.endpoint_url("anthropic.claude-sonnet-4-6-20250514-v1:0");
        assert!(url.contains("bedrock-runtime.us-east-1.amazonaws.com"));
        assert!(url.contains("invoke-with-response-stream"));
    }

    #[test]
    fn test_bedrock_region_in_endpoint() {
        let client = BedrockClient::new("key".into(), "secret".into(), "eu-west-1".into());
        let url = client.endpoint_url("some-model");
        assert!(url.contains("eu-west-1"));
    }

    #[test]
    fn test_bedrock_sigv4_signing() {
        let client = BedrockClient::new(
            "AKIDEXAMPLE".into(),
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            "us-east-1".into(),
        );
        let request = client
            .signed_request(
                "anthropic.claude-sonnet-4-6-20250514-v1:0",
                r#"{"messages":[],"max_tokens":1}"#,
                UNIX_EPOCH + Duration::from_hours(482_136),
            )
            .expect("sign request");

        let authorization = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .expect("authorization header");

        assert!(authorization.starts_with("AWS4-HMAC-SHA256"));
        assert!(authorization.contains("/us-east-1/bedrock-runtime/aws4_request"));
        assert!(request.headers().contains_key("x-amz-date"));
    }

    #[test]
    fn test_bedrock_event_stream_parse() {
        let payload =
            serde_json::to_vec(&StreamEvent::MessageStop(crate::types::MessageStopEvent {}))
                .expect("serialize stream event");
        let frame = build_frame(&payload);
        let (headers, parsed_payload, consumed) = parse_event_frame(&frame).expect("parse frame");

        assert_eq!(consumed, frame.len());
        assert!(headers.is_empty());
        assert_eq!(parsed_payload, payload);
        let parsed_event = parse_stream_event_payload(&parsed_payload).expect("stream event");
        assert!(matches!(parsed_event, StreamEvent::MessageStop(_)));
    }

    #[test]
    fn test_parse_event_stream_headers_string_values() {
        let frame = build_frame_with_headers(
            &[
                (":message-type", "exception"),
                (":exception-type", "throttlingException"),
            ],
            b"{}",
        );
        let (headers, _payload, _consumed) = parse_event_frame(&frame).expect("parse frame");

        assert_eq!(
            headers,
            vec![
                (":message-type".to_string(), "exception".to_string()),
                (
                    ":exception-type".to_string(),
                    "throttlingException".to_string()
                ),
            ]
        );
    }

    #[test]
    fn test_event_frame_error_none_for_normal_frame() {
        let (headers, payload, _consumed) = parse_event_frame(&build_frame_with_headers(
            &[(":event-type", "chunk")],
            b"{}",
        ))
        .expect("parse frame");
        assert!(event_frame_error(&headers, &payload).is_none());
    }

    #[test]
    fn test_event_frame_error_surfaces_exception_frame() {
        let payload = br#"{"message":"Too many requests, please wait and try again."}"#;
        let frame = build_frame_with_headers(
            &[
                (":message-type", "exception"),
                (":exception-type", "throttlingException"),
            ],
            payload,
        );
        let (headers, payload, _consumed) = parse_event_frame(&frame).expect("parse frame");

        let error = event_frame_error(&headers, &payload).expect("exception frame is an error");
        match error {
            ApiError::Api {
                status,
                error_type,
                message,
                retryable,
                ..
            } => {
                assert_eq!(status, reqwest::StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(error_type.as_deref(), Some("throttlingException"));
                assert_eq!(
                    message.as_deref(),
                    Some("Too many requests, please wait and try again.")
                );
                assert!(retryable, "throttlingException should be retryable");
            }
            other => panic!("expected ApiError::Api, got {other:?}"),
        }
    }

    #[test]
    fn test_event_frame_error_surfaces_validation_exception_as_non_retryable() {
        let payload = br#"{"message":"malformed input request"}"#;
        let frame = build_frame_with_headers(
            &[
                (":message-type", "exception"),
                (":exception-type", "validationException"),
            ],
            payload,
        );
        let (headers, payload, _consumed) = parse_event_frame(&frame).expect("parse frame");

        let error = event_frame_error(&headers, &payload).expect("exception frame is an error");
        match error {
            ApiError::Api {
                status, retryable, ..
            } => {
                assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
                assert!(!retryable, "validationException should not be retryable");
            }
            other => panic!("expected ApiError::Api, got {other:?}"),
        }
    }

    #[test]
    fn test_bedrock_exception_status_mapping() {
        assert_eq!(
            bedrock_exception_status(Some("validationException")),
            reqwest::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            bedrock_exception_status(Some("throttlingException")),
            reqwest::StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            bedrock_exception_status(Some("modelTimeoutException")),
            reqwest::StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(
            bedrock_exception_status(Some("serviceUnavailableException")),
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            bedrock_exception_status(Some("internalServerException")),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            bedrock_exception_status(Some("someOtherException")),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            bedrock_exception_status(None),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
