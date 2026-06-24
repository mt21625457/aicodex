use std::borrow::Cow;

use serde::Serialize;
use sqlx::FromRow;

pub(crate) const MAX_PERSISTED_LOG_BODY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub ts: i64,
    pub ts_nanos: i64,
    pub level: String,
    pub target: String,
    pub message: Option<String>,
    pub feedback_log_body: Option<String>,
    pub thread_id: Option<String>,
    pub process_uuid: Option<String>,
    pub module_path: Option<String>,
    pub file: Option<String>,
    pub line: Option<i64>,
}

#[derive(Clone, Debug, FromRow)]
pub struct LogRow {
    pub id: i64,
    pub ts: i64,
    pub ts_nanos: i64,
    pub level: String,
    pub target: String,
    pub message: Option<String>,
    pub thread_id: Option<String>,
    pub process_uuid: Option<String>,
    pub file: Option<String>,
    pub line: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct LogQuery {
    pub levels_upper: Vec<String>,
    pub from_ts: Option<i64>,
    pub to_ts: Option<i64>,
    pub module_like: Vec<String>,
    pub file_like: Vec<String>,
    pub thread_ids: Vec<String>,
    pub search: Option<String>,
    pub include_threadless: bool,
    pub after_id: Option<i64>,
    pub limit: Option<usize>,
    pub descending: bool,
}

pub(crate) fn truncate_persisted_log_body(body: String) -> String {
    if body.len() <= MAX_PERSISTED_LOG_BODY_BYTES {
        return body;
    }
    bounded_persisted_log_body(&body).into_owned()
}

pub(crate) fn bounded_persisted_log_body(body: &str) -> Cow<'_, str> {
    if body.len() <= MAX_PERSISTED_LOG_BODY_BYTES {
        return Cow::Borrowed(body);
    }
    let original_bytes = body.len();
    let suffix = format!("... [truncated, original_bytes={original_bytes}]");
    let max_body_bytes = MAX_PERSISTED_LOG_BODY_BYTES.saturating_sub(suffix.len());
    let truncate_at = floor_char_boundary(body, max_body_bytes);
    let mut truncated = String::with_capacity(truncate_at + suffix.len());
    truncated.push_str(&body[..truncate_at]);
    truncated.push_str(&suffix);
    Cow::Owned(truncated)
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}
