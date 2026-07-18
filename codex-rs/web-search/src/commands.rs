use serde_json::Value;
use std::collections::HashSet;
use thiserror::Error;

const MAX_QUERIES: usize = 4;
/// Cap each query before the authenticated Moonshot POST (aligned with R9 URL/snippet scale).
const MAX_QUERY_CHARS: usize = 2_048;
const QUERY_KEYS: &[&str] = &["query", "queries", "search_query", "response_length"];
const RICH_COMMAND_KEYS: &[&str] = &[
    "open",
    "click",
    "find",
    "screenshot",
    "image_query",
    "finance",
    "weather",
    "sports",
    "time",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedMoonshotCommands {
    pub queries: Vec<String>,
    pub ignored_filter_note: bool,
    pub ignored_response_length_note: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MoonshotCommandError {
    #[error("failed to parse web search arguments: {0}")]
    InvalidJson(String),
    #[error("Moonshot simple search does not recognize top-level field `{0}`")]
    UnknownField(String),
    #[error(
        "Moonshot simple search supports query-only calls; `{0}` requires a page-reading or richer web tool"
    )]
    UnsupportedRichCommand(String),
    #[error("Moonshot simple search requires non-empty query text")]
    EmptyQuery,
    #[error("Moonshot simple search accepts at most four unique queries per call")]
    TooManyQueries,
    #[error("Moonshot simple search query exceeds the {0}-character limit")]
    QueryTooLong(usize),
    #[error("Moonshot simple search query fields have an invalid shape")]
    InvalidQueryShape,
}

pub fn normalize_moonshot_commands(
    arguments: &str,
) -> Result<NormalizedMoonshotCommands, MoonshotCommandError> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| MoonshotCommandError::InvalidJson(error.to_string()))?;
    let object = value
        .as_object()
        .ok_or(MoonshotCommandError::InvalidQueryShape)?;
    for key in object.keys() {
        if !QUERY_KEYS.contains(&key.as_str()) && !RICH_COMMAND_KEYS.contains(&key.as_str()) {
            return Err(MoonshotCommandError::UnknownField(key.clone()));
        }
    }
    for key in RICH_COMMAND_KEYS {
        if object.get(*key).is_some_and(non_empty_payload) {
            return Err(MoonshotCommandError::UnsupportedRichCommand(
                (*key).to_string(),
            ));
        }
    }

    let mut queries = Vec::new();
    let mut seen = HashSet::new();
    if let Some(value) = object.get("query") {
        let query = value
            .as_str()
            .ok_or(MoonshotCommandError::InvalidQueryShape)?;
        push_query(&mut queries, &mut seen, query)?;
    }
    if let Some(value) = object.get("queries") {
        let values = value
            .as_array()
            .ok_or(MoonshotCommandError::InvalidQueryShape)?;
        for value in values {
            let query = value
                .as_str()
                .ok_or(MoonshotCommandError::InvalidQueryShape)?;
            push_query(&mut queries, &mut seen, query)?;
        }
    }
    let mut ignored_filter_note = false;
    if let Some(value) = object.get("search_query") {
        let values = value
            .as_array()
            .ok_or(MoonshotCommandError::InvalidQueryShape)?;
        for value in values {
            let query = value
                .as_object()
                .ok_or(MoonshotCommandError::InvalidQueryShape)?;
            let q = query
                .get("q")
                .and_then(Value::as_str)
                .ok_or(MoonshotCommandError::InvalidQueryShape)?;
            ignored_filter_note |= query.get("recency").is_some_and(non_empty_payload)
                || query.get("domains").is_some_and(non_empty_payload);
            push_query(&mut queries, &mut seen, q)?;
        }
    }
    if queries.is_empty() {
        return Err(MoonshotCommandError::EmptyQuery);
    }
    if queries.len() > MAX_QUERIES {
        return Err(MoonshotCommandError::TooManyQueries);
    }
    Ok(NormalizedMoonshotCommands {
        queries,
        ignored_filter_note,
        ignored_response_length_note: object.get("response_length").is_some_and(non_empty_payload),
    })
}

fn push_query(
    queries: &mut Vec<String>,
    seen: &mut HashSet<String>,
    query: &str,
) -> Result<(), MoonshotCommandError> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(MoonshotCommandError::QueryTooLong(MAX_QUERY_CHARS));
    }
    if seen.insert(query.to_string()) {
        queries.push(query.to_string());
    }
    Ok(())
}

fn non_empty_payload(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(_) => true,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
