use codex_api::AuthProvider;
use http::HeaderMap;
use http::HeaderValue;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Auth provider for Anthropic's native Claude Messages API.
#[derive(Clone, Default)]
pub struct AnthropicAuthProvider {
    pub api_key: Option<String>,
    pub auth_token: Option<String>,
}

impl AnthropicAuthProvider {
    pub fn new(api_key: Option<String>, auth_token: Option<String>) -> Self {
        Self {
            api_key,
            auth_token,
        }
    }
}

impl AuthProvider for AnthropicAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        if let Some(api_key) = self.api_key.as_ref()
            && let Ok(header) = HeaderValue::from_str(api_key)
        {
            let _ = headers.insert("x-api-key", header);
        } else if let Some(token) = self.auth_token.as_ref()
            && let Ok(header) = HeaderValue::from_str(&format!("Bearer {token}"))
        {
            let _ = headers.insert(http::header::AUTHORIZATION, header);
        }

        let _ = headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn anthropic_auth_provider_prefers_api_key_header() {
        let auth =
            AnthropicAuthProvider::new(Some("api-key".to_string()), Some("auth-token".to_string()));
        let mut headers = HeaderMap::new();

        auth.add_auth_headers(&mut headers);

        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("api-key")
        );
        assert_eq!(headers.get(http::header::AUTHORIZATION), None);
        assert_eq!(
            headers
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some(ANTHROPIC_VERSION)
        );
    }

    #[test]
    fn anthropic_auth_provider_can_use_bearer_token() {
        let auth = AnthropicAuthProvider::new(None, Some("auth-token".to_string()));
        let mut headers = HeaderMap::new();

        auth.add_auth_headers(&mut headers);

        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer auth-token")
        );
    }
}
