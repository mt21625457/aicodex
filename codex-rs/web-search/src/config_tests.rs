use super::*;
use http::HeaderMap;
use http::HeaderValue;
use http::header::AUTHORIZATION;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

fn config() -> MoonshotSearchConfig {
    MoonshotSearchConfig {
        enabled: true,
        base_url: None,
        env_key: None,
        api_key: None,
        custom_headers: BTreeMap::new(),
    }
}

#[test]
fn derives_url_and_reuses_same_provider_bearer() {
    let resolved = resolve_moonshot_search_config(
        &config(),
        Some("https://api.moonshot.cn/v1/"),
        Some("provider-token"),
    )
    .expect("same-origin provider token should be usable");
    assert_eq!(resolved.url.as_str(), "https://api.moonshot.cn/v1/search");
    assert_eq!(resolved.bearer_token, "provider-token");
}

#[test]
fn provider_token_from_auth_headers_accepts_claude_x_api_key() {
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_static("claude-key"));
    assert_eq!(
        provider_token_from_auth_headers(&headers).as_deref(),
        Some("claude-key")
    );

    headers.clear();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer bearer-token"),
    );
    assert_eq!(
        provider_token_from_auth_headers(&headers).as_deref(),
        Some("bearer-token")
    );
}

#[test]
fn cross_origin_requires_independent_credential() {
    let mut config = config();
    config.base_url = Some("https://search.example/v1/search".to_string());
    assert_eq!(
        resolve_moonshot_search_config(
            &config,
            Some("https://provider.example/v1"),
            Some("provider-token"),
        )
        .expect_err("cross-origin provider token must not be reused"),
        MoonshotConfigError::CrossOriginCredentialRequired
    );
    config.api_key = Some("independent".to_string());
    assert_eq!(
        resolve_moonshot_search_config(
            &config,
            Some("https://provider.example/v1"),
            Some("provider-token"),
        )
        .expect("independent credential should allow cross-origin search")
        .bearer_token,
        "independent"
    );
}

#[test]
fn environment_token_precedes_plaintext_and_provider_tokens() {
    let mut config = config();
    config.env_key = Some("MOONSHOT_API_KEY".to_string());
    config.api_key = Some("plaintext-token".to_string());
    let resolved = resolve_moonshot_search_config_with_env(
        &config,
        Some("https://provider.example/v1"),
        Some("provider-token"),
        |key| (key == "MOONSHOT_API_KEY").then(|| "environment-token".to_string()),
    )
    .expect("environment token should resolve");

    assert_eq!(resolved.bearer_token, "environment-token");
}

#[test]
fn rejects_reserved_headers_and_unsafe_urls() {
    for name in ["Authorization", "content-type", "X-MSH-TOOL-CALL-ID"] {
        let mut config = config();
        config.api_key = Some("token".to_string());
        config
            .custom_headers
            .insert(name.to_string(), "x".to_string());
        assert!(matches!(
            resolve_moonshot_search_config(&config, Some("https://provider.example/v1"), None,),
            Err(MoonshotConfigError::ReservedHeader(_))
        ));
    }
    for url in [
        "file:///tmp/search",
        "https://user@example.com/search",
        "https://example.com/search#fragment",
    ] {
        let mut config = config();
        config.base_url = Some(url.to_string());
        config.api_key = Some("token".to_string());
        assert_eq!(
            resolve_moonshot_search_config(&config, None, None)
                .expect_err("unsafe URL should fail"),
            MoonshotConfigError::InvalidUrl
        );
    }
}

#[test]
fn load_validation_does_not_require_runtime_credentials() {
    let mut config = config();
    config.base_url = Some("https://search.example/v1/search".to_string());
    assert_eq!(validate_moonshot_search_config(&config), Ok(()));

    config
        .custom_headers
        .insert("authorization".to_string(), "secret".to_string());
    assert_eq!(
        validate_moonshot_search_config(&config),
        Err(MoonshotConfigError::ReservedHeader(
            "authorization".to_string()
        ))
    );
}
