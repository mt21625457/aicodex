use codex_config::config_toml::MoonshotSearchConfig;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use http::header::AUTHORIZATION;
use http::header::CONTENT_TYPE;
use std::str::FromStr;
use thiserror::Error;
use url::Url;

const TOOL_CALL_ID_HEADER: &str = "x-msh-tool-call-id";

#[derive(Debug, Clone)]
pub struct ResolvedMoonshotSearchConfig {
    pub url: Url,
    pub bearer_token: String,
    pub custom_headers: HeaderMap,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MoonshotConfigError {
    #[error("Moonshot search requires an absolute http or https URL without userinfo or fragment")]
    InvalidUrl,
    #[error("Moonshot search custom header `{0}` is reserved")]
    ReservedHeader(String),
    #[error("Moonshot search custom header name is invalid")]
    InvalidHeaderName,
    #[error("Moonshot search custom header value is invalid")]
    InvalidHeaderValue,
    #[error("Moonshot search requires an independent credential for a cross-origin URL")]
    CrossOriginCredentialRequired,
    #[error(
        "Moonshot search requires a Bearer credential; configure moonshot_search.env_key or api_key"
    )]
    MissingCredential,
    #[error(
        "Moonshot search cannot derive a URL because the primary provider has no valid base URL"
    )]
    MissingProviderUrl,
}

pub fn bearer_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// Raw provider credential usable as a Moonshot `Authorization: Bearer` token.
///
/// Accepts Claude-wire `x-api-key` as well as `Authorization: Bearer`, so same-origin
/// Kimi/Claude providers can reuse inference credentials without duplicating secrets
/// under `[moonshot_search]`. Does not forward the full Claude header set.
pub fn provider_token_from_auth_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = bearer_token_from_headers(headers) {
        return Some(token);
    }
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

pub fn resolve_moonshot_search_config(
    config: &MoonshotSearchConfig,
    provider_base_url: Option<&str>,
    provider_bearer_token: Option<&str>,
) -> Result<ResolvedMoonshotSearchConfig, MoonshotConfigError> {
    resolve_moonshot_search_config_with_env(
        config,
        provider_base_url,
        provider_bearer_token,
        |key| std::env::var(key).ok(),
    )
}

fn resolve_moonshot_search_config_with_env(
    config: &MoonshotSearchConfig,
    provider_base_url: Option<&str>,
    provider_bearer_token: Option<&str>,
    read_env: impl Fn(&str) -> Option<String>,
) -> Result<ResolvedMoonshotSearchConfig, MoonshotConfigError> {
    validate_moonshot_search_config(config)?;
    let provider_url = provider_base_url.and_then(|value| parse_search_url(value).ok());
    let explicit = config
        .base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let (url, derived) = match explicit {
        Some(value) => (parse_search_url(value)?, false),
        None => {
            let mut url = provider_url
                .clone()
                .ok_or(MoonshotConfigError::MissingProviderUrl)?;
            let path = format!("{}/search", url.path().trim_end_matches('/'));
            url.set_path(&path);
            url.set_query(None);
            (url, true)
        }
    };
    let independent_token = config
        .env_key
        .as_deref()
        .and_then(read_env)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            config
                .api_key
                .clone()
                .filter(|value| !value.trim().is_empty())
        });
    let same_origin = provider_url
        .as_ref()
        .is_some_and(|provider_url| same_origin(&url, provider_url));
    let bearer_token = match independent_token {
        Some(token) => token,
        None if derived || same_origin => provider_bearer_token
            .filter(|token| !token.trim().is_empty())
            .map(str::to_string)
            .ok_or(MoonshotConfigError::MissingCredential)?,
        None => return Err(MoonshotConfigError::CrossOriginCredentialRequired),
    };
    let custom_headers = validated_custom_headers(config)?;
    Ok(ResolvedMoonshotSearchConfig {
        url,
        bearer_token,
        custom_headers,
    })
}

pub fn validate_moonshot_search_config(
    config: &MoonshotSearchConfig,
) -> Result<(), MoonshotConfigError> {
    if let Some(base_url) = config
        .base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parse_search_url(base_url)?;
    }
    validated_custom_headers(config)?;
    Ok(())
}

fn validated_custom_headers(
    config: &MoonshotSearchConfig,
) -> Result<HeaderMap, MoonshotConfigError> {
    let mut custom_headers = HeaderMap::new();
    for (name, value) in &config.custom_headers {
        if is_reserved_header(name) {
            return Err(MoonshotConfigError::ReservedHeader(name.clone()));
        }
        let name =
            HeaderName::from_str(name).map_err(|_| MoonshotConfigError::InvalidHeaderName)?;
        let value =
            HeaderValue::from_str(value).map_err(|_| MoonshotConfigError::InvalidHeaderValue)?;
        custom_headers.insert(name, value);
    }
    Ok(custom_headers)
}

fn parse_search_url(value: &str) -> Result<Url, MoonshotConfigError> {
    let url = Url::parse(value.trim()).map_err(|_| MoonshotConfigError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(MoonshotConfigError::InvalidUrl);
    }
    Ok(url)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn is_reserved_header(name: &str) -> bool {
    name.eq_ignore_ascii_case(AUTHORIZATION.as_str())
        || name.eq_ignore_ascii_case(CONTENT_TYPE.as_str())
        || name.eq_ignore_ascii_case(TOOL_CALL_ID_HEADER)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
