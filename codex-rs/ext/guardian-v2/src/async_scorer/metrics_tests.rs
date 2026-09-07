use super::LunaSamplerError;
use super::sampler_failure_reason;
use codex_api::ApiError;
use codex_api::ProviderMediaErrorKind;
use codex_api::ProviderStreamErrorKind;
use pretty_assertions::assert_eq;

#[test]
fn provider_failure_reasons_do_not_include_response_content() {
    for message in ["credential-secret", "response payload with user data"] {
        let errors = [
            ApiError::MalformedResponse {
                message: message.to_owned(),
            },
            ApiError::StreamFailure {
                kind: ProviderStreamErrorKind::ParseError,
                message: message.to_owned(),
            },
            ApiError::StreamIdleTimeout {
                message: message.to_owned(),
            },
            ApiError::ProviderMedia {
                kind: ProviderMediaErrorKind::InvalidImage,
                message: message.to_owned(),
            },
        ];

        assert_eq!(
            errors.map(|error| sampler_failure_reason(&LunaSamplerError::Api(error))),
            [
                "malformed_response",
                "stream_failure",
                "stream_idle_timeout",
                "provider_media_error",
            ]
        );
    }
}
