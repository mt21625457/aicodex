use crate::rate_limits::RateLimitError;
use codex_client::TransportError;
use codex_http_client::RetryAfter;
use codex_protocol::error::ResponseIncompleteError;
use codex_protocol::protocol::MisalignmentErrorDetails;
use http::StatusCode;
use serde_json::Value;
use std::fmt;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderMediaErrorKind {
    RequestTooLarge,
    ImageTooLarge,
    ImageDimensionsTooLarge,
    InvalidImage,
    DocumentTooLarge,
    InvalidDocument,
    PasswordProtectedDocument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderStreamErrorKind {
    IdleTimeout,
    ClosedBeforeMessageStart,
    ClosedAfterMessageStartBeforeStop,
    ProviderError,
    ParseError,
    TransportError,
}

impl ProviderStreamErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::IdleTimeout => "idle_timeout",
            Self::ClosedBeforeMessageStart => "closed_before_message_start",
            Self::ClosedAfterMessageStartBeforeStop => "closed_after_message_start_before_stop",
            Self::ProviderError => "provider_error",
            Self::ParseError => "parse_error",
            Self::TransportError => "transport_error",
        }
    }
}

impl fmt::Display for ProviderStreamErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ProviderMediaErrorKind {
    pub(crate) fn classify(status: Option<StatusCode>, message: &str) -> Option<Self> {
        let lower = message.to_ascii_lowercase();
        if status == Some(StatusCode::PAYLOAD_TOO_LARGE) {
            return Some(Self::RequestTooLarge);
        }
        if matches!(
            status,
            Some(StatusCode::BAD_REQUEST | StatusCode::INTERNAL_SERVER_ERROR) | None
        ) && lower.contains("could not process image")
        {
            return Some(Self::InvalidImage);
        }
        if !matches!(status, Some(StatusCode::BAD_REQUEST) | None) {
            return None;
        }
        if lower.contains("image exceeds") && lower.contains("maximum") {
            return Some(Self::ImageTooLarge);
        }
        if lower.contains("image dimensions exceed") && lower.contains("many-image") {
            return Some(Self::ImageDimensionsTooLarge);
        }
        if lower.contains("valid image")
            || lower.contains("invalid image")
            || lower.contains("image cannot be empty")
        {
            return Some(Self::InvalidImage);
        }
        if lower.contains("maximum of") && lower.contains("pdf pages") {
            return Some(Self::DocumentTooLarge);
        }
        if lower.contains("pdf specified is password protected") {
            return Some(Self::PasswordProtectedDocument);
        }
        if lower.contains("pdf specified was not valid") || lower.contains("invalid document") {
            return Some(Self::InvalidDocument);
        }
        None
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::RequestTooLarge => "request_too_large",
            Self::ImageTooLarge => "image_too_large",
            Self::ImageDimensionsTooLarge => "image_dimensions_too_large",
            Self::InvalidImage => "invalid_image",
            Self::DocumentTooLarge => "document_too_large",
            Self::InvalidDocument => "invalid_document",
            Self::PasswordProtectedDocument => "password_protected_document",
        }
    }
}

impl fmt::Display for ProviderMediaErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("api error {status}: {message}")]
    Api { status: StatusCode, message: String },
    #[error("stream error: {0}")]
    Stream(String),
    #[error("malformed provider response: {message}")]
    MalformedResponse { message: String },
    #[error("provider stream error ({kind}): {message}")]
    StreamFailure {
        kind: ProviderStreamErrorKind,
        message: String,
    },
    #[error("provider stream idle timeout: {message}")]
    StreamIdleTimeout { message: String },
    #[error("provider media error ({kind}): {message}")]
    ProviderMedia {
        kind: ProviderMediaErrorKind,
        message: String,
    },
    #[error("stream error: Incomplete response returned, reason: content_filter")]
    ContentFilter,
    #[error(transparent)]
    ResponseIncomplete(Box<ResponseIncompleteError>),
    #[error("context window exceeded")]
    ContextWindowExceeded,
    #[error("quota exceeded")]
    QuotaExceeded,
    #[error("usage not included")]
    UsageNotIncluded,
    #[error("session budget exceeded")]
    SessionBudgetExceeded,
    #[error("retryable error: {message}")]
    Retryable {
        message: String,
        retry_after: Option<RetryAfter>,
    },
    #[error("rate limit exceeded: {message}")]
    RateLimitExceeded {
        message: String,
        retry_after: Option<RetryAfter>,
    },
    #[error("rate limit: {0}")]
    RateLimit(String),
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("{message}")]
    AccessProgramNotEnabled { message: String },
    #[error("{message}")]
    InvalidAudioInput {
        message: String,
        /// True when the provider rejected an individual clip, independent of request size.
        per_recording: bool,
    },
    #[error("{message}")]
    ModelNotFound {
        message: String,
        status: Option<StatusCode>,
    },
    #[error("invalid prompt: {message}")]
    InvalidPrompt { message: String },
    #[error("{message}")]
    InferenceHookDenied { message: String },
    #[error("{message}")]
    ContentPolicyViolation {
        message: String,
        status: Option<StatusCode>,
    },
    #[error("cyber policy: {message}")]
    CyberPolicy { message: String },
    #[error("bio policy: {message}")]
    BioPolicy { message: String },
    #[error("misalignment policy violation: {message}")]
    MisalignmentPolicyViolation {
        message: String,
        misalignment: Option<MisalignmentErrorDetails>,
    },
    #[error("Flex capacity unavailable.")]
    FlexUnavailable,
    /// Server-confirmed capacity failure on a policy-restricted route.
    #[error("policy-restricted capacity unavailable")]
    PolicyCapacityUnavailable { retry_after: Option<RetryAfter> },
    #[error("server overloaded")]
    ServerOverloaded { retry_after: Option<RetryAfter> },
}

