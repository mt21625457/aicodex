use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::web_search::web_search_action_detail;
use codex_login::default_client::build_reqwest_client;
use codex_protocol::items::TurnItem;
use codex_protocol::items::WebSearchItem;
use codex_protocol::models::WebSearchAction;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use regex_lite::Regex;
use reqwest::header::ACCEPT;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

const WEB_SEARCH_TOOL_NAME: &str = "web_search";
const DUCKDUCKGO_HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const WEB_SEARCH_USER_AGENT: &str = "aicodex-web-search/1.0";
const WEB_SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_WEB_SEARCH_QUERIES: usize = 5;
const MAX_RESULTS_PER_QUERY: usize = 5;

pub struct WebSearchHandler {
    spec: ToolSpec,
    endpoint: Url,
    client: reqwest::Client,
    allowed_domains: Vec<String>,
}

impl WebSearchHandler {
    pub(crate) fn new(spec: ToolSpec) -> Self {
        let endpoint = Url::parse(DUCKDUCKGO_HTML_ENDPOINT)
            .expect("built-in DuckDuckGo HTML endpoint must parse");
        let allowed_domains = allowed_domains_from_spec(&spec);
        Self {
            spec,
            endpoint,
            client: build_reqwest_client(),
            allowed_domains,
        }
    }

    #[cfg(test)]
    fn new_for_test(spec: ToolSpec, endpoint: Url, client: reqwest::Client) -> Self {
        let allowed_domains = allowed_domains_from_spec(&spec);
        Self {
            spec,
            endpoint,
            client,
            allowed_domains,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    queries: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct WebSearchResponse {
    source: &'static str,
    searches: Vec<WebSearchResultSet>,
}

#[derive(Debug, Serialize)]
struct WebSearchResultSet {
    query: String,
    results: Vec<WebSearchResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct WebSearchResult {
    title: String,
    url: String,
    snippet: String,
}

impl ToolHandler for WebSearchHandler {
    type Output = FunctionToolOutput;

    fn tool_name(&self) -> ToolName {
        ToolName::plain(WEB_SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> Option<ToolSpec> {
        Some(self.spec.clone())
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            ..
        } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "web_search handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: WebSearchArgs = parse_arguments(&arguments)?;
        let queries = normalize_queries(args)?;
        let action = web_search_action_for_queries(&queries);
        let query_detail = web_search_action_detail(&action);
        let item = TurnItem::WebSearch(WebSearchItem {
            id: call_id,
            query: query_detail,
            action,
        });
        session.emit_turn_item_started(turn.as_ref(), &item).await;
        let result = run_web_searches(
            &self.client,
            &self.endpoint,
            queries,
            self.allowed_domains.as_slice(),
        )
        .await;
        session.emit_turn_item_completed(turn.as_ref(), item).await;

        let response = result?;
        let text = serde_json::to_string_pretty(&response).unwrap_or_else(|err| {
            format!(r#"{{"error":"failed to serialize web search results: {err}"}}"#)
        });
        Ok(FunctionToolOutput::from_text(text, Some(true)))
    }
}

fn allowed_domains_from_spec(spec: &ToolSpec) -> Vec<String> {
    let ToolSpec::WebSearch { filters, .. } = spec else {
        return Vec::new();
    };

    filters
        .as_ref()
        .and_then(|filters| filters.allowed_domains.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|domain| normalize_domain_filter(domain))
        .collect()
}

fn normalize_domain_filter(domain: &str) -> Option<String> {
    let domain = domain.trim().trim_start_matches("*.").trim_end_matches('/');
    if domain.is_empty()
        || domain.contains('/')
        || domain.contains(':')
        || domain.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(domain.to_ascii_lowercase())
}

fn normalize_queries(args: WebSearchArgs) -> Result<Vec<String>, FunctionCallError> {
    let mut queries = Vec::new();
    if let Some(query) = args.query {
        push_query(&mut queries, query);
    }
    if let Some(batch) = args.queries {
        for query in batch {
            push_query(&mut queries, query);
        }
    }

    if queries.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "web_search requires `query` or `queries` with at least one non-empty string"
                .to_string(),
        ));
    }
    if queries.len() > MAX_WEB_SEARCH_QUERIES {
        queries.truncate(MAX_WEB_SEARCH_QUERIES);
    }
    Ok(queries)
}

fn push_query(queries: &mut Vec<String>, query: String) {
    let query = query.trim();
    if !query.is_empty() && !queries.iter().any(|existing| existing == query) {
        queries.push(query.to_string());
    }
}

fn web_search_action_for_queries(queries: &[String]) -> WebSearchAction {
    WebSearchAction::Search {
        query: queries.first().cloned(),
        queries: (queries.len() > 1).then_some(queries.to_vec()),
    }
}

async fn run_web_searches(
    client: &reqwest::Client,
    endpoint: &Url,
    queries: Vec<String>,
    allowed_domains: &[String],
) -> Result<WebSearchResponse, FunctionCallError> {
    let mut searches = Vec::with_capacity(queries.len());
    for query in queries {
        let effective_query = query_with_allowed_domains(&query, allowed_domains);
        let html = fetch_search_html(client, endpoint, &effective_query).await?;
        let results = parse_duckduckgo_results(&html, MAX_RESULTS_PER_QUERY);
        searches.push(WebSearchResultSet { query, results });
    }

    Ok(WebSearchResponse {
        source: "duckduckgo_html",
        searches,
    })
}

async fn fetch_search_html(
    client: &reqwest::Client,
    endpoint: &Url,
    query: &str,
) -> Result<String, FunctionCallError> {
    let mut url = endpoint.clone();
    url.query_pairs_mut().append_pair("q", query);
    let response = client
        .get(url)
        .header(USER_AGENT, WEB_SEARCH_USER_AGENT)
        .header(ACCEPT, "text/html,application/xhtml+xml")
        .timeout(WEB_SEARCH_TIMEOUT)
        .send()
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format!("web_search failed: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "web_search failed with HTTP status {status}"
        )));
    }
    response
        .text()
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format!("web_search failed: {err}")))
}

