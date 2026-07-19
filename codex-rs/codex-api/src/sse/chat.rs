use crate::ChatToolCallInfo;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::error::ProviderStreamErrorKind;
use crate::sse::progress::ProgressDeadline;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

mod state;

use state::ChatStreamState;

const REQUEST_ID_HEADER: &str = "x-request-id";
// Chat providers may use different tokenizers. Apply the repository-wide four-bytes-per-token
// estimate, then enforce the resulting byte budgets exactly while streaming.
const MAX_CHAT_CONTEXT_ITEM_TOKENS: usize = 10_000;
const MAX_CHAT_CONTEXT_ITEM_BYTES: usize = MAX_CHAT_CONTEXT_ITEM_TOKENS * 4;
const MAX_CHAT_RESPONSE_CONTEXT_TOKENS: usize = 16_000;
const MAX_CHAT_RESPONSE_CONTEXT_BYTES: usize = MAX_CHAT_RESPONSE_CONTEXT_TOKENS * 4;
const MAX_CHAT_TOOL_CALLS: usize = 64;
const MAX_CHAT_WIRE_IDENTIFIER_BYTES: usize = 512;

pub fn spawn_chat_response_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_call_info: HashMap<String, ChatToolCallInfo>,
) -> ResponseStream {
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(process_sse(
        stream_response.bytes,
        tx_event,
        idle_timeout,
        telemetry,
        tool_call_info,
    ));
    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

impl ChatCompletionChunk {
    fn is_meaningful(&self) -> bool {
        self.usage.is_some() || self.choices.iter().any(ChatChoice::is_meaningful)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    delta: ChatDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

impl ChatChoice {
    fn is_meaningful(&self) -> bool {
        self.finish_reason.is_some() || self.delta.is_meaningful()
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChatDelta {
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    reasoning_content: Option<Value>,
    #[serde(default)]
    reasoning: Option<Value>,
    #[serde(default)]
    thinking: Option<Value>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCallDelta>,
}

impl ChatDelta {
    fn is_meaningful(&self) -> bool {
        self.content.as_ref().is_some_and(value_has_text)
            || self.reasoning_text().is_some()
            || self.tool_calls.iter().any(ChatToolCallDelta::is_meaningful)
    }

    fn reasoning_text(&self) -> Option<String> {
        [
            self.reasoning_content.as_ref(),
            self.reasoning.as_ref(),
            self.thinking.as_ref(),
        ]
        .into_iter()
        .flatten()
        .find_map(text_from_value)
        .filter(|text| !text.is_empty())
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChatToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChatFunctionDelta>,
}

impl ChatToolCallDelta {
    fn is_meaningful(&self) -> bool {
        self.id.as_ref().is_some_and(|id| !id.is_empty())
            || self.function.as_ref().is_some_and(|function| {
                function.name.as_ref().is_some_and(|name| !name.is_empty())
                    || function
                        .arguments
                        .as_ref()
                        .is_some_and(|arguments| !arguments.is_empty())
            })
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChatFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: i64,
    #[serde(default)]
    completion_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
    #[serde(default)]
    prompt_tokens_details: Option<ChatPromptTokenDetails>,
    #[serde(default)]
    completion_tokens_details: Option<ChatCompletionTokenDetails>,
}

impl ChatUsage {
    fn token_usage(&self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.prompt_tokens,
            cached_input_tokens: self
                .prompt_tokens_details
                .as_ref()
                .map_or(0, |details| details.cached_tokens),
            cache_write_input_tokens: 0,
            output_tokens: self.completion_tokens,
            reasoning_output_tokens: self
                .completion_tokens_details
                .as_ref()
                .map_or(0, |details| details.reasoning_tokens),
            total_tokens: self.total_tokens,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChatPromptTokenDetails {
    #[serde(default)]
    cached_tokens: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChatCompletionTokenDetails {
    #[serde(default)]
    reasoning_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct ChatErrorEnvelope {
    error: ChatError,
}

#[derive(Debug, Deserialize)]
struct ChatError {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    code: Option<Value>,
    message: String,
}

async fn process_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_call_info: HashMap<String, ChatToolCallInfo>,
) {
    let mut stream = stream.eventsource();
    let mut state = ChatStreamState::new(tool_call_info);
    let mut progress = ProgressDeadline::new(idle_timeout);

    loop {
        let start = Instant::now();
        let response = timeout(progress.remaining(), stream.next()).await;
        if let Some(telemetry) = telemetry.as_ref() {
            telemetry.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(error))) => {
                let _ = tx_event
                    .send(Err(provider_stream_error(
                        ProviderStreamErrorKind::TransportError,
                        error.to_string(),
                    )))
                    .await;
                return;
            }
            Ok(None) if state.finish_reason.is_some() => {
                if let Err(error) = state.finish(&tx_event).await {
                    let _ = tx_event.send(Err(error)).await;
                }
                return;
            }
            Ok(None) => {
                let kind = if state.response_id.is_some() {
                    ProviderStreamErrorKind::ClosedAfterMessageStartBeforeStop
                } else {
                    ProviderStreamErrorKind::ClosedBeforeMessageStart
                };
                let _ = tx_event
                    .send(Err(provider_stream_error(
                        kind,
                        "Chat stream closed before a terminal finish reason",
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(provider_stream_error(
                        ProviderStreamErrorKind::IdleTimeout,
                        "idle timeout waiting for meaningful Chat SSE content",
                    )))
                    .await;
                return;
            }
        };

        let data = sse.data.trim();
        if data == "[DONE]" {
            progress.mark_progress();
            if let Err(error) = state.finish(&tx_event).await {
                let _ = tx_event.send(Err(error)).await;
            }
            return;
        }
        if data.is_empty() || matches!(data, "ping" | "keepalive") {
            continue;
        }

        let value: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(error) => {
                let _ = tx_event
                    .send(Err(provider_stream_error(
                        ProviderStreamErrorKind::ParseError,
                        format!("failed to parse Chat SSE JSON: {error}"),
                    )))
                    .await;
                return;
            }
        };
        if let Ok(envelope) = serde_json::from_value::<ChatErrorEnvelope>(value.clone()) {
            progress.mark_progress();
            let ChatError {
                r#type,
                code,
                message,
            } = envelope.error;
            let prefix = r#type
                .or_else(|| code.map(|code| code.to_string()))
                .map_or_else(String::new, |kind| format!("{kind}: "));
            let _ = tx_event
                .send(Err(provider_stream_error(
                    ProviderStreamErrorKind::ProviderError,
                    format!("{prefix}{message}"),
                )))
                .await;
            return;
        }

        let chunk: ChatCompletionChunk = match serde_json::from_value(value) {
            Ok(chunk) => chunk,
            Err(error) => {
                debug!(%error, "failed to parse Chat completion chunk");
                let _ = tx_event
                    .send(Err(provider_stream_error(
                        ProviderStreamErrorKind::ParseError,
                        format!("failed to parse Chat completion chunk: {error}"),
                    )))
                    .await;
                return;
            }
        };
        let is_meaningful = chunk.is_meaningful()
            || (state.response_id.is_none() && chunk.id.as_ref().is_some_and(|id| !id.is_empty()));
        if !is_meaningful {
            trace!("ignoring non-meaningful Chat SSE event");
        } else {
            progress.mark_progress();
        }
        if let Err(error) = state.handle_chunk(chunk, &tx_event).await {
            let _ = tx_event.send(Err(error)).await;
            return;
        }
    }
}

fn provider_stream_error(kind: ProviderStreamErrorKind, message: impl Into<String>) -> ApiError {
    ApiError::StreamFailure {
        kind,
        message: message.into(),
    }
}

fn value_has_text(value: &Value) -> bool {
    text_from_value(value).is_some_and(|text| !text.is_empty())
}

fn text_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("content"))
            .and_then(text_from_value),
        Value::Array(items) => {
            let text = items.iter().filter_map(text_from_value).collect::<String>();
            (!text.is_empty()).then_some(text)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
