use crate::rate_limits::RateLimitError;
use codex_client::TransportError;
use http::StatusCode;
use std::fmt;
use std::time::Duration;
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
    #[error("provider stream error ({kind}): {message}")]
    StreamFailure {
        kind: ProviderStreamErrorKind,
        message: String,
    },
    #[error("provider media error ({kind}): {message}")]
    ProviderMedia {
        kind: ProviderMediaErrorKind,
        message: String,
    },
    #[error("context window exceeded")]
    ContextWindowExceeded,
    #[error("quota exceeded")]
    QuotaExceeded,
    #[error("usage not included")]
    UsageNotIncluded,
    #[error("retryable error: {message}")]
    Retryable {
        message: String,
        delay: Option<Duration>,
    },
    #[error("rate limit: {0}")]
    RateLimit(String),
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("cyber policy: {message}")]
    CyberPolicy { message: String },
    #[error("server overloaded")]
    ServerOverloaded,
}

impl From<RateLimitError> for ApiError {
    fn from(err: RateLimitError) -> Self {
        Self::RateLimit(err.to_string())
    }
}
