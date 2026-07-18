use crate::NormalizedMoonshotCommands;
use codex_api::MOONSHOT_SEARCH_MAX_RESULTS;
use codex_api::MoonshotSearchClient;
use codex_api::MoonshotSearchError;
use codex_api::MoonshotSearchResult;
use codex_utils_string::approx_bytes_for_tokens;
use codex_utils_string::take_bytes_at_char_boundary;
use serde::Serialize;

const MAX_TITLE_CHARS: usize = 512;
const MAX_URL_CHARS: usize = 2_048;
const MAX_SNIPPET_CHARS: usize = 2_048;
const MAX_SITE_NAME_CHARS: usize = 256;
const MAX_DATE_CHARS: usize = 128;
const MAX_OUTPUT_TOKENS: usize = 8_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundedSearchResult {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoonshotSearchExecution {
    pub output: String,
    pub results: Vec<BoundedSearchResult>,
}

pub async fn execute_moonshot_search(
    client: &MoonshotSearchClient,
    commands: &NormalizedMoonshotCommands,
    call_id: Option<&str>,
    turn_token_budget: usize,
) -> Result<MoonshotSearchExecution, MoonshotSearchError> {
    let mut results = Vec::new();
    let mut omitted_results = 0usize;
    let mut fields_truncated = false;
    for query in &commands.queries {
        let response = client.search(query, call_id).await?;
        omitted_results = omitted_results.saturating_add(response.omitted_results);
        for result in response.search_results {
            if results.len() >= MOONSHOT_SEARCH_MAX_RESULTS {
                omitted_results = omitted_results.saturating_add(1);
                continue;
            }
            let (result, truncated) = bound_result(result);
            fields_truncated |= truncated;
            results.push(result);
        }
    }

    let mut output = String::from("<external_context source=\"web_search\">\n");
    if results.is_empty() {
        output.push_str("No Moonshot search results were found.\n");
    } else {
        for (index, result) in results.iter().enumerate() {
            let title = escape_external_context_text(&result.title);
            let url = escape_external_context_text(&result.url);
            let snippet = escape_external_context_text(&result.snippet);
            output.push_str(&format!(
                "{}. {title}\n   URL: {url}\n   Snippet: {snippet}\n",
                index + 1,
            ));
            if let Some(site_name) = &result.site_name {
                let site_name = escape_external_context_text(site_name);
                output.push_str(&format!("   Site: {site_name}\n"));
            }
            if let Some(date) = &result.date {
                let date = escape_external_context_text(date);
                output.push_str(&format!("   Date: {date}\n"));
            }
        }
        output.push_str("Cite useful sources with markdown links using the returned URLs. Read the page with an available fetch/read-page capability when the snippet is insufficient.\n");
    }
    if commands.ignored_filter_note {
        output.push_str(
            "Note: recency and domain filters were not applied by Moonshot simple search.\n",
        );
    }
    if commands.ignored_response_length_note {
        output.push_str("Note: response_length was not applied by Moonshot simple search.\n");
    }
    if omitted_results > 0 {
        output.push_str(&format!(
            "Note: {omitted_results} additional results were omitted by the 8-result limit.\n"
        ));
    }
    if fields_truncated {
        output.push_str("Note: one or more result fields were safely truncated.\n");
    }
    output.push_str("</external_context>");
    let token_budget = turn_token_budget.min(MAX_OUTPUT_TOKENS);
    let output = hard_cap_output(&output, token_budget);
    Ok(MoonshotSearchExecution { output, results })
}

fn hard_cap_output(output: &str, token_budget: usize) -> String {
    let max_bytes = approx_bytes_for_tokens(token_budget);
    if output.len() <= max_bytes {
        return output.to_string();
    }
    const SUFFIX: &str = "\n[web search output, result fields, or omitted-result notices were truncated by safety limits]\n</external_context>";
    if max_bytes <= SUFFIX.len() {
        return take_bytes_at_char_boundary(SUFFIX, max_bytes).to_string();
    }
    let prefix = take_bytes_at_char_boundary(output, max_bytes - SUFFIX.len());
    format!("{prefix}{SUFFIX}")
}

fn escape_external_context_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn bound_result(result: MoonshotSearchResult) -> (BoundedSearchResult, bool) {
    let (title, title_truncated) = truncate_chars(&result.title, MAX_TITLE_CHARS);
    let (url, url_truncated) = truncate_chars(&result.url, MAX_URL_CHARS);
    let (snippet, snippet_truncated) = truncate_chars(&result.snippet, MAX_SNIPPET_CHARS);
    let (date, date_truncated) = truncate_optional(result.date, MAX_DATE_CHARS);
    let (site_name, site_name_truncated) = truncate_optional(result.site_name, MAX_SITE_NAME_CHARS);
    (
        BoundedSearchResult {
            kind: "text_result",
            title,
            url,
            snippet,
            date,
            site_name,
        },
        title_truncated
            || url_truncated
            || snippet_truncated
            || date_truncated
            || site_name_truncated,
    )
}

fn truncate_optional(value: Option<String>, limit: usize) -> (Option<String>, bool) {
    match value {
        Some(value) => {
            let (value, truncated) = truncate_chars(&value, limit);
            (Some(value), truncated)
        }
        None => (None, false),
    }
}

fn truncate_chars(value: &str, limit: usize) -> (String, bool) {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(limit).collect::<String>();
    let truncated = chars.next().is_some();
    (prefix, truncated)
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
