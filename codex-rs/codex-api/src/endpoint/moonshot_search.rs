use codex_http_client::build_reqwest_client_with_custom_ca;
use codex_http_client::collect_bytes_bounded;
use http::HeaderMap;
use http::HeaderValue;
use http::header::AUTHORIZATION;
use http::header::CONTENT_TYPE;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::IgnoredAny;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;
use std::fmt;
use thiserror::Error;
use url::Url;

pub const MOONSHOT_SEARCH_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MOONSHOT_SEARCH_MAX_ERROR_BYTES: usize = 16 * 1024;
pub const MOONSHOT_SEARCH_MAX_RESULTS: usize = 8;
pub const MOONSHOT_TOOL_CALL_ID_HEADER: &str = "X-Msh-Tool-Call-Id";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MoonshotSearchRequest<'a> {
    pub text_query: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoonshotSearchResponse {
    pub search_results: Vec<MoonshotSearchResult>,
    /// Results skipped while decoding because [`MOONSHOT_SEARCH_MAX_RESULTS`] was reached.
    pub omitted_results: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MoonshotSearchResult {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub site_name: Option<String>,
}

#[derive(Debug, Error)]
pub enum MoonshotSearchError {
    #[error("failed to build Moonshot search request: {0}")]
    InvalidRequest(String),
    #[error("Moonshot search transport failed: {0}")]
    Transport(String),
    // Keep the remote-controlled diagnostic out of Display because callers may
    // surface the error text to the model. The bounded field remains available
    // to trusted diagnostics through structured matching or Debug output.
    #[error("Moonshot search returned HTTP {status}")]
    Http {
        status: StatusCode,
        diagnostic: String,
    },
    #[error("Moonshot search response exceeds the 1 MiB limit")]
    ResponseTooLarge,
    #[error("Moonshot search returned invalid JSON: {0}")]
    InvalidResponse(String),
    #[error("Moonshot search refused to follow HTTP redirect {status}")]
    Redirect { status: StatusCode },
}

/// Builds the reqwest client used for Moonshot search.
///
/// Redirects are disabled so Bearer tokens and custom auth headers are never
/// forwarded to an attacker-controlled Location.
pub fn build_moonshot_search_http_client() -> Result<reqwest::Client, MoonshotSearchError> {
    match build_reqwest_client_with_custom_ca(
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()),
    ) {
        Ok(client) => Ok(client),
        Err(_) => reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| MoonshotSearchError::Transport(error.to_string())),
    }
}

pub struct MoonshotSearchClient {
    client: reqwest::Client,
    url: Url,
    bearer_token: String,
    custom_headers: HeaderMap,
}

impl MoonshotSearchClient {
    pub fn new(
        client: reqwest::Client,
        url: Url,
        bearer_token: String,
        custom_headers: HeaderMap,
    ) -> Result<Self, MoonshotSearchError> {
        if bearer_token.trim().is_empty() {
            return Err(MoonshotSearchError::InvalidRequest(
                "Moonshot search requires a non-empty Bearer token".to_string(),
            ));
        }
        for reserved in [
            AUTHORIZATION.as_str(),
            CONTENT_TYPE.as_str(),
            MOONSHOT_TOOL_CALL_ID_HEADER,
        ] {
            if custom_headers
                .keys()
                .any(|name| name.as_str().eq_ignore_ascii_case(reserved))
            {
                return Err(MoonshotSearchError::InvalidRequest(format!(
                    "custom Moonshot search headers may not override `{reserved}`"
                )));
            }
        }
        Ok(Self {
            client,
            url,
            bearer_token,
            custom_headers,
        })
    }

