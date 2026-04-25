use anthropic_sdk::ClientOptions;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use std::collections::HashMap;

use crate::dto::AnthropicTurnRequest;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;

pub(crate) fn build_client_options(request: &AnthropicTurnRequest) -> Result<ClientOptions> {
    let provider = &request.provider;
    let api_key = provider.api_key()?;
    let auth_token = if api_key.is_some() {
        None
    } else {
        provider.experimental_bearer_token.clone()
    };

    if api_key.is_none() && auth_token.is_none() {
        return Err(CodexErr::InvalidRequest(
            "anthropic provider requires credentials via env_key or experimental_bearer_token"
                .to_string(),
        ));
    }

    Ok(ClientOptions {
        api_key,
        auth_token,
        base_url: provider
            .base_url
            .clone()
            .or_else(|| Some("https://api.anthropic.com".to_string())),
        timeout: Some(provider.stream_idle_timeout()),
        max_retries: Some(provider.request_max_retries().min(u64::from(u32::MAX)) as u32),
        default_headers: build_default_headers(
            provider.http_headers.as_ref(),
            provider.env_http_headers.as_ref(),
        ),
    })
}

fn build_default_headers(
    http_headers: Option<&HashMap<String, String>>,
    env_http_headers: Option<&HashMap<String, String>>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    insert_headers(&mut headers, http_headers);
    insert_env_headers(&mut headers, env_http_headers);
    headers
}

fn insert_headers(headers: &mut HeaderMap, source: Option<&HashMap<String, String>>) {
    let Some(source) = source else {
        return;
    };

    for (key, value) in source {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(key.as_str()),
            HeaderValue::try_from(value.as_str()),
        ) {
            headers.insert(name, value);
        }
    }
}

fn insert_env_headers(headers: &mut HeaderMap, source: Option<&HashMap<String, String>>) {
    let Some(source) = source else {
        return;
    };

    for (header_name, env_var) in source {
        if let Ok(env_value) = std::env::var(env_var)
            && !env_value.trim().is_empty()
            && let (Ok(name), Ok(value)) = (
                HeaderName::try_from(header_name.as_str()),
                HeaderValue::try_from(env_value.as_str()),
            )
        {
            headers.insert(name, value);
        }
    }
}
