use crate::auth::SharedAuthProvider;
use crate::common::ClaudeMessagesApiRequest;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::headers::insert_header;
use crate::sse::spawn_claude_response_stream;
use crate::telemetry::SseTelemetry;
use codex_client::HttpTransport;
use codex_client::RequestTelemetry;
use codex_client::TransportError;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use http::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::instrument;

const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct ClaudeMessagesClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct ClaudeMessagesOptions {
    pub conversation_id: Option<String>,
    pub extra_headers: HeaderMap,
}

impl<T: HttpTransport> ClaudeMessagesClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    #[instrument(
        name = "claude_messages.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "claude_messages_http",
            http.method = "POST",
            api.path = "messages"
        )
    )]
    pub async fn stream_request(
        &self,
        request: ClaudeMessagesApiRequest,
        options: ClaudeMessagesOptions,
    ) -> Result<ResponseStream, ApiError> {
        let mut headers = options.extra_headers;
        if let Some(ref conv_id) = options.conversation_id {
            insert_header(&mut headers, "x-client-request-id", conv_id);
        }
        if !headers.contains_key("anthropic-version") {
            headers.insert(
                "anthropic-version",
                HeaderValue::from_static(ANTHROPIC_VERSION),
            );
        }

        let tool_call_info = request.tool_call_info.clone();
        let body = serde_json::to_value(&request)
            .map_err(|e| ApiError::Stream(format!("failed to encode claude request: {e}")))?;
        self.stream(body, headers, tool_call_info).await
    }

    fn path() -> &'static str {
        "messages"
    }

    #[instrument(
        name = "claude_messages.stream",
        level = "info",
        skip_all,
        fields(
            transport = "claude_messages_http",
            http.method = "POST",
            api.path = "messages"
        )
    )]
    pub async fn stream(
        &self,
        body: Value,
        extra_headers: HeaderMap,
        tool_call_info: std::collections::HashMap<String, crate::common::ClaudeToolCallInfo>,
    ) -> Result<ResponseStream, ApiError> {
        let stream_response = self
            .session
            .stream_with(
                Method::POST,
                Self::path(),
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                },
            )
            .await
            .map_err(map_claude_api_error)?;

        Ok(spawn_claude_response_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            tool_call_info,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeApiErrorEnvelope {
    error: ClaudeApiErrorBody,
}

#[derive(Debug, Deserialize)]
struct ClaudeApiErrorBody {
    #[serde(default)]
    r#type: Option<String>,
    message: String,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedClaudeApiError {
    kind: Option<String>,
    message: String,
}

fn map_claude_api_error(error: ApiError) -> ApiError {
    let ApiError::Transport(TransportError::Http {
        status,
        body: Some(body),
        ..
    }) = error
    else {
        return error;
    };

    match parse_claude_api_error(&body) {
        Some(error) => {
            let message = match error.kind.as_deref() {
                Some(kind) if !kind.trim().is_empty() => format!("{kind}: {}", error.message),
                _ => error.message,
            };
            map_claude_status_error(status, error.kind.as_deref(), message)
        }
        None => map_claude_status_error(status, /*kind*/ None, body),
    }
}

fn parse_claude_api_error(body: &str) -> Option<ParsedClaudeApiError> {
    let envelope = serde_json::from_str::<ClaudeApiErrorEnvelope>(body).ok()?;
    Some(ParsedClaudeApiError {
        kind: envelope.error.r#type,
        message: envelope.error.message,
    })
}

fn map_claude_status_error(status: StatusCode, kind: Option<&str>, message: String) -> ApiError {
    if kind == Some("rate_limit_error") || status == StatusCode::TOO_MANY_REQUESTS {
        return ApiError::RateLimit(message);
    }
    if kind == Some("overloaded_error") || status == StatusCode::SERVICE_UNAVAILABLE {
        return ApiError::ServerOverloaded;
    }
    ApiError::Api { status, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_claude_json_http_error_to_api_error() {
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::BAD_REQUEST,
            url: Some("https://example.com/v1/messages".to_string()),
            headers: None,
            body: Some(
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"missing field"}}"#
                    .to_string(),
            ),
        });

        let ApiError::Api { status, message } = map_claude_api_error(error) else {
            panic!("expected Api error");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(message, "invalid_request_error: missing field");
    }

    #[test]
    fn maps_claude_rate_limit_http_error_to_rate_limit() {
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::TOO_MANY_REQUESTS,
            url: Some("https://example.com/v1/messages".to_string()),
            headers: None,
            body: Some(
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#
                    .to_string(),
            ),
        });

        assert!(matches!(
            map_claude_api_error(error),
            ApiError::RateLimit(message) if message == "rate_limit_error: slow down"
        ));
    }

    #[test]
    fn maps_claude_overloaded_error_type_to_server_overloaded() {
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::from_u16(529).expect("status code"),
            url: Some("https://example.com/v1/messages".to_string()),
            headers: None,
            body: Some(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"overloaded"}}"#
                    .to_string(),
            ),
        });

        assert!(matches!(
            map_claude_api_error(error),
            ApiError::ServerOverloaded
        ));
    }
}