    pub async fn search(
        &self,
        text_query: &str,
        call_id: Option<&str>,
    ) -> Result<MoonshotSearchResponse, MoonshotSearchError> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.bearer_token))
            .map_err(|_| {
                MoonshotSearchError::InvalidRequest(
                    "Moonshot bearer token cannot be encoded as an HTTP header".to_string(),
                )
            })?;
        authorization.set_sensitive(true);
        let mut request = self
            .client
            .post(self.url.clone())
            .headers(self.custom_headers.clone())
            .header(AUTHORIZATION, authorization)
            .header(CONTENT_TYPE, "application/json")
            .json(&MoonshotSearchRequest { text_query });
        if let Some(call_id) = call_id.filter(|call_id| !call_id.is_empty()) {
            request = request.header(MOONSHOT_TOOL_CALL_ID_HEADER, call_id);
        }
        let response = request
            .send()
            .await
            .map_err(|error| MoonshotSearchError::Transport(error.to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            return Err(MoonshotSearchError::Redirect { status });
        }
        let limit = if status.is_success() {
            MOONSHOT_SEARCH_MAX_RESPONSE_BYTES
        } else {
            MOONSHOT_SEARCH_MAX_ERROR_BYTES
        };
        let body = collect_bytes_bounded(response.bytes_stream(), limit)
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData && status.is_success() {
                    MoonshotSearchError::ResponseTooLarge
                } else if error.kind() == std::io::ErrorKind::InvalidData {
                    MoonshotSearchError::Http {
                        status,
                        diagnostic: "response body exceeded the 16 KiB diagnostic limit"
                            .to_string(),
                    }
                } else {
                    MoonshotSearchError::Transport(error.to_string())
                }
            })?;
        if !status.is_success() {
            return Err(MoonshotSearchError::Http {
                status,
                diagnostic: self.redact_diagnostic(&String::from_utf8_lossy(&body)),
            });
        }
        serde_json::from_slice(&body)
            .map_err(|error| MoonshotSearchError::InvalidResponse(error.to_string()))
    }

    fn redact_diagnostic(&self, diagnostic: &str) -> String {
        let mut diagnostic = diagnostic.replace(&self.bearer_token, "[REDACTED]");
        for value in self.custom_headers.values() {
            if let Ok(value) = value.to_str()
                && !value.is_empty()
            {
                diagnostic = diagnostic.replace(value, "[REDACTED]");
            }
        }
        diagnostic
    }
}

impl<'de> Deserialize<'de> for MoonshotSearchResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ResponseVisitor;

        impl<'de> Visitor<'de> for ResponseVisitor {
            type Value = MoonshotSearchResponse;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a Moonshot search response object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut search_results = None;
                let mut omitted_results = 0usize;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "search_results" {
                        let bounded = map.next_value::<BoundedSearchResults>()?;
                        search_results = Some(bounded.results);
                        omitted_results = bounded.omitted;
                    } else {
                        let _ = map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(MoonshotSearchResponse {
                    search_results: search_results.unwrap_or_default(),
                    omitted_results,
                })
            }
        }

        deserializer.deserialize_map(ResponseVisitor)
    }
}

struct BoundedSearchResults {
    results: Vec<MoonshotSearchResult>,
    omitted: usize,
}

impl<'de> Deserialize<'de> for BoundedSearchResults {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoundedResultsVisitor;

        impl<'de> Visitor<'de> for BoundedResultsVisitor {
            type Value = BoundedSearchResults;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a Moonshot search_results array")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut results = Vec::new();
                let mut omitted = 0usize;
                while results.len() < MOONSHOT_SEARCH_MAX_RESULTS {
                    match seq.next_element::<MoonshotSearchResult>()? {
                        Some(result) => results.push(result),
                        None => {
                            return Ok(BoundedSearchResults { results, omitted });
                        }
                    }
                }
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    omitted = omitted.saturating_add(1);
                }
                Ok(BoundedSearchResults { results, omitted })
            }
        }

        deserializer.deserialize_seq(BoundedResultsVisitor)
    }
}

#[cfg(test)]
#[path = "moonshot_search_tests.rs"]
mod tests;