impl From<RateLimitError> for ApiError {
    fn from(err: RateLimitError) -> Self {
        Self::RateLimit(err.to_string())
    }
}

pub(crate) fn parse_terminal_provider_error(
    error: Option<&Value>,
    status: Option<StatusCode>,
) -> Option<ApiError> {
    let error = error?;
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty());
    // 网关已建立 SSE 后也可能拒绝请求，code=null、type=invalid_request_error
    // 是明确的参数错误，不是断网。仅在没有更具体错误码时使用 type，保留专用分类。
    // HTTP 非 2xx 继续由原边界保留完整错误体；这里只补齐流内错误的终态分类。
    if status.is_none()
        && (matches!(code, Some("invalid_request_error" | "invalid_request"))
            || (code.is_none() && error["type"] == "invalid_request_error"))
    {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or("Invalid request.")
            .to_owned();
        // 保留既有媒体错误专用分类；通用参数错误不能覆盖图片修复等上层行为。
        return Some(match ProviderMediaErrorKind::classify(None, &message) {
            Some(kind) => ApiError::ProviderMedia { kind, message },
            None => ApiError::InvalidRequest { message },
        });
    }
    let code = code?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty());
    match (code, status) {
        ("inference_hook_denied", None | Some(StatusCode::FORBIDDEN)) => {
            Some(ApiError::InferenceHookDenied {
                message: message
                    .unwrap_or("Your workspace inference hook blocked this request.")
                    .to_owned(),
            })
        }
        ("context_length_exceeded", None | Some(StatusCode::BAD_REQUEST)) => {
            Some(ApiError::ContextWindowExceeded)
        }
        // A non-streaming 400 already follows the ordinary invalid-request path.
        ("invalid_access_program" | "unsupported_access_program", None) => {
            Some(ApiError::InvalidRequest {
                message: message
                    .unwrap_or("The requested access program cannot be used with this model.")
                    .to_owned(),
            })
        }
        ("content_policy_violation", None | Some(StatusCode::BAD_REQUEST)) => {
            Some(ApiError::ContentPolicyViolation {
                message: message
                    .unwrap_or("The input was rejected by the content policy.")
                    .to_owned(),
                status,
            })
        }
        // HTTP 403s retain their request ID and headers in the API bridge.
        ("access_program_not_enabled", None) => Some(ApiError::AccessProgramNotEnabled {
            message: message
                .unwrap_or("The requested access program is not enabled.")
                .to_owned(),
        }),
        ("budget_reached", None | Some(StatusCode::BAD_REQUEST)) => {
            Some(ApiError::SessionBudgetExceeded)
        }
        (
            "audio_too_large"
            | "audio_too_short"
            | "invalid_audio"
            | "invalid_audio_data"
            | "invalid_audio_format"
            | "missing_audio_sample_rate",
            None | Some(StatusCode::BAD_REQUEST),
        ) => Some(ApiError::InvalidAudioInput {
            per_recording: code != "audio_too_large",
            message: message.unwrap_or("Invalid audio input.").to_owned(),
        }),
        ("flex_unavailable", None | Some(StatusCode::TOO_MANY_REQUESTS)) => {
            Some(ApiError::FlexUnavailable)
        }
        (
            "model_not_found",
            None | Some(StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        ) => Some(ApiError::ModelNotFound {
            message: message.unwrap_or(code).to_owned(),
            status,
        }),
        _ => None,
    }
}

impl From<crate::AuthError> for ApiError {
    fn from(error: crate::AuthError) -> Self {
        Self::Transport(error.into())
    }
}
