use std::collections::VecDeque;
use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use serde_json::Value;

use crate::error::ApiError;
use crate::types::{MessageRequest, StreamEvent};

const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
const BEDROCK_SERVICE: &str = "bedrock-runtime";
const BEDROCK_ACCEPT: &str = "application/vnd.amazon.eventstream";

#[derive(Debug, Clone)]
pub struct BedrockClient {
    http: reqwest::Client,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

impl BedrockClient {
    #[must_use]
    pub fn new(access_key_id: String, secret_access_key: String, region: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            region,
            access_key_id,
            secret_access_key,
            session_token: None,
        }
    }

    #[must_use]
    pub fn with_session_token(mut self, token: String) -> Self {
        self.session_token = Some(token);
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
        let req = self.signed_request(&request.model, &body, SystemTime::now())?;
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
        streaming: bool,
    ) -> Result<String, ApiError> {
        let mut payload = serde_json::to_value(request).map_err(ApiError::from)?;
        let Some(map) = payload.as_object_mut() else {
            return Err(ApiError::Auth("bedrock request payload must be a JSON object".into()));
        };

        map.insert(
            "anthropic_version".into(),
            Value::String(BEDROCK_ANTHROPIC_VERSION.to_string()),
        );
        map.insert("stream".into(), Value::Bool(streaming));

        serde_json::to_string(&payload).map_err(ApiError::from)
    }

    fn credentials(&self) -> Credentials {
        Credentials::new(
            &self.access_key_id,
            &self.secret_access_key,
            self.session_token.clone(),
            None,
            "acrawl-bedrock",
        )
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
                    .map_err(|error| ApiError::Auth(format!("bedrock header is not valid UTF-8: {error}")))
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
            .map_err(|error| ApiError::Auth(format!("failed to build Bedrock signing params: {error}")))?;

        let signable_request = SignableRequest::new(
            request.method().as_str(),
            request.url().as_str(),
            signable_headers.iter().map(|(name, value)| (name.as_str(), value.as_str())),
            SignableBody::Bytes(body.as_bytes()),
        )
        .map_err(|error| ApiError::Auth(format!("failed to create Bedrock signable request: {error}")))?;

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
                    self.drain_frames();
                }
                None => {
                    self.done = true;
                }
            }
        }
    }

    fn drain_frames(&mut self) {
        while let Some((payload, consumed)) = parse_event_frame(&self.buffer) {
            self.buffer.drain(..consumed);
            if payload.is_empty() {
                continue;
            }
            if let Some(event) = parse_stream_event_payload(&payload) {
                self.pending.push_back(event);
            }
        }
    }
}

fn parse_stream_event_payload(payload: &[u8]) -> Option<StreamEvent> {
    serde_json::from_slice::<StreamEvent>(payload).ok()
}

fn parse_event_frame(data: &[u8]) -> Option<(Vec<u8>, usize)> {
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
    if payload_start > payload_end {
        return None;
    }

    Some((data[payload_start..payload_end].to_vec(), total_len))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn build_frame(payload: &[u8]) -> Vec<u8> {
        let total_len = 12 + payload.len() + 4;
        let mut frame = Vec::with_capacity(total_len);
        frame.extend_from_slice(
            &u32::try_from(total_len)
                .expect("frame length should fit into u32")
                .to_be_bytes(),
        );
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame.extend_from_slice(&0_u32.to_be_bytes());
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
                UNIX_EPOCH + Duration::from_secs(1_735_689_600),
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
        let payload = serde_json::to_vec(&StreamEvent::MessageStop(crate::types::MessageStopEvent {}))
            .expect("serialize stream event");
        let frame = build_frame(&payload);
        let (parsed_payload, consumed) = parse_event_frame(&frame).expect("parse frame");

        assert_eq!(consumed, frame.len());
        assert_eq!(parsed_payload, payload);
        let parsed_event = parse_stream_event_payload(&parsed_payload)
            .expect("stream event");
        assert!(matches!(parsed_event, StreamEvent::MessageStop(_)));
    }
}
