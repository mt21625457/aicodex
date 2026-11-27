//! Classifies failed Responses events and their retry advice.
//! Only rate-limit codes derive retry delays from plaintext messages.

use crate::error::ApiError;
use crate::error::ProviderMediaErrorKind;
use crate::error::parse_terminal_provider_error;
use crate::responses_headers::json_headers_to_http_headers;
use codex_http_client::RetryAfter;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

// Keep decoding unused fields: malformed values currently produce a stream error.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Error {
    r#type: Option<String>,
    code: Option<String>,
    message: Option<String>,
    plan_type: Option<String>,
    resets_at: Option<i64>,
    #[serde(default)]
    misalignment: Option<Value>,
}

pub(super) fn parse_failed_response(response: Option<Value>) -> ApiError {
    let error = response.as_ref().and_then(|response| response.get("error"));
    let retry_after_header = stream_retry_after(error, /*envelope_headers*/ None);
    if let Some(error) = parse_terminal_provider_error(error, /*status*/ None) {
        return error;
    }
    let Some(error) = error.and_then(|error| Error::deserialize(error).ok()) else {
        let message = "response.failed event received".to_string();
        return match retry_after_header {
            Some(_) => ApiError::Retryable {
                message,
                retry_after: retry_after_header,
            },
            None => ApiError::Stream(message),
        };
    };

    if let Some(classified) = classify_retryable_stream_error(
        error.code.as_deref(),
        error.message.as_deref(),
        retry_after_header,
    ) {
        return classified;
    }
    let message = error.message.unwrap_or_default();
    if let Some(kind) = ProviderMediaErrorKind::classify(/*status*/ None, &message) {
        return ApiError::ProviderMedia { kind, message };
    }
    ApiError::Retryable {
        message,
        retry_after: retry_after_header,
    }
}

pub(super) fn stream_retry_after(
    error: Option<&Value>,
    envelope_headers: Option<&Value>,
) -> Option<RetryAfter> {
    [
        error.and_then(|error| error.get("headers")),
        envelope_headers,
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_object)
    .find_map(|headers| RetryAfter::from_headers(&json_headers_to_http_headers(headers)))
}

pub(super) fn classify_stream_error(
    error: Option<&Value>,
    retry_after_header: Option<RetryAfter>,
) -> Option<ApiError> {
    if let Some(error) = parse_terminal_provider_error(error, /*status*/ None) {
        return Some(error);
    }
    let error = error?;
    classify_retryable_stream_error(
        error.get("code").and_then(Value::as_str),
        error.get("message").and_then(Value::as_str),
        retry_after_header,
    )
}

fn classify_retryable_stream_error(
    code: Option<&str>,
    message: Option<&str>,
    retry_after_header: Option<RetryAfter>,
) -> Option<ApiError> {
    let classified = match code? {
        "policy_capacity_unavailable" => ApiError::PolicyCapacityUnavailable {
            retry_after: retry_after_header,
        },
        "server_is_overloaded" => ApiError::ServerOverloaded {
            retry_after: retry_after_header,
        },
        "rate_limit_exceeded" | "slow_down" => {
            let retry_after = retry_after_header
                .or_else(|| try_parse_retry_delay(message?).and_then(RetryAfter::from_delay));
            ApiError::RateLimitExceeded {
                message: message.unwrap_or_default().to_owned(),
                retry_after,
            }
        }
        _ => return None,
    };
    Some(classified)
}

fn try_parse_retry_delay(message: &str) -> Option<Duration> {
    let re = rate_limit_regex();
    if let Some(captures) = re.captures(message) {
        let seconds = captures.get(1);
        let unit = captures.get(2);

        if let (Some(value), Some(unit)) = (seconds, unit) {
            let value = value.as_str().parse::<f64>().ok()?;
            let unit = unit.as_str().to_ascii_lowercase();

            if unit == "s" || unit.starts_with("second") {
                return Duration::try_from_secs_f64(value).ok();
            } else if unit == "ms" {
                return Some(Duration::from_millis(value as u64));
            }
        }
    }
    None
}

fn rate_limit_regex() -> &'static regex_lite::Regex {
    static RE: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    #[expect(clippy::unwrap_used)]
    RE.get_or_init(|| {
        regex_lite::Regex::new(r"(?i)try again in\s*(\d+(?:\.\d+)?)\s*(s|ms|seconds?)").unwrap()
    })
}

#[cfg(test)]
#[path = "responses_error_parser_tests.rs"]
mod tests;
