use anthropic_sdk::Error as AnthropicError;
use anthropic_sdk::HttpApiError;
use reqwest::StatusCode;

use codex_protocol::error::CodexErr;
use codex_protocol::error::RetryLimitReachedError;
use codex_protocol::error::UnexpectedResponseError;

pub(crate) fn map_anthropic_error(err: AnthropicError) -> CodexErr {
    match err {
        AnthropicError::AuthMissing => CodexErr::InvalidRequest(
            "anthropic authentication missing: configure env_key or experimental_bearer_token"
                .to_string(),
        ),
        AnthropicError::Http(http_error) => map_http_error(http_error),
        AnthropicError::Timeout => CodexErr::Timeout,
        AnthropicError::Transport(source) => {
            if source.is_timeout() {
                CodexErr::Timeout
            } else {
                CodexErr::Stream(source.to_string(), None)
            }
        }
        AnthropicError::Json(error) => CodexErr::Stream(error.to_string(), None),
        AnthropicError::Url(error) => CodexErr::Stream(error.to_string(), None),
        AnthropicError::InvalidHeaderValue(error) => CodexErr::Stream(error.to_string(), None),
        AnthropicError::InvalidSse(message)
        | AnthropicError::InvalidJsonl(message)
        | AnthropicError::Internal(message) => CodexErr::Stream(message, None),
        AnthropicError::Aborted => CodexErr::TurnAborted,
    }
}

fn map_http_error(err: HttpApiError) -> CodexErr {
    match err {
        HttpApiError::BadRequest(api_error) => CodexErr::InvalidRequest(api_error.to_string()),
        HttpApiError::RateLimit(api_error) => CodexErr::RetryLimit(RetryLimitReachedError {
            status: StatusCode::TOO_MANY_REQUESTS,
            request_id: api_error.request_id,
        }),
        HttpApiError::InternalServer(_) => CodexErr::InternalServerError,
        HttpApiError::Authentication(api_error) => {
            map_unexpected_status(StatusCode::UNAUTHORIZED, api_error)
        }
        HttpApiError::PermissionDenied(api_error) => {
            map_unexpected_status(StatusCode::FORBIDDEN, api_error)
        }
        HttpApiError::NotFound(api_error) => {
            map_unexpected_status(StatusCode::NOT_FOUND, api_error)
        }
        HttpApiError::Conflict(api_error) => map_unexpected_status(StatusCode::CONFLICT, api_error),
        HttpApiError::UnprocessableEntity(api_error) => {
            map_unexpected_status(StatusCode::UNPROCESSABLE_ENTITY, api_error)
        }
        HttpApiError::Other(api_error) => map_other_api_error(api_error),
    }
}

fn map_other_api_error(api_error: anthropic_sdk::ApiError) -> CodexErr {
    if anthropic_error_type(&api_error).is_some_and(|error_type| error_type == "overloaded_error") {
        return CodexErr::ServerOverloaded;
    }

    map_unexpected_status(
        api_error
            .status
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        api_error,
    )
}

fn map_unexpected_status(status: StatusCode, api_error: anthropic_sdk::ApiError) -> CodexErr {
    let body = api_error
        .message
        .or_else(|| api_error.body.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| "unknown error".to_string());
    CodexErr::UnexpectedStatus(UnexpectedResponseError {
        status,
        body,
        url: None,
        cf_ray: None,
        request_id: api_error.request_id,
        identity_authorization_error: None,
        identity_error_code: None,
    })
}

fn anthropic_error_type(api_error: &anthropic_sdk::ApiError) -> Option<&str> {
    api_error
        .body
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("error"))
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("type"))
        .and_then(serde_json::Value::as_str)
}