fn query_with_allowed_domains(query: &str, allowed_domains: &[String]) -> String {
    if allowed_domains.is_empty() {
        return query.to_string();
    }
    let sites = allowed_domains
        .iter()
        .filter_map(|domain| normalize_domain_filter(domain))
        .map(|domain| format!("site:{domain}"))
        .collect::<Vec<_>>();
    if sites.is_empty() {
        return query.to_string();
    }
    format!("{query} ({})", sites.join(" OR "))
}

fn parse_duckduckgo_results(html: &str, limit: usize) -> Vec<WebSearchResult> {
    let result_link_re = Regex::new(
        r#"(?is)<a[^>]*class=["'][^"']*result__a[^"']*["'][^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#,
    )
    .expect("result regex should compile");
    let snippet_re = Regex::new(
        r#"(?is)<(?:a|div)[^>]*class=["'][^"']*result__snippet[^"']*["'][^>]*>(.*?)</(?:a|div)>"#,
    )
    .expect("snippet regex should compile");

    let mut results = Vec::new();
    let mut seen_urls = HashSet::new();
    for captures in result_link_re.captures_iter(html) {
        let Some(link) = captures.get(1) else {
            continue;
        };
        let Some(title_html) = captures.get(2) else {
            continue;
        };
        let title = clean_html_text(title_html.as_str());
        if title.is_empty() {
            continue;
        }
        let url = normalize_duckduckgo_href(link.as_str());
        if url.is_empty() || !seen_urls.insert(url.clone()) {
            continue;
        }
        let snippet = captures.get(0).map_or_else(String::new, |match_| {
            let tail = &html[match_.end()..];
            let bounded_tail = take_prefix_chars(tail, 4000);
            snippet_re
                .captures(&bounded_tail)
                .and_then(|captures| captures.get(1))
                .map(|snippet| clean_html_text(snippet.as_str()))
                .unwrap_or_default()
        });
        results.push(WebSearchResult {
            title,
            url,
            snippet,
        });
        if results.len() >= limit {
            break;
        }
    }
    results
}

fn normalize_duckduckgo_href(href: &str) -> String {
    let href = decode_html_entities(href.trim());
    let candidate = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href
    };
    let Ok(url) = Url::parse(&candidate) else {
        return candidate;
    };
    if url
        .host_str()
        .is_some_and(|host| host.ends_with("duckduckgo.com"))
        && let Some((_, target)) = url.query_pairs().find(|(key, _)| key == "uddg")
    {
        return target.into_owned();
    }
    url.to_string()
}

fn clean_html_text(html: &str) -> String {
    let tag_re = Regex::new(r"(?is)<[^>]+>").expect("tag regex should compile");
    let text = tag_re.replace_all(html, " ");
    decode_html_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_html_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(start) = remaining.find('&') {
        output.push_str(&remaining[..start]);
        let after_amp = &remaining[start + 1..];
        let Some(end) = after_amp.find(';').filter(|end| *end <= 16) else {
            output.push('&');
            remaining = after_amp;
            continue;
        };
        let entity = &after_amp[..end];
        if let Some(decoded) = decode_entity(entity) {
            output.push_str(&decoded);
        } else {
            output.push('&');
            output.push_str(entity);
            output.push(';');
        }
        remaining = &after_amp[end + 1..];
    }
    output.push_str(remaining);
    output
}

fn decode_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => Some("&".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" | "#39" => Some("'".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "nbsp" => Some(" ".to_string()),
        _ => {
            let number = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse::<u32>().ok())
                })?;
            char::from_u32(number).map(|ch| ch.to_string())
        }
    }
}

fn take_prefix_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

#[cfg(test)]
#[path = "web_search_tests.rs"]
mod tests;
