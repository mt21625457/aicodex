use crate::common::ClaudeToolCallInfo;
use crate::common::ClaudeToolCallKind;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::error::ProviderMediaErrorKind;
use crate::error::ProviderStreamErrorKind;
use crate::rate_limits::parse_all_rate_limits;
use crate::sse::progress::ProgressDeadline;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::ResponseItemId;
use codex_protocol::models::Citation;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

const REQUEST_ID_HEADER: &str = "x-request-id";
const INVALID_CLAUDE_CUSTOM_TOOL_INPUT_STATUS_PREFIX: &str = "invalid_claude_custom_tool_input: ";
const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";
const APPLY_PATCH_COMPAT_INPUT_FIELDS: &[&str] = &["patch", "body", "content", "command"];
const COLLABORATION_TOOL_NAMES: &[&str] = &[
    "spawn_agent",
    "send_message",
    "followup_task",
    "wait_agent",
    "interrupt_agent",
    "list_agents",
    "send_input",
    "resume_agent",
    "close_agent",
];
const COLLABORATION_TOOL_NAMESPACES: &[&str] = &["agents", "multi_agent", "multi_agent_v1"];

pub fn spawn_claude_response_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
) -> ResponseStream {
    let rate_limit_snapshots = parse_all_rate_limits(&stream_response.headers);
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        for snapshot in rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        process_sse(
            stream_response.bytes,
            tx_event,
            idle_timeout,
            telemetry,
            tool_call_info,
        )
        .await;
    });
    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeStreamEvent {
    MessageStart {
        message: ClaudeMessageStart,
    },
    ContentBlockStart {
        index: usize,
        content_block: ClaudeStreamContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: ClaudeStreamDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        #[serde(default)]
        delta: Option<ClaudeMessageDelta>,
        #[serde(default)]
        usage: Option<ClaudeUsage>,
    },
    MessageStop,
    Error {
        #[serde(default)]
        error: Option<ClaudeError>,
    },
    #[serde(other)]
    Unknown,
}

impl ClaudeStreamEvent {
    fn event_name_only(kind: &str) -> Self {
        match kind {
            "message_stop" => Self::MessageStop,
            "error" => Self::Error { error: None },
            _ => Self::Unknown,
        }
    }

    fn is_meaningful(&self) -> bool {
        match self {
            Self::Unknown => false,
            Self::ContentBlockDelta { delta, .. } => delta.is_meaningful(),
            Self::MessageDelta { delta, usage } => {
                delta
                    .as_ref()
                    .is_some_and(ClaudeMessageDelta::is_meaningful)
                    || usage.as_ref().is_some_and(ClaudeUsage::is_meaningful)
            }
            Self::MessageStart { .. }
            | Self::ContentBlockStart { .. }
            | Self::ContentBlockStop { .. }
            | Self::MessageStop
            | Self::Error { .. } => true,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageStart {
    id: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageDelta {
    #[serde(default)]
    stop_reason: Option<ClaudeStopReason>,
    #[serde(default)]
    stop_sequence: Option<String>,
}

impl ClaudeMessageDelta {
    fn is_meaningful(&self) -> bool {
        self.stop_reason.is_some()
            || self
                .stop_sequence
                .as_ref()
                .is_some_and(|sequence| !sequence.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeStopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
    ModelContextWindowExceeded,
    Unknown(String),
}

impl<'de> Deserialize<'de> for ClaudeStopReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "end_turn" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "stop_sequence" => Self::StopSequence,
            "tool_use" => Self::ToolUse,
            "pause_turn" => Self::PauseTurn,
            "refusal" => Self::Refusal,
            "model_context_window_exceeded" => Self::ModelContextWindowExceeded,
            _ => Self::Unknown(value),
        })
    }
}

impl ClaudeStopReason {
    fn as_str(&self) -> &str {
        match self {
            Self::EndTurn => "end_turn",
            Self::MaxTokens => "max_tokens",
            Self::StopSequence => "stop_sequence",
            Self::ToolUse => "tool_use",
            Self::PauseTurn => "pause_turn",
            Self::Refusal => "refusal",
            Self::ModelContextWindowExceeded => "model_context_window_exceeded",
            Self::Unknown(value) => value,
        }
    }

    fn end_turn(&self) -> Option<bool> {
        match self {
            Self::EndTurn | Self::MaxTokens | Self::StopSequence | Self::Refusal => Some(true),
            Self::ToolUse | Self::PauseTurn => Some(false),
            Self::ModelContextWindowExceeded | Self::Unknown(_) => None,
        }
    }
}

#[derive(Debug)]
enum ClaudeStreamContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ServerToolUse {
        id: String,
        name: String,
        input: Value,
        value: Value,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    Unknown {
        kind: String,
        value: Value,
    },
}

impl<'de> Deserialize<'de> for ClaudeStreamContentBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let Some(object) = value.as_object() else {
            return Err(serde::de::Error::custom(
                "Claude stream content block must be an object",
            ));
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match kind {
            "text" => Ok(Self::Text {
                text: object
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            "tool_use" => Ok(Self::ToolUse {
                id: object
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: object
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: object
                    .get("input")
                    .cloned()
                    .unwrap_or_else(empty_json_object),
            }),
            "server_tool_use" => Ok(Self::ServerToolUse {
                id: object
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: object
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: object
                    .get("input")
                    .cloned()
                    .unwrap_or_else(empty_json_object),
                value,
            }),
            "thinking" => Ok(Self::Thinking {
                thinking: object
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                signature: object
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            _ => Ok(Self::Unknown {
                kind: kind.to_string(),
                value,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeStreamDelta {
    TextDelta {
        #[serde(default)]
        text: String,
    },
    InputJsonDelta {
        #[serde(default)]
        partial_json: String,
    },
    ThinkingDelta {
        #[serde(default)]
        thinking: String,
    },
    SignatureDelta {
        #[serde(default)]
        signature: String,
    },
    CitationsDelta {
        citation: Value,
    },
    #[serde(other)]
    Unknown,
}

impl ClaudeStreamDelta {
    fn is_meaningful(&self) -> bool {
        match self {
            Self::TextDelta { text } => !text.is_empty(),
            Self::InputJsonDelta { partial_json } => !partial_json.is_empty(),
            Self::ThinkingDelta { thinking } => !thinking.is_empty(),
            Self::SignatureDelta { signature } => !signature.is_empty(),
            Self::CitationsDelta { citation } => !citation.is_null(),
            Self::Unknown => false,
        }
    }
}

fn empty_json_object() -> Value {
    Value::Object(Map::new())
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: Option<i64>,
    #[serde(default)]
    output_tokens: Option<i64>,
    #[serde(default)]
    cache_read_input_tokens: Option<i64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<i64>,
    #[serde(default)]
    server_tool_use: Option<Value>,
    #[serde(default)]
    iterations: Option<i64>,
}

impl ClaudeUsage {
    fn is_meaningful(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.cache_creation_input_tokens.is_some()
            || self.server_tool_use.is_some()
            || self.iterations.is_some()
    }

    fn merge(&mut self, usage: ClaudeUsage) {
        merge_non_zero_usage_field(&mut self.input_tokens, usage.input_tokens);
        self.output_tokens = usage.output_tokens.or(self.output_tokens);
        merge_non_zero_usage_field(
            &mut self.cache_read_input_tokens,
            usage.cache_read_input_tokens,
        );
        merge_non_zero_usage_field(
            &mut self.cache_creation_input_tokens,
            usage.cache_creation_input_tokens,
        );
        self.server_tool_use = usage.server_tool_use.or(self.server_tool_use.take());
        self.iterations = usage.iterations.or(self.iterations);
    }

    fn token_usage(&self) -> Option<TokenUsage> {
        let input_tokens = self.input_tokens.unwrap_or_default();
        let output_tokens = self.output_tokens.unwrap_or_default();
        let cached_input_tokens = self
            .cache_read_input_tokens
            .unwrap_or_default()
            .max(0)
            .min(input_tokens.max(0));
        let cache_write_input_tokens = self
            .cache_creation_input_tokens
            .unwrap_or_default()
            .max(0)
            .min(input_tokens.max(0));
        let total_tokens = input_tokens + output_tokens;
        if total_tokens == 0 && cached_input_tokens == 0 {
            return None;
        }
        Some(TokenUsage {
            input_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            reasoning_output_tokens: 0,
            total_tokens,
        })
    }
}

fn merge_non_zero_usage_field(current: &mut Option<i64>, incoming: Option<i64>) {
    if incoming.is_some_and(|value| value > 0) {
        *current = incoming;
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeError {
    #[serde(default)]
    r#type: Option<String>,
    message: String,
}

#[derive(Default)]
struct ClaudeStreamState {
    response_id: Option<String>,
    message_id: Option<String>,
    message_started: bool,
    started_text_blocks: BTreeSet<usize>,
    started_reasoning_blocks: BTreeSet<usize>,
    finalized_blocks: BTreeSet<usize>,
    block_kinds: BTreeMap<usize, ClaudeStreamBlockKind>,
    text_blocks: BTreeMap<usize, String>,
    text_citations: BTreeMap<usize, Vec<Citation>>,
    reasoning_blocks: BTreeMap<usize, String>,
    reasoning_signatures: BTreeMap<usize, String>,
    tool_blocks: BTreeMap<usize, ToolUseState>,
    provider_state_blocks: BTreeMap<usize, Value>,
    usage: ClaudeUsage,
    stop_reason: Option<ClaudeStopReason>,
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
    tool_call_aliases: HashMap<String, ClaudeToolCallAlias>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeToolCallAlias {
    canonical_claude_name: String,
    canonical_name: String,
    canonical_namespace: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeStreamBlockKind {
    Text,
    ToolUse,
    ServerToolUse,
    Thinking,
    ProviderState,
}

#[derive(Default)]
struct ToolUseState {
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
    partial_json: String,
    streamed_custom_tool_input: String,
    item_added: bool,
}

impl ClaudeStreamState {
    fn new(tool_call_info: HashMap<String, ClaudeToolCallInfo>) -> Self {
        let compatibility = ClaudeToolCallCompatibility::new(tool_call_info);
        Self {
            tool_call_info: compatibility.tool_call_info,
            tool_call_aliases: compatibility.aliases,
            ..Self::default()
        }
    }

    async fn handle_event(
        &mut self,
        event: ClaudeStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<bool, ApiError> {
        match event {
            ClaudeStreamEvent::MessageStart { message } => {
                self.message_started = true;
                self.block_kinds.clear();
                if let Some(model) = message.model.filter(|model| !model.trim().is_empty()) {
                    tx_event
                        .send(Ok(ResponseEvent::ServerModel(model)))
                        .await
                        .map_err(|err| ApiError::Stream(err.to_string()))?;
                }
                self.response_id = Some(message.id.clone());
                self.message_id = Some(message.id);
                if let Some(usage) = message.usage {
                    self.usage.merge(usage);
                }
                tx_event
                    .send(Ok(ResponseEvent::Created))
                    .await
                    .map_err(|err| ApiError::Stream(err.to_string()))?;
            }
            ClaudeStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.handle_content_block_start(index, content_block, tx_event)
                    .await?;
            }
            ClaudeStreamEvent::ContentBlockDelta { index, delta } => {
                self.handle_content_block_delta(index, delta, tx_event)
                    .await?;
            }
            ClaudeStreamEvent::ContentBlockStop { index } => {
                self.handle_content_block_stop(index, tx_event).await?;
            }
            ClaudeStreamEvent::MessageDelta { delta, usage } => {
                self.ensure_message_started("message_delta")?;
                if let Some(ClaudeMessageDelta {
                    stop_reason,
                    stop_sequence,
                }) = delta
                {
                    trace!(?stop_reason, ?stop_sequence, "Claude message delta");
                    if let Some(stop_reason) = stop_reason {
                        if stop_reason == ClaudeStopReason::ModelContextWindowExceeded {
                            return Err(ApiError::ContextWindowExceeded);
                        }
                        self.stop_reason = Some(stop_reason);
                    }
                }
                if let Some(usage) = usage {
                    self.usage.merge(usage);
                }
            }
            ClaudeStreamEvent::MessageStop => {
                self.ensure_message_started("message_stop")?;
                self.finish(tx_event).await?;
                return Ok(true);
            }
            ClaudeStreamEvent::Error { error } => {
                let message = error
                    .map(|error| match error.r#type {
                        Some(kind) => format!("{kind}: {}", error.message),
                        None => error.message,
                    })
                    .unwrap_or_else(|| "claude stream error".to_string());
                if let Some(kind) = ProviderMediaErrorKind::classify(/*status*/ None, &message) {
                    return Err(ApiError::ProviderMedia { kind, message });
                }
                return Err(provider_stream_error(
                    ProviderStreamErrorKind::ProviderError,
                    message,
                ));
            }
            ClaudeStreamEvent::Unknown => trace!("unhandled Claude stream event"),
        }
        Ok(false)
    }

    async fn handle_content_block_start(
        &mut self,
        index: usize,
        block: ClaudeStreamContentBlock,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        self.ensure_message_started("content_block_start")?;
        match block {
            ClaudeStreamContentBlock::Text { text } => {
                self.block_kinds.insert(index, ClaudeStreamBlockKind::Text);
                if !text.is_empty() {
                    self.ensure_text_item_started(index, tx_event).await?;
                    self.push_text_delta(index, &text, tx_event).await?;
                }
            }
            ClaudeStreamContentBlock::ToolUse { id, name, input } => {
                self.block_kinds
                    .insert(index, ClaudeStreamBlockKind::ToolUse);
                self.log_tool_call_alias_if_needed(&name);
                let state = self.tool_blocks.entry(index).or_default();
                state.id = Some(id);
                state.name = Some(name);
                state.input = Some(input);
                self.maybe_send_custom_tool_item_added(index, tx_event)
                    .await?;
                self.maybe_send_custom_tool_input_delta(index, tx_event)
                    .await?;
            }
            ClaudeStreamContentBlock::ServerToolUse {
                id,
                name,
                input,
                value,
            } => {
                self.block_kinds
                    .insert(index, ClaudeStreamBlockKind::ServerToolUse);
                self.log_tool_call_alias_if_needed(&name);
                let state = self.tool_blocks.entry(index).or_default();
                state.id = Some(id);
                state.name = Some(name);
                state.input = Some(input);
                self.provider_state_blocks.insert(index, value);
            }
            ClaudeStreamContentBlock::Thinking {
                thinking,
                signature,
            } => {
                self.block_kinds
                    .insert(index, ClaudeStreamBlockKind::Thinking);
                self.ensure_reasoning_item_started(index, tx_event).await?;
                if !thinking.is_empty() {
                    self.push_reasoning_delta(index, &thinking, tx_event)
                        .await?;
                }
                if let Some(signature) = signature
                    && !signature.is_empty()
                {
                    self.reasoning_signatures
                        .entry(index)
                        .or_default()
                        .push_str(&signature);
                }
            }
            ClaudeStreamContentBlock::Unknown { kind, value } => {
                if is_provider_state_block_type(&kind) {
                    self.block_kinds
                        .insert(index, ClaudeStreamBlockKind::ProviderState);
                    self.provider_state_blocks.insert(index, value);
                } else {
                    self.block_kinds.insert(index, ClaudeStreamBlockKind::Text);
                    let placeholder = format!("[unsupported Claude content block: {kind}]");
                    self.ensure_text_item_started(index, tx_event).await?;
                    self.push_text_delta(index, &placeholder, tx_event).await?;
                }
            }
        }
        Ok(())
    }

    async fn handle_content_block_delta(
        &mut self,
        index: usize,
        delta: ClaudeStreamDelta,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        self.ensure_message_started("content_block_delta")?;
        if self.finalized_blocks.contains(&index) {
            return Err(ApiError::Stream(format!(
                "Claude content_block_delta received after content_block_stop for block {index}"
            )));
        }
        match delta {
            ClaudeStreamDelta::TextDelta { text } => {
                self.ensure_delta_matches(index, ClaudeStreamBlockKind::Text, "text_delta")?;
                if !text.is_empty() {
                    self.ensure_text_item_started(index, tx_event).await?;
                    self.push_text_delta(index, &text, tx_event).await?;
                }
            }
            ClaudeStreamDelta::InputJsonDelta { partial_json } => {
                self.ensure_input_json_delta_matches(index)?;
                let state = self.tool_blocks.entry(index).or_default();
                state.partial_json.push_str(&partial_json);
                self.maybe_send_custom_tool_input_delta(index, tx_event)
                    .await?;
            }
            ClaudeStreamDelta::ThinkingDelta { thinking } => {
                self.ensure_delta_matches(
                    index,
                    ClaudeStreamBlockKind::Thinking,
                    "thinking_delta",
                )?;
                self.ensure_reasoning_item_started(index, tx_event).await?;
                if !thinking.is_empty() {
                    self.push_reasoning_delta(index, &thinking, tx_event)
                        .await?;
                }
            }
            ClaudeStreamDelta::SignatureDelta { signature } => {
                self.ensure_delta_matches(
                    index,
                    ClaudeStreamBlockKind::Thinking,
                    "signature_delta",
                )?;
                self.reasoning_signatures
                    .entry(index)
                    .or_default()
                    .push_str(&signature);
            }
            ClaudeStreamDelta::CitationsDelta { citation } => {
                self.ensure_delta_matches(index, ClaudeStreamBlockKind::Text, "citations_delta")?;
                if let Some(citation) = citation_from_claude(&citation) {
                    self.text_citations.entry(index).or_default().push(citation);
                }
                if let Some(marker) = citation_marker(&citation) {
                    self.ensure_text_item_started(index, tx_event).await?;
                    self.push_text_delta(index, &marker, tx_event).await?;
                }
            }
            ClaudeStreamDelta::Unknown => {}
        }
        Ok(())
    }

    async fn handle_content_block_stop(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        self.ensure_message_started("content_block_stop")?;
        if let Some(state) = self.tool_blocks.get_mut(&index)
            && !state.partial_json.trim().is_empty()
        {
            let value =
                parse_partial_tool_input_json(index, state.name.as_deref(), &state.partial_json)?;
            state.input = Some(value);
            state.partial_json.clear();
        };
        self.maybe_send_custom_tool_input_delta(index, tx_event)
            .await?;
        self.update_server_tool_use_provider_state(index);

        self.emit_final_block(index, tx_event).await
    }

    fn ensure_message_started(&self, event_name: &str) -> Result<(), ApiError> {
        if self.message_started {
            return Ok(());
        }
        Err(ApiError::Stream(format!(
            "Claude {event_name} received before message_start"
        )))
    }

    fn ensure_delta_matches(
        &self,
        index: usize,
        expected: ClaudeStreamBlockKind,
        delta_name: &str,
    ) -> Result<(), ApiError> {
        let Some(actual) = self.block_kinds.get(&index).copied() else {
            return Err(ApiError::Stream(format!(
                "Claude {delta_name} received for unknown content block {index}"
            )));
        };
        if actual == expected {
            return Ok(());
        }
        Err(ApiError::Stream(format!(
            "Claude {delta_name} does not match content block {index} kind {actual:?}"
        )))
    }

    fn ensure_input_json_delta_matches(&self, index: usize) -> Result<(), ApiError> {
        let Some(actual) = self.block_kinds.get(&index).copied() else {
            return Err(ApiError::Stream(format!(
                "Claude input_json_delta received for unknown content block {index}"
            )));
        };
        if matches!(
            actual,
            ClaudeStreamBlockKind::ToolUse | ClaudeStreamBlockKind::ServerToolUse
        ) {
            return Ok(());
        }
        Err(ApiError::Stream(format!(
            "Claude input_json_delta does not match content block {index} kind {actual:?}"
        )))
    }

    async fn ensure_text_item_started(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if !self.started_text_blocks.insert(index) {
            return Ok(());
        }
        tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                id: self.message_id_for_block(index),
                role: "assistant".to_string(),
                content: Vec::new(),
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            })))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn ensure_reasoning_item_started(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if !self.started_reasoning_blocks.insert(index) {
            return Ok(());
        }
        tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(
                ResponseItem::Reasoning {
                    id: Some(self.reasoning_id_for_block(index)),
                    summary: Vec::new(),
                    content: Some(Vec::new()),
                    encrypted_content: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            )))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn push_text_delta(
        &mut self,
        index: usize,
        text: &str,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        self.text_blocks.entry(index).or_default().push_str(text);
        tx_event
            .send(Ok(ResponseEvent::OutputTextDelta(text.to_string())))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn push_reasoning_delta(
        &mut self,
        index: usize,
        text: &str,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        self.reasoning_blocks
            .entry(index)
            .or_default()
            .push_str(text);
        tx_event
            .send(Ok(ResponseEvent::ReasoningContentDelta {
                delta: text.to_string(),
                content_index: index as i64,
            }))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn maybe_send_custom_tool_item_added(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        let Some(state) = self.tool_blocks.get_mut(&index) else {
            return Ok(());
        };
        if state.item_added {
            return Ok(());
        }
        let Some(name) = state.name.as_ref() else {
            return Ok(());
        };
        let Some(info) = self.tool_call_info.get(name) else {
            return Ok(());
        };
        if info.kind != ClaudeToolCallKind::Custom {
            return Ok(());
        }
        let call_id = state.id.clone().unwrap_or_else(|| name.clone());
        state.item_added = true;
        tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(
                ResponseItem::CustomToolCall {
                    id: Some(ResponseItemId::with_suffix("ctc", &call_id)),
                    status: None,
                    call_id,
                    name: info.name.clone(),
                    namespace: None,
                    input: String::new(),
                    internal_chat_message_metadata_passthrough: None,
                },
            )))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    fn log_tool_call_alias_if_needed(&self, upstream_tool_name: &str) {
        let Some(alias) = self.tool_call_aliases.get(upstream_tool_name) else {
            return;
        };
        debug!(
            upstream_tool_name,
            canonical_claude_name = %alias.canonical_claude_name,
            canonical_tool_name = %alias.canonical_name,
            canonical_namespace = alias.canonical_namespace.as_deref().unwrap_or(""),
            "Claude tool call alias resolved"
        );
    }

    async fn maybe_send_custom_tool_input_delta(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        let Some(state) = self.tool_blocks.get(&index) else {
            return Ok(());
        };
        let Some(name) = state.name.as_ref() else {
            return Ok(());
        };
        let Some(info) = self.tool_call_info.get(name) else {
            return Ok(());
        };
        if info.kind != ClaudeToolCallKind::Custom {
            return Ok(());
        }
        let call_id = state.id.clone().unwrap_or_else(|| name.clone());
        let Some(value) = state.complete_input_value_if_available() else {
            return Ok(());
        };
        let Ok(input) = custom_tool_input(&info.name, &value) else {
            return Ok(());
        };
        let Some(delta) = input.strip_prefix(&state.streamed_custom_tool_input) else {
            return Ok(());
        };
        if delta.is_empty() {
            return Ok(());
        }
        let delta = delta.to_string();
        if let Some(state) = self.tool_blocks.get_mut(&index) {
            state.streamed_custom_tool_input = input;
        }
        tx_event
            .send(Ok(ResponseEvent::ToolCallInputDelta {
                item_id: call_id.clone(),
                call_id: Some(call_id),
                delta,
            }))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn finish(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.stop_reason == Some(ClaudeStopReason::ModelContextWindowExceeded) {
            return Err(ApiError::ContextWindowExceeded);
        }

        let unfinished_blocks = self
            .block_kinds
            .keys()
            .copied()
            .filter(|index| !self.finalized_blocks.contains(index))
            .collect::<Vec<_>>();
        for index in unfinished_blocks {
            self.emit_final_block(index, tx_event).await?;
        }

        tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id: self.response_id(),
                token_usage: self.usage.token_usage(),
                end_turn: self
                    .stop_reason
                    .as_ref()
                    .and_then(ClaudeStopReason::end_turn),
                provider_stop_reason: self
                    .stop_reason
                    .as_ref()
                    .map(ClaudeStopReason::as_str)
                    .map(str::to_string),
            }))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn emit_final_block(
        &mut self,
        index: usize,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if !self.block_kinds.contains_key(&index) {
            return Ok(());
        }
        if !self.finalized_blocks.insert(index) {
            return Ok(());
        }
        let items = self.final_response_items_for_block(index)?;
        for item in items {
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(item)))
                .await
                .map_err(|err| ApiError::Stream(err.to_string()))?;
        }
        Ok(())
    }

    fn final_response_items_for_block(&self, index: usize) -> Result<Vec<ResponseItem>, ApiError> {
        let Some(kind) = self.block_kinds.get(&index) else {
            return Ok(Vec::new());
        };
        match kind {
            ClaudeStreamBlockKind::Text => {
                if let Some(text) = self.text_blocks.get(&index)
                    && !text.is_empty()
                {
                    let citations = self.text_citations.get(&index).cloned().unwrap_or_default();
                    let content = if citations.is_empty() {
                        vec![ContentItem::OutputText { text: text.clone() }]
                    } else {
                        vec![ContentItem::OutputTextWithCitations {
                            text: text.clone(),
                            citations,
                        }]
                    };
                    return Ok(vec![ResponseItem::Message {
                        id: self.message_id_for_block(index),
                        role: "assistant".to_string(),
                        content,
                        phase: None,
                        internal_chat_message_metadata_passthrough: None,
                    }]);
                }
                Ok(Vec::new())
            }
            ClaudeStreamBlockKind::Thinking => Ok(vec![self.reasoning_item_for_block(index)]),
            ClaudeStreamBlockKind::ToolUse => Ok(self
                .tool_call_item_for_block(index)?
                .into_iter()
                .collect::<Vec<_>>()),
            ClaudeStreamBlockKind::ServerToolUse => {
                let mut items = Vec::new();
                if let Some(item) = self.server_tool_call_item_for_block(index)? {
                    items.push(item);
                }
                if let Some(item) = self.provider_state_item_for_block(index) {
                    items.push(item);
                }
                Ok(items)
            }
            ClaudeStreamBlockKind::ProviderState => Ok(self
                .provider_state_item_for_block(index)
                .into_iter()
                .collect()),
        }
    }

    fn reasoning_item_for_block(&self, index: usize) -> ResponseItem {
        let text = self
            .reasoning_blocks
            .get(&index)
            .cloned()
            .unwrap_or_default();
        let signature = self
            .reasoning_signatures
            .get(&index)
            .filter(|signature| !signature.trim().is_empty())
            .cloned();
        ResponseItem::Reasoning {
            id: Some(self.reasoning_id_for_block(index)),
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText { text }]),
            encrypted_content: signature,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn tool_call_item_for_block(&self, index: usize) -> Result<Option<ResponseItem>, ApiError> {
        let Some(state) = self.tool_blocks.get(&index) else {
            return Ok(None);
        };
        let Some(claude_name) = state.name.as_ref() else {
            return Ok(None);
        };
        let call_id = state.id.clone().unwrap_or_else(|| claude_name.clone());
        let info = self
            .tool_call_info
            .get(claude_name)
            .cloned()
            .unwrap_or_else(|| ClaudeToolCallInfo {
                name: claude_name.clone(),
                namespace: None,
                kind: ClaudeToolCallKind::Function,
            });
        let input = match info.kind {
            ClaudeToolCallKind::Function | ClaudeToolCallKind::ToolSearch => {
                state.json_object_input_value(index)?
            }
            ClaudeToolCallKind::Custom => state.input_value(index)?,
        };

        Ok(Some(match info.kind {
            ClaudeToolCallKind::Function => ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", &call_id)),
                name: info.name,
                namespace: info.namespace,
                arguments: stringify_tool_input(&input),
                call_id,
                internal_chat_message_metadata_passthrough: None,
            },
            ClaudeToolCallKind::Custom => match custom_tool_input(&info.name, &input) {
                Ok(input) => ResponseItem::CustomToolCall {
                    id: Some(ResponseItemId::with_suffix("ctc", &call_id)),
                    status: None,
                    call_id,
                    name: info.name,
                    namespace: None,
                    input,
                    internal_chat_message_metadata_passthrough: None,
                },
                Err(message) => ResponseItem::CustomToolCall {
                    id: Some(ResponseItemId::with_suffix("ctc", &call_id)),
                    status: Some(format!(
                        "{INVALID_CLAUDE_CUSTOM_TOOL_INPUT_STATUS_PREFIX}{message}"
                    )),
                    call_id,
                    name: info.name,
                    namespace: None,
                    input: String::new(),
                    internal_chat_message_metadata_passthrough: None,
                },
            },
            ClaudeToolCallKind::ToolSearch => ResponseItem::ToolSearchCall {
                id: Some(ResponseItemId::with_suffix("tsc", &call_id)),
                call_id: Some(call_id),
                status: None,
                execution: "client".to_string(),
                arguments: input,
                internal_chat_message_metadata_passthrough: None,
            },
        }))
    }

    fn server_tool_call_item_for_block(
        &self,
        index: usize,
    ) -> Result<Option<ResponseItem>, ApiError> {
        let Some(state) = self.tool_blocks.get(&index) else {
            return Ok(None);
        };
        let Some(name) = state.name.as_deref() else {
            return Ok(None);
        };
        if name != "web_search" {
            return Ok(None);
        }
        let call_id = state.id.clone().unwrap_or_else(|| name.to_string());
        let input = state.json_object_input_value(index)?;
        Ok(Some(ResponseItem::WebSearchCall {
            id: Some(ResponseItemId::with_suffix("ws", call_id)),
            status: None,
            action: web_search_action_from_claude_input(&input),
            internal_chat_message_metadata_passthrough: None,
        }))
    }

    fn provider_state_item_for_block(&self, index: usize) -> Option<ResponseItem> {
        self.provider_state_blocks
            .get(&index)
            .map(|value| ResponseItem::Compaction {
                id: None,
                encrypted_content: value.to_string(),
                internal_chat_message_metadata_passthrough: None,
            })
    }

    fn update_server_tool_use_provider_state(&mut self, index: usize) {
        if self.block_kinds.get(&index) != Some(&ClaudeStreamBlockKind::ServerToolUse) {
            return;
        }
        let Some(input) = self
            .tool_blocks
            .get(&index)
            .and_then(|state| state.input.clone())
        else {
            return;
        };
        let Some(value) = self.provider_state_blocks.get_mut(&index) else {
            return;
        };
        let Some(object) = value.as_object_mut() else {
            return;
        };
        object.insert("input".to_string(), input);
    }

    fn response_id(&self) -> String {
        self.response_id
            .clone()
            .unwrap_or_else(|| "claude-response".to_string())
    }

    fn message_id_for_block(&self, index: usize) -> Option<ResponseItemId> {
        self.message_id.as_ref().map(|message_id| {
            let id = if index == 0 {
                message_id.clone()
            } else {
                format!("{message_id}_text_{index}")
            };
            ResponseItemId::from_server(id)
        })
    }

    fn reasoning_id_for_block(&self, index: usize) -> ResponseItemId {
        ResponseItemId::with_suffix("rs", format!("{}_reasoning_{index}", self.response_id()))
    }
}

#[derive(Default)]
struct ClaudeToolCallCompatibility {
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
    aliases: HashMap<String, ClaudeToolCallAlias>,
}

impl ClaudeToolCallCompatibility {
    fn new(mut tool_call_info: HashMap<String, ClaudeToolCallInfo>) -> Self {
        let originals = tool_call_info
            .iter()
            .map(|(claude_name, info)| (claude_name.clone(), info.clone()))
            .collect::<Vec<_>>();
        let mut pending_aliases: HashMap<String, Option<(String, ClaudeToolCallInfo)>> =
            HashMap::new();

        for (canonical_claude_name, info) in originals {
            for alias in claude_tool_call_aliases(&canonical_claude_name, &info) {
                if tool_call_info.contains_key(&alias) {
                    continue;
                }
                let candidate = (canonical_claude_name.clone(), info.clone());
                match pending_aliases.get_mut(&alias) {
                    Some(existing) if existing.as_ref() != Some(&candidate) => {
                        *existing = None;
                    }
                    Some(_) => {}
                    None => {
                        pending_aliases.insert(alias, Some(candidate));
                    }
                }
            }
        }

        let mut aliases = HashMap::new();
        for (alias, target) in pending_aliases {
            if let Some((canonical_claude_name, info)) = target {
                tool_call_info.insert(alias.clone(), info.clone());
                aliases.insert(
                    alias,
                    ClaudeToolCallAlias {
                        canonical_claude_name,
                        canonical_name: info.name,
                        canonical_namespace: info.namespace,
                    },
                );
            }
        }

        Self {
            tool_call_info,
            aliases,
        }
    }
}

fn claude_tool_call_aliases(canonical_claude_name: &str, info: &ClaudeToolCallInfo) -> Vec<String> {
    let mut aliases = vec![format!("functions.{canonical_claude_name}")];
    if let Some(namespace) = info.namespace.as_deref() {
        aliases.extend(namespace_tool_aliases(namespace, &info.name));
    } else if COLLABORATION_TOOL_NAMES.contains(&info.name.as_str()) {
        for namespace in COLLABORATION_TOOL_NAMESPACES {
            aliases.extend(namespace_tool_aliases(namespace, &info.name));
        }
    }

    aliases.sort();
    aliases.dedup();
    aliases
        .into_iter()
        .filter(|alias| alias != canonical_claude_name)
        .collect()
}

fn namespace_tool_aliases(namespace: &str, name: &str) -> Vec<String> {
    let mut aliases = vec![
        claude_namespace_tool_name(namespace, name),
        format!("{namespace}.{name}"),
        format!("functions.{namespace}.{name}"),
    ];
    if !namespace.ends_with('_') && !name.starts_with('_') {
        aliases.push(format!("{namespace}__{name}"));
        aliases.push(format!("functions.{namespace}__{name}"));
    }
    if COLLABORATION_TOOL_NAMESPACES.contains(&namespace)
        && COLLABORATION_TOOL_NAMES.contains(&name)
    {
        aliases.push(name.to_string());
    }
    aliases
}

fn claude_namespace_tool_name(namespace: &str, name: &str) -> String {
    if namespace.ends_with('_')
        || name.starts_with('_')
        || namespace
            .chars()
            .last()
            .is_some_and(|ch| !ch.is_ascii_alphanumeric())
    {
        format!("{namespace}{name}")
    } else {
        format!("{namespace}_{name}")
    }
}

impl ToolUseState {
    fn input_value(&self, index: usize) -> Result<Value, ApiError> {
        if !self.partial_json.trim().is_empty() {
            return parse_partial_tool_input_json(index, self.name.as_deref(), &self.partial_json);
        }
        Ok(self
            .input
            .clone()
            .unwrap_or_else(|| Value::Object(Map::new())))
    }

    fn json_object_input_value(&self, index: usize) -> Result<Value, ApiError> {
        match self.input_value(index)? {
            Value::Null => Ok(Value::Object(Map::new())),
            Value::String(text) if text.trim().is_empty() => Ok(Value::Object(Map::new())),
            value => Ok(value),
        }
    }

    fn complete_input_value_if_available(&self) -> Option<Value> {
        if !self.partial_json.trim().is_empty() {
            return serde_json::from_str::<Value>(&self.partial_json).ok();
        }
        self.input.clone()
    }
}

fn parse_partial_tool_input_json(
    index: usize,
    tool_name: Option<&str>,
    partial_json: &str,
) -> Result<Value, ApiError> {
    serde_json::from_str::<Value>(partial_json).map_err(|err| {
        let tool_name = tool_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("<unknown>");
        ApiError::MalformedResponse {
            message: format!(
                "invalid Claude tool input JSON for content block {index} \
             (tool `{tool_name}`, input length {} bytes): {err}",
                partial_json.len()
            ),
        }
    })
}

fn stringify_tool_input(input: &Value) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string())
}

fn web_search_action_from_claude_input(input: &Value) -> Option<WebSearchAction> {
    let object = input.as_object()?;
    if let Some(query) = object
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        return Some(WebSearchAction::Search {
            query: Some(query.to_string()),
            queries: None,
        });
    }

    let queries = object
        .get("queries")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|queries| !queries.is_empty());
    queries.map(|queries| WebSearchAction::Search {
        query: queries.first().cloned(),
        queries: Some(queries),
    })
}

fn citation_marker(citation: &Value) -> Option<String> {
    let object = citation.as_object()?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty())
        .unwrap_or("citation");
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(clean_citation_part);
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(clean_citation_part);

    match (title, url) {
        (Some(title), Some(url)) => Some(format!(" [source: {title} ({url})]")),
        (Some(title), None) => Some(format!(" [source: {title}]")),
        (None, Some(url)) => Some(format!(" [source: {url}]")),
        (None, None) if kind != "citation" => Some(format!(" [source: {kind}]")),
        (None, None) => None,
    }
}

fn citation_from_claude(citation: &Value) -> Option<Citation> {
    let object = citation.as_object()?;
    let citation_type = object
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty())
        .unwrap_or("citation")
        .to_string();
    Some(Citation {
        provider: "claude".to_string(),
        citation_type,
        title: object
            .get("title")
            .and_then(Value::as_str)
            .map(clean_citation_part),
        url: object
            .get("url")
            .and_then(Value::as_str)
            .map(clean_citation_part),
        cited_text: object
            .get("cited_text")
            .and_then(Value::as_str)
            .map(str::to_string),
        encrypted_index: object
            .get("encrypted_index")
            .and_then(Value::as_str)
            .map(str::to_string),
        document_index: object.get("document_index").and_then(Value::as_i64),
        start_page_number: object.get("start_page_number").and_then(Value::as_i64),
        end_page_number: object.get("end_page_number").and_then(Value::as_i64),
        start_char_index: object.get("start_char_index").and_then(Value::as_i64),
        end_char_index: object.get("end_char_index").and_then(Value::as_i64),
        raw: Some(citation.clone()),
    })
}

fn clean_citation_part(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn custom_tool_input(tool_name: &str, input: &Value) -> Result<String, String> {
    match input {
        Value::String(input) => Ok(input.clone()),
        Value::Object(object) => custom_tool_input_from_object(tool_name, object),
        _ => Err(
            "Claude custom/freeform tool input must be either a raw string or an object with an `input` string containing the raw tool body"
                .to_string(),
        ),
    }
}

fn custom_tool_input_from_object(
    tool_name: &str,
    object: &Map<String, Value>,
) -> Result<String, String> {
    match object.get("input") {
        Some(Value::String(input)) => return Ok(input.clone()),
        Some(_) => {
            return Err(
                "Claude custom/freeform tool field `input` must be a string containing the raw tool body"
                    .to_string(),
            );
        }
        None => {}
    }

    if tool_name == APPLY_PATCH_TOOL_NAME {
        return apply_patch_compat_tool_input(object).ok_or_else(|| {
            "Claude custom/freeform tool calls must include an `input` string containing the raw tool body"
                .to_string()
        });
    }

    Err(
        "Claude custom/freeform tool calls must include an `input` string containing the raw tool body"
            .to_string(),
    )
}

fn apply_patch_compat_tool_input(object: &Map<String, Value>) -> Option<String> {
    for field in APPLY_PATCH_COMPAT_INPUT_FIELDS {
        let Some(Value::String(value)) = object.get(*field) else {
            continue;
        };
        if looks_like_apply_patch(value) {
            return Some(value.clone());
        }
    }

    let mut patch_like_values = object
        .values()
        .filter_map(Value::as_str)
        .filter(|value| looks_like_apply_patch(value));
    let first = patch_like_values.next()?;
    if patch_like_values.next().is_none() {
        Some(first.to_string())
    } else {
        None
    }
}

fn looks_like_apply_patch(input: &str) -> bool {
    input.trim_start().starts_with("*** Begin Patch")
}

fn is_provider_state_block_type(kind: &str) -> bool {
    kind == "compaction"
        || kind == "redacted_thinking"
        || kind == "server_tool_use"
        || kind == "tool_reference"
        || kind.ends_with("_tool_use")
        || kind.ends_with("_tool_result")
}

fn parse_claude_stream_event(event_name: &str, data: &str) -> Result<ClaudeStreamEvent, ApiError> {
    if data.trim().is_empty() && !event_name.trim().is_empty() {
        return Ok(ClaudeStreamEvent::event_name_only(event_name));
    }

    serde_json::from_str(data).map_err(|err| {
        provider_stream_error(
            ProviderStreamErrorKind::ParseError,
            format!("failed to parse Claude SSE event: {err}"),
        )
    })
}

fn provider_stream_error(kind: ProviderStreamErrorKind, message: String) -> ApiError {
    debug!(%kind, "Claude stream failure classified");
    ApiError::StreamFailure { kind, message }
}

fn stream_closed_error(state: &ClaudeStreamState) -> ApiError {
    if state.message_started {
        provider_stream_error(
            ProviderStreamErrorKind::ClosedAfterMessageStartBeforeStop,
            "stream closed after message_start before message_stop".to_string(),
        )
    } else {
        provider_stream_error(
            ProviderStreamErrorKind::ClosedBeforeMessageStart,
            "stream closed before message_start".to_string(),
        )
    }
}

fn idle_timeout_error() -> ApiError {
    provider_stream_error(
        ProviderStreamErrorKind::IdleTimeout,
        "idle timeout waiting for SSE".to_string(),
    )
}

async fn process_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
) {
    let mut stream = stream.eventsource();
    let mut state = ClaudeStreamState::new(tool_call_info);
    let mut progress = ProgressDeadline::new(idle_timeout);

    loop {
        let start = Instant::now();
        let response = timeout(progress.remaining(), stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("Claude SSE error: {e:#}");
                let _ = tx_event
                    .send(Err(provider_stream_error(
                        ProviderStreamErrorKind::TransportError,
                        e.to_string(),
                    )))
                    .await;
                return;
            }
            Ok(None) => {
                let _ = tx_event.send(Err(stream_closed_error(&state))).await;
                return;
            }
            Err(_) => {
                let _ = tx_event.send(Err(idle_timeout_error())).await;
                return;
            }
        };

        trace!(
            event = %sse.event,
            data_len = sse.data.len(),
            "Claude SSE event received"
        );
        let event: ClaudeStreamEvent = match parse_claude_stream_event(&sse.event, &sse.data) {
            Ok(event) => event,
            Err(e) => {
                debug!(
                    event = %sse.event,
                    data_len = sse.data.len(),
                    error = %e,
                    "failed to parse Claude SSE event"
                );
                let _ = tx_event.send(Err(e)).await;
                return;
            }
        };
        if event.is_meaningful() {
            progress.mark_progress();
        }

        match state.handle_event(event, &tx_event).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                let _ = tx_event.send(Err(error)).await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use bytes::Bytes;
    use codex_client::TransportError;
    use futures::StreamExt;
    use futures::stream;
    use http::HeaderMap;
    use http::HeaderValue;
    use http::StatusCode;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    async fn run_events(
        events: Vec<Value>,
        tool_call_info: HashMap<String, ClaudeToolCallInfo>,
    ) -> Vec<ResponseEvent> {
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
        let mut state = ClaudeStreamState::new(tool_call_info);
        for event in events {
            let event = serde_json::from_value::<ClaudeStreamEvent>(event).expect("event parses");
            if state.handle_event(event, &tx).await.expect("event handled") {
                break;
            }
        }
        drop(tx);
        let mut output = Vec::new();
        while let Some(event) = rx.recv().await {
            output.push(event.expect("event ok"));
        }
        output
    }

    async fn run_events_expect_error(events: Vec<Value>) -> ApiError {
        let (tx, _rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(16);
        let mut state = ClaudeStreamState::new(HashMap::new());
        for event in events {
            let event = serde_json::from_value::<ClaudeStreamEvent>(event).expect("event parses");
            if let Err(err) = state.handle_event(event, &tx).await {
                return err;
            }
        }
        panic!("expected Claude stream event error")
    }

    fn stream_response(chunks: Vec<Result<Bytes, TransportError>>) -> codex_client::StreamResponse {
        codex_client::StreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            bytes: Box::pin(stream::iter(chunks)),
        }
    }

    fn stream_response_with_headers(
        chunks: Vec<Result<Bytes, TransportError>>,
        headers: HeaderMap,
    ) -> codex_client::StreamResponse {
        codex_client::StreamResponse {
            status: StatusCode::OK,
            headers,
            bytes: Box::pin(stream::iter(chunks)),
        }
    }

    async fn recv_stream_error(mut stream: ResponseStream) -> ApiError {
        while let Some(event) = stream.rx_event.recv().await {
            if let Err(error) = event {
                return error;
            }
        }
        panic!("expected stream error")
    }

    fn custom_apply_patch_tool_call_info() -> HashMap<String, ClaudeToolCallInfo> {
        HashMap::from([(
            "apply_patch".to_string(),
            ClaudeToolCallInfo {
                name: "apply_patch".to_string(),
                namespace: None,
                kind: ClaudeToolCallKind::Custom,
            },
        )])
    }

    fn sample_apply_patch() -> &'static str {
        "*** Begin Patch\n*** Add File: claude.txt\n+hello\n*** End Patch"
    }

    fn custom_tool_done_input(events: &[ResponseEvent], tool_name: &str) -> Option<String> {
        events.iter().find_map(|event| match event {
            ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall { name, input, .. })
                if name == tool_name =>
            {
                Some(input.clone())
            }
            _ => None,
        })
    }

    fn custom_tool_input_deltas(events: &[ResponseEvent], call_id: &str) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::ToolCallInputDelta {
                    call_id: Some(event_call_id),
                    delta,
                    ..
                } if event_call_id == call_id => Some(delta.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn claude_stream_parses_event_name_only_sse() {
        let event = parse_claude_stream_event("message_stop", "").expect("event parses");
        assert!(matches!(event, ClaudeStreamEvent::MessageStop));
    }

    #[tokio::test]
    async fn claude_stream_classifies_close_before_message_start() {
        let stream = spawn_claude_response_stream(
            stream_response(Vec::new()),
            Duration::from_secs(1),
            /*telemetry*/ None,
            HashMap::new(),
        );

        let ApiError::StreamFailure { kind, message } = recv_stream_error(stream).await else {
            panic!("expected stream failure");
        };
        assert_eq!(kind, ProviderStreamErrorKind::ClosedBeforeMessageStart);
        assert_eq!(message, "stream closed before message_start");
    }

    #[tokio::test]
    async fn claude_stream_classifies_close_after_message_start() {
        let start = json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": []
            }
        });
        let stream = spawn_claude_response_stream(
            stream_response(vec![Ok(Bytes::from(format!(
                "event: message_start\ndata: {start}\n\n"
            )))]),
            Duration::from_secs(1),
            /*telemetry*/ None,
            HashMap::new(),
        );

        let ApiError::StreamFailure { kind, message } = recv_stream_error(stream).await else {
            panic!("expected stream failure");
        };
        assert_eq!(
            kind,
            ProviderStreamErrorKind::ClosedAfterMessageStartBeforeStop
        );
        assert_eq!(
            message,
            "stream closed after message_start before message_stop"
        );
    }

    #[tokio::test]
    async fn claude_stream_classifies_parse_error() {
        let stream = spawn_claude_response_stream(
            stream_response(vec![Ok(Bytes::from("event: message_start\ndata: {\n\n"))]),
            Duration::from_secs(1),
            /*telemetry*/ None,
            HashMap::new(),
        );

        let ApiError::StreamFailure { kind, message } = recv_stream_error(stream).await else {
            panic!("expected stream failure");
        };
        assert_eq!(kind, ProviderStreamErrorKind::ParseError);
        assert!(message.contains("failed to parse Claude SSE event"));
    }

    #[tokio::test]
    async fn claude_stream_classifies_idle_timeout() {
        let stream_response = codex_client::StreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            bytes: Box::pin(stream::pending::<Result<Bytes, TransportError>>()),
        };
        let stream = spawn_claude_response_stream(
            stream_response,
            Duration::from_millis(1),
            /*telemetry*/ None,
            HashMap::new(),
        );

        let ApiError::StreamFailure { kind, message } = recv_stream_error(stream).await else {
            panic!("expected stream failure");
        };
        assert_eq!(kind, ProviderStreamErrorKind::IdleTimeout);
        assert_eq!(message, "idle timeout waiting for SSE");
    }

    #[tokio::test(start_paused = true)]
    async fn claude_unknown_events_do_not_extend_idle_deadline() {
        let bytes = stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok(Bytes::from_static(
                    b"event: ping\ndata: {\"type\":\"ping\"}\n\n",
                )),
                (),
            ))
        });
        let stream = spawn_claude_response_stream(
            codex_client::StreamResponse {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                bytes: Box::pin(bytes),
            },
            Duration::from_millis(50),
            /*telemetry*/ None,
            HashMap::new(),
        );

        let ApiError::StreamFailure { kind, message } = recv_stream_error(stream).await else {
            panic!("expected stream failure");
        };
        assert_eq!(kind, ProviderStreamErrorKind::IdleTimeout);
        assert_eq!(message, "idle timeout waiting for SSE");
    }

    #[tokio::test(start_paused = true)]
    async fn claude_empty_known_deltas_do_not_extend_idle_deadline() {
        let start = Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        );
        let empty_deltas = stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok(Bytes::from_static(
                    b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"\"}}\n\n",
                )),
                (),
            ))
        });
        let bytes = stream::once(async { Ok(start) }).chain(empty_deltas);
        let stream = spawn_claude_response_stream(
            codex_client::StreamResponse {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                bytes: Box::pin(bytes),
            },
            Duration::from_millis(50),
            /*telemetry*/ None,
            HashMap::new(),
        );

        let ApiError::StreamFailure { kind, message } = recv_stream_error(stream).await else {
            panic!("expected stream failure");
        };
        assert_eq!(kind, ProviderStreamErrorKind::IdleTimeout);
        assert_eq!(message, "idle timeout waiting for SSE");
    }

    #[tokio::test(start_paused = true)]
    async fn claude_meaningful_events_extend_idle_deadline() {
        let chunks = [
            Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_progress\",\"content\":[]}}\n\n",
            ),
            Bytes::from_static(
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            ),
            Bytes::from_static(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
        ];
        let bytes = stream::iter(chunks).then(|chunk| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            Ok(chunk)
        });
        let stream = spawn_claude_response_stream(
            codex_client::StreamResponse {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                bytes: Box::pin(bytes),
            },
            Duration::from_millis(50),
            /*telemetry*/ None,
            HashMap::new(),
        );
        let mut rx = stream.rx_event;
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        assert_matches!(
            events.last(),
            Some(Ok(ResponseEvent::Completed { response_id, .. }))
                if response_id == "msg_progress"
        );
    }

    #[tokio::test]
    async fn spawn_claude_response_stream_emits_header_events() {
        let mut headers = HeaderMap::new();
        headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("req-claude-1"));
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("42"),
        );
        headers.insert(
            "anthropic-ratelimit-requests-limit",
            HeaderValue::from_static("100"),
        );
        headers.insert(
            "anthropic-ratelimit-requests-remaining",
            HeaderValue::from_static("60"),
        );
        let start = json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": []
            }
        });
        let stop = json!({"type": "message_stop"});
        let mut stream = spawn_claude_response_stream(
            stream_response_with_headers(
                vec![Ok(Bytes::from(format!(
                    "event: message_start\ndata: {start}\n\nevent: message_stop\ndata: {stop}\n\n"
                )))],
                headers,
            ),
            Duration::from_secs(1),
            /*telemetry*/ None,
            HashMap::new(),
        );

        assert_eq!(stream.upstream_request_id.as_deref(), Some("req-claude-1"));

        let event = stream
            .rx_event
            .recv()
            .await
            .expect("expected codex rate limit event")
            .expect("expected ok event");
        let ResponseEvent::RateLimits(snapshot) = event else {
            panic!("expected codex rate limits event");
        };
        assert_eq!(snapshot.limit_id.as_deref(), Some("codex"));
        let primary = snapshot.primary.expect("primary");
        assert_eq!(primary.used_percent, 42.0);

        let event = stream
            .rx_event
            .recv()
            .await
            .expect("expected anthropic rate limit event")
            .expect("expected ok event");
        let ResponseEvent::RateLimits(snapshot) = event else {
            panic!("expected anthropic rate limits event");
        };
        assert_eq!(snapshot.limit_id.as_deref(), Some("anthropic_requests"));
        let primary = snapshot.primary.expect("primary");
        assert_eq!(primary.used_percent, 40.0);
    }

    #[tokio::test]
    async fn claude_stream_emits_server_model_from_message_start() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_1",
                        "type": "message",
                        "role": "assistant",
                        "model": "claude-sonnet-4-5",
                        "content": []
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(matches!(
            events.as_slice(),
            [
                ResponseEvent::ServerModel(model),
                ResponseEvent::Created,
                ResponseEvent::Completed { .. }
            ] if model == "claude-sonnet-4-5"
        ));
    }

    #[tokio::test]
    async fn claude_stream_maps_text_and_tool_use() {
        let mut tool_call_info = HashMap::new();
        tool_call_info.insert(
            "mcp__demo__search".to_string(),
            ClaudeToolCallInfo {
                name: "search".to_string(),
                namespace: Some("mcp__demo__".to_string()),
                kind: ClaudeToolCallKind::Function,
            },
        );

        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "hi"}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "mcp__demo__search",
                        "input": {}
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 1,
                    "delta": {"type": "input_json_delta", "partial_json": "{\"query\":\"rust\"}"}
                }),
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use"},
                    "usage": {"input_tokens": 4, "output_tokens": 5}
                }),
                json!({"type": "message_stop"}),
            ],
            tool_call_info,
        )
        .await;

        assert!(matches!(events[0], ResponseEvent::Created));
        assert_eq!(
            events.iter().find_map(|event| match event {
                ResponseEvent::OutputTextDelta(delta) => Some(delta.as_str()),
                _ => None,
            }),
            Some("hi")
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                namespace,
                arguments,
                call_id,
                ..
            }) if name == "search"
                && namespace.as_deref() == Some("mcp__demo__")
                && arguments == "{\"query\":\"rust\"}"
                && call_id == "toolu_1"
        )));
        let end_turn = events
            .iter()
            .find_map(|event| match event {
                ResponseEvent::Completed { end_turn, .. } => Some(*end_turn),
                _ => None,
            })
            .expect("completed event");
        assert_eq!(end_turn, Some(false));
    }

    #[tokio::test]
    async fn claude_stream_maps_namespaced_multi_agent_tool_aliases() {
        for alias in [
            "multi_agent_spawn_agent",
            "multi_agent.spawn_agent",
            "multi_agent__spawn_agent",
            "functions.multi_agent.spawn_agent",
            "functions.multi_agent__spawn_agent",
            "spawn_agent",
        ] {
            let tool_call_info = HashMap::from([(
                "multi_agent_spawn_agent".to_string(),
                ClaudeToolCallInfo {
                    name: "spawn_agent".to_string(),
                    namespace: Some("multi_agent".to_string()),
                    kind: ClaudeToolCallKind::Function,
                },
            )]);

            let events = run_events(
                vec![
                    json!({
                        "type": "message_start",
                        "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                    }),
                    json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": alias,
                            "input": {}
                        }
                    }),
                    json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "input_json_delta", "partial_json": "{\"task_name\":\"review\",\"message\":\"check\"}"}
                    }),
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "tool_use"}
                    }),
                    json!({"type": "message_stop"}),
                ],
                tool_call_info,
            )
            .await;

            let mapped_call = events.iter().find_map(|event| match event {
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                    name,
                    namespace,
                    arguments,
                    call_id,
                    ..
                }) if name == "spawn_agent"
                    && namespace.as_deref() == Some("multi_agent")
                    && call_id == "toolu_1" =>
                {
                    Some(arguments)
                }
                _ => None,
            });
            let Some(arguments) = mapped_call else {
                panic!(
                    "expected alias {alias} to map to multi_agent.spawn_agent; events: {events:?}"
                );
            };
            assert_eq!(
                serde_json::from_str::<Value>(arguments).expect("arguments should be JSON"),
                json!({"task_name": "review", "message": "check"})
            );
        }
    }

    #[tokio::test]
    async fn claude_stream_maps_namespaced_alias_to_bare_multi_agent_v2_tool() {
        let tool_call_info = HashMap::from([(
            "spawn_agent".to_string(),
            ClaudeToolCallInfo {
                name: "spawn_agent".to_string(),
                namespace: None,
                kind: ClaudeToolCallKind::Function,
            },
        )]);

        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "multi_agent.spawn_agent",
                        "input": {}
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": "{\"task_name\":\"review\",\"message\":\"check\"}"}
                }),
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use"}
                }),
                json!({"type": "message_stop"}),
            ],
            tool_call_info,
        )
        .await;

        let mapped_call = events.iter().find_map(|event| match event {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                namespace,
                arguments,
                call_id,
                ..
            }) if name == "spawn_agent" && namespace.is_none() && call_id == "toolu_1" => {
                Some(arguments)
            }
            _ => None,
        });
        let Some(arguments) = mapped_call else {
            panic!("expected namespaced alias to map to bare V2 spawn_agent; events: {events:?}");
        };
        assert_eq!(
            serde_json::from_str::<Value>(arguments).expect("arguments should be JSON"),
            json!({"task_name": "review", "message": "check"})
        );
    }

    #[test]
    fn claude_tool_call_compatibility_maps_bare_collaboration_tools_to_namespaced_aliases() {
        for tool_name in COLLABORATION_TOOL_NAMES {
            let state = ClaudeStreamState::new(HashMap::from([(
                (*tool_name).to_string(),
                ClaudeToolCallInfo {
                    name: (*tool_name).to_string(),
                    namespace: None,
                    kind: ClaudeToolCallKind::Function,
                },
            )]));

            for namespace in COLLABORATION_TOOL_NAMESPACES {
                for alias in [
                    format!("{namespace}_{tool_name}"),
                    format!("{namespace}.{tool_name}"),
                    format!("{namespace}__{tool_name}"),
                    format!("functions.{namespace}.{tool_name}"),
                    format!("functions.{namespace}__{tool_name}"),
                ] {
                    let info = state
                        .tool_call_info
                        .get(&alias)
                        .unwrap_or_else(|| panic!("{alias} should map to {tool_name}"));
                    assert_eq!(info.name, *tool_name);
                    assert_eq!(info.namespace, None);

                    let alias_info = state
                        .tool_call_aliases
                        .get(&alias)
                        .unwrap_or_else(|| panic!("{alias} should be recorded as an alias"));
                    assert_eq!(alias_info.canonical_claude_name, *tool_name);
                    assert_eq!(alias_info.canonical_name, *tool_name);
                    assert_eq!(alias_info.canonical_namespace, None);
                }
            }
        }
    }

    #[test]
    fn claude_tool_call_compatibility_maps_functions_prefix_for_any_claude_tool_name() {
        let state = ClaudeStreamState::new(HashMap::from([
            (
                "apply_patch".to_string(),
                ClaudeToolCallInfo {
                    name: "apply_patch".to_string(),
                    namespace: None,
                    kind: ClaudeToolCallKind::Custom,
                },
            ),
            (
                "mcp__demo__search".to_string(),
                ClaudeToolCallInfo {
                    name: "search".to_string(),
                    namespace: Some("mcp__demo__".to_string()),
                    kind: ClaudeToolCallKind::Function,
                },
            ),
        ]));

        let apply_patch = state
            .tool_call_info
            .get("functions.apply_patch")
            .expect("functions-prefixed custom tool should map");
        assert_eq!(apply_patch.name, "apply_patch");
        assert_eq!(apply_patch.namespace, None);
        assert_eq!(apply_patch.kind, ClaudeToolCallKind::Custom);

        let mcp_search = state
            .tool_call_info
            .get("functions.mcp__demo__search")
            .expect("functions-prefixed flattened namespace tool should map");
        assert_eq!(mcp_search.name, "search");
        assert_eq!(mcp_search.namespace.as_deref(), Some("mcp__demo__"));
        assert_eq!(mcp_search.kind, ClaudeToolCallKind::Function);
    }

    #[test]
    fn claude_tool_call_compatibility_drops_ambiguous_collaboration_aliases() {
        let state = ClaudeStreamState::new(HashMap::from([
            (
                "spawn_agent".to_string(),
                ClaudeToolCallInfo {
                    name: "spawn_agent".to_string(),
                    namespace: None,
                    kind: ClaudeToolCallKind::Function,
                },
            ),
            (
                "multi_agent_spawn_agent".to_string(),
                ClaudeToolCallInfo {
                    name: "spawn_agent".to_string(),
                    namespace: Some("multi_agent".to_string()),
                    kind: ClaudeToolCallKind::Function,
                },
            ),
        ]));

        assert!(
            !state.tool_call_info.contains_key("multi_agent.spawn_agent"),
            "ambiguous dotted alias should not be auto-mapped"
        );
        assert!(
            !state
                .tool_call_info
                .contains_key("multi_agent__spawn_agent"),
            "ambiguous double-underscore alias should not be auto-mapped"
        );
        assert!(
            !state
                .tool_call_info
                .contains_key("functions.multi_agent.spawn_agent"),
            "ambiguous functions alias should not be auto-mapped"
        );
    }

    #[tokio::test]
    async fn claude_stream_does_not_bare_alias_non_collaboration_namespace() {
        let tool_call_info = HashMap::from([(
            "mcp__demo__spawn_agent".to_string(),
            ClaudeToolCallInfo {
                name: "spawn_agent".to_string(),
                namespace: Some("mcp__demo__".to_string()),
                kind: ClaudeToolCallKind::Function,
            },
        )]);

        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "spawn_agent",
                        "input": {}
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            tool_call_info,
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                namespace,
                call_id,
                ..
            }) if name == "spawn_agent" && namespace.is_none() && call_id == "toolu_1"
        )));
    }

    #[tokio::test]
    async fn claude_stream_maps_null_function_tool_input_to_empty_object() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "search",
                        "input": null
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) if name == "search" && arguments == "{}" && call_id == "toolu_1"
        )));
    }

    #[tokio::test]
    async fn claude_stream_does_not_emit_zero_usage_when_usage_is_absent() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": "Done"}
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn"},
                    "usage": {"output_tokens": 0}
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        let completed_usage = events.iter().find_map(|event| match event {
            ResponseEvent::Completed { token_usage, .. } => Some(token_usage),
            _ => None,
        });
        assert_eq!(completed_usage, Some(&None));
    }

    #[tokio::test]
    async fn claude_stream_preserves_input_cache_usage_when_delta_reports_zero() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_1",
                        "type": "message",
                        "role": "assistant",
                        "content": [],
                        "usage": {
                            "input_tokens": 100,
                            "output_tokens": 0,
                            "cache_read_input_tokens": 30,
                            "cache_creation_input_tokens": 12
                        }
                    }
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": "Done"}
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn"},
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 9,
                        "cache_read_input_tokens": 0,
                        "cache_creation_input_tokens": 0
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        let usage = events
            .iter()
            .find_map(|event| match event {
                ResponseEvent::Completed {
                    token_usage: Some(usage),
                    ..
                } => Some(usage.clone()),
                _ => None,
            })
            .expect("completed usage");

        assert_eq!(
            usage,
            TokenUsage {
                input_tokens: 100,
                cached_input_tokens: 30,
                cache_write_input_tokens: 12,
                output_tokens: 9,
                reasoning_output_tokens: 0,
                total_tokens: 109,
            }
        );
    }

    #[tokio::test]
    async fn claude_stream_accumulates_fragments_usage_cache_and_stop_metadata() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_1",
                        "type": "message",
                        "role": "assistant",
                        "content": [],
                        "usage": {
                            "input_tokens": 10,
                            "output_tokens": 0,
                            "cache_read_input_tokens": 1
                        }
                    }
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": "Hello"}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": ", "}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "world"}
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "get_weather",
                        "input": {}
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 1,
                    "delta": {"type": "input_json_delta", "partial_json": "{\"city\":"}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 1,
                    "delta": {"type": "input_json_delta", "partial_json": "\"Paris\"}"}
                }),
                json!({"type": "content_block_stop", "index": 1}),
                json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": "stop_sequence",
                        "stop_sequence": "STOP"
                    },
                    "usage": {
                        "input_tokens": 12,
                        "output_tokens": 8,
                        "cache_creation_input_tokens": 3,
                        "cache_read_input_tokens": 4
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        let text = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputTextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "Hello, world");
        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. })
                if content == &vec![ContentItem::OutputText {
                    text: "Hello, world".to_string()
                }]
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) if name == "get_weather"
                && arguments == "{\"city\":\"Paris\"}"
                && call_id == "toolu_1"
        )));

        let usage = events
            .iter()
            .find_map(|event| match event {
                ResponseEvent::Completed {
                    token_usage: Some(usage),
                    ..
                } => Some(usage.clone()),
                _ => None,
            })
            .expect("completed usage");
        assert_eq!(
            usage,
            TokenUsage {
                input_tokens: 12,
                cached_input_tokens: 4,
                cache_write_input_tokens: 3,
                output_tokens: 8,
                reasoning_output_tokens: 0,
                total_tokens: 20,
            }
        );
        let end_turn = events
            .iter()
            .find_map(|event| match event {
                ResponseEvent::Completed { end_turn, .. } => Some(*end_turn),
                _ => None,
            })
            .expect("completed event");
        assert_eq!(end_turn, Some(true));
        let provider_stop_reason = events
            .iter()
            .find_map(|event| match event {
                ResponseEvent::Completed {
                    provider_stop_reason,
                    ..
                } => provider_stop_reason.as_deref(),
                _ => None,
            })
            .expect("provider stop reason");
        assert_eq!(provider_stop_reason, "stop_sequence");
    }

    #[test]
    fn claude_usage_merge_ignores_zero_input_cache_fields() {
        let mut usage = ClaudeUsage {
            input_tokens: Some(100),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(30),
            cache_creation_input_tokens: Some(12),
            server_tool_use: Some(json!({"web_search_requests": 1})),
            iterations: Some(1),
        };

        usage.merge(ClaudeUsage {
            input_tokens: Some(0),
            output_tokens: Some(9),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
            server_tool_use: Some(json!({"web_search_requests": 2})),
            iterations: Some(2),
        });

        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(9));
        assert_eq!(usage.cache_read_input_tokens, Some(30));
        assert_eq!(usage.cache_creation_input_tokens, Some(12));
        assert_eq!(
            usage.server_tool_use,
            Some(json!({"web_search_requests": 2}))
        );
        assert_eq!(usage.iterations, Some(2));

        usage.merge(ClaudeUsage {
            input_tokens: Some(110),
            cache_read_input_tokens: Some(40),
            cache_creation_input_tokens: Some(13),
            ..ClaudeUsage::default()
        });

        assert_eq!(usage.input_tokens, Some(110));
        assert_eq!(usage.cache_read_input_tokens, Some(40));
        assert_eq!(usage.cache_creation_input_tokens, Some(13));
    }

    #[test]
    fn claude_usage_maps_cache_creation_without_cached_input() {
        let usage = ClaudeUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: Some(6),
            ..ClaudeUsage::default()
        };

        assert_eq!(
            usage.token_usage(),
            Some(TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                cache_write_input_tokens: 6,
                output_tokens: 2,
                reasoning_output_tokens: 0,
                total_tokens: 12,
            })
        );
    }

    #[test]
    fn claude_usage_clamps_cache_read_to_input_tokens() {
        let usage = ClaudeUsage {
            input_tokens: Some(3),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(10),
            cache_creation_input_tokens: Some(7),
            ..ClaudeUsage::default()
        };

        assert_eq!(
            usage.token_usage(),
            Some(TokenUsage {
                input_tokens: 3,
                cached_input_tokens: 3,
                cache_write_input_tokens: 3,
                output_tokens: 1,
                reasoning_output_tokens: 0,
                total_tokens: 4,
            })
        );
    }

    #[test]
    fn claude_usage_preserves_server_and_advisor_accounting_without_token_pollution() {
        let usage = serde_json::from_value::<ClaudeUsage>(json!({
            "input_tokens": 11,
            "output_tokens": 5,
            "server_tool_use": {"web_search_requests": 2},
            "iterations": 3
        }))
        .expect("usage parses");

        assert_eq!(
            usage.server_tool_use,
            Some(json!({"web_search_requests": 2}))
        );
        assert_eq!(usage.iterations, Some(3));
        assert_eq!(
            usage.token_usage(),
            Some(TokenUsage {
                input_tokens: 11,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 5,
                reasoning_output_tokens: 0,
                total_tokens: 16,
            })
        );
    }

    #[test]
    fn claude_stream_stop_reason_variants_deserialize() {
        let pause = serde_json::from_value::<ClaudeStreamEvent>(json!({
            "type": "message_delta",
            "delta": {"stop_reason": "pause_turn"},
            "usage": {"output_tokens": 0}
        }))
        .expect("pause_turn parses");
        assert!(matches!(
            pause,
            ClaudeStreamEvent::MessageDelta {
                delta: Some(ClaudeMessageDelta {
                    stop_reason: Some(ClaudeStopReason::PauseTurn),
                    ..
                }),
                ..
            }
        ));

        let refusal = serde_json::from_value::<ClaudeStreamEvent>(json!({
            "type": "message_delta",
            "delta": {"stop_reason": "refusal"},
            "usage": {"output_tokens": 0}
        }))
        .expect("refusal parses");
        assert!(matches!(
            refusal,
            ClaudeStreamEvent::MessageDelta {
                delta: Some(ClaudeMessageDelta {
                    stop_reason: Some(ClaudeStopReason::Refusal),
                    ..
                }),
                ..
            }
        ));

        let context_window = serde_json::from_value::<ClaudeStreamEvent>(json!({
            "type": "message_delta",
            "delta": {"stop_reason": "model_context_window_exceeded"},
            "usage": {"output_tokens": 0}
        }))
        .expect("model_context_window_exceeded parses");
        assert!(matches!(
            context_window,
            ClaudeStreamEvent::MessageDelta {
                delta: Some(ClaudeMessageDelta {
                    stop_reason: Some(ClaudeStopReason::ModelContextWindowExceeded),
                    ..
                }),
                ..
            }
        ));

        let unknown = serde_json::from_value::<ClaudeStreamEvent>(json!({
            "type": "message_delta",
            "delta": {"stop_reason": "new_future_reason"},
            "usage": {"output_tokens": 0}
        }))
        .expect("unknown stop reason parses");
        assert!(matches!(
            unknown,
            ClaudeStreamEvent::MessageDelta {
                delta: Some(ClaudeMessageDelta {
                    stop_reason: Some(ClaudeStopReason::Unknown(reason)),
                    ..
                }),
                ..
            } if reason == "new_future_reason"
        ));
    }

    #[tokio::test]
    async fn claude_stream_context_window_stop_reason_errors() {
        let error = run_events_expect_error(vec![
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "content": []
                }
            }),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "model_context_window_exceeded"},
                "usage": {"output_tokens": 0}
            }),
            json!({"type": "message_stop"}),
        ])
        .await;

        assert!(matches!(error, ApiError::ContextWindowExceeded));
    }

    #[tokio::test]
    async fn claude_stream_maps_custom_tool_input() {
        let patch = sample_apply_patch();
        let partial_json =
            serde_json::to_string(&json!({ "input": patch })).expect("serializes partial input");

        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "apply_patch",
                        "input": {}
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": partial_json}
                }),
                json!({"type": "message_stop"}),
            ],
            custom_apply_patch_tool_call_info(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemAdded(ResponseItem::CustomToolCall { name, .. })
                if name == "apply_patch"
        )));
        assert_eq!(custom_tool_input_deltas(&events, "toolu_1"), vec![patch]);
        assert_eq!(
            custom_tool_done_input(&events, "apply_patch").as_deref(),
            Some(patch)
        );
    }

    #[tokio::test]
    async fn claude_stream_maps_custom_tool_raw_string_input() {
        let patch = sample_apply_patch();
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "apply_patch",
                        "input": patch
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            custom_apply_patch_tool_call_info(),
        )
        .await;

        assert_eq!(custom_tool_input_deltas(&events, "toolu_1"), vec![patch]);
        assert_eq!(
            custom_tool_done_input(&events, "apply_patch").as_deref(),
            Some(patch)
        );
    }

    #[tokio::test]
    async fn claude_stream_maps_custom_tool_streamed_raw_json_string() {
        let patch = sample_apply_patch();
        let partial_json = serde_json::to_string(patch).expect("serializes raw string input");
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "apply_patch",
                        "input": {}
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": partial_json}
                }),
                json!({"type": "message_stop"}),
            ],
            custom_apply_patch_tool_call_info(),
        )
        .await;

        assert_eq!(custom_tool_input_deltas(&events, "toolu_1"), vec![patch]);
        assert_eq!(
            custom_tool_done_input(&events, "apply_patch").as_deref(),
            Some(patch)
        );
    }

    #[tokio::test]
    async fn claude_stream_maps_apply_patch_compat_patch_field() {
        let patch = sample_apply_patch();
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "apply_patch",
                        "input": {"patch": patch}
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            custom_apply_patch_tool_call_info(),
        )
        .await;

        assert_eq!(custom_tool_input_deltas(&events, "toolu_1"), vec![patch]);
        assert_eq!(
            custom_tool_done_input(&events, "apply_patch").as_deref(),
            Some(patch)
        );
    }

    #[tokio::test]
    async fn claude_stream_maps_apply_patch_single_patch_like_field() {
        let patch = sample_apply_patch();
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "apply_patch",
                        "input": {"command": patch}
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            custom_apply_patch_tool_call_info(),
        )
        .await;

        assert_eq!(custom_tool_input_deltas(&events, "toolu_1"), vec![patch]);
        assert_eq!(
            custom_tool_done_input(&events, "apply_patch").as_deref(),
            Some(patch)
        );
    }

    #[tokio::test]
    async fn claude_stream_marks_custom_tool_missing_input_string() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "apply_patch",
                        "input": {}
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            custom_apply_patch_tool_call_info(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall {
                call_id,
                name,
                status: Some(status),
                input,
                ..
            }) if call_id == "toolu_1"
                && name == "apply_patch"
                && input.is_empty()
                && status.starts_with(INVALID_CLAUDE_CUSTOM_TOOL_INPUT_STATUS_PREFIX)
                && status.contains("must include an `input` string")
        )));
    }

    #[tokio::test]
    async fn claude_stream_marks_custom_tool_non_string_input() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "apply_patch",
                        "input": {"input": {"not": "a string"}}
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            custom_apply_patch_tool_call_info(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall {
                call_id,
                name,
                status: Some(status),
                input,
                ..
            }) if call_id == "toolu_1"
                && name == "apply_patch"
                && input.is_empty()
                && status.starts_with(INVALID_CLAUDE_CUSTOM_TOOL_INPUT_STATUS_PREFIX)
                && status.contains("field `input` must be a string")
        )));
    }

    #[tokio::test]
    async fn claude_stream_preserves_thinking_signature() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "thinking", "thinking": ""}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "thinking_delta", "thinking": "reason"}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "signature_delta", "signature": "sig"}
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                content: Some(content),
                encrypted_content: Some(signature),
                ..
            }) if content == &vec![ReasoningItemContent::ReasoningText {
                text: "reason".to_string()
            }] && signature == "sig"
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ResponseEvent::OutputTextDelta(_)))
        );
    }

    #[tokio::test]
    async fn claude_stream_emits_reasoning_item_for_empty_thinking_start() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "thinking", "thinking": ""}
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
                id,
                summary,
                content: Some(content),
                encrypted_content: None,
                ..
            }) if id.as_deref() == Some("rs_msg_1_reasoning_0")
                && summary.is_empty()
                && content.is_empty()
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ResponseEvent::OutputTextDelta(_)))
        );
    }

    #[tokio::test]
    async fn claude_stream_keeps_thinking_delta_separate_from_delayed_text_delta() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "thinking", "thinking": ""}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "thinking_delta", "thinking": "one-line thinking"}
                }),
                json!({
                    "type": "content_block_stop",
                    "index": 0
                }),
                json!({
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {"type": "text", "text": ""}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 1,
                    "delta": {"type": "text_delta", "text": "final answer"}
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        let reasoning_deltas: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::ReasoningContentDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        let text_deltas: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputTextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(reasoning_deltas, vec!["one-line thinking"]);
        assert_eq!(text_deltas, vec!["final answer"]);
    }

    #[tokio::test]
    async fn claude_stream_maps_citations_delta_to_visible_source_marker() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": "Rust 1.90 shipped"}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "citations_delta",
                        "citation": {
                            "type": "web_search_result_location",
                            "title": "Rust Releases",
                            "url": "https://example.com/rust",
                            "encrypted_index": "enc-1",
                            "cited_text": "Rust 1.90 shipped"
                        }
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        let text = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputTextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(
            text,
            "Rust 1.90 shipped [source: Rust Releases (https://example.com/rust)]"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. })
                if matches!(
                    content.as_slice(),
                    [ContentItem::OutputTextWithCitations { text, citations }]
                        if text == "Rust 1.90 shipped [source: Rust Releases (https://example.com/rust)]"
                            && citations.len() == 1
                            && citations[0].citation_type == "web_search_result_location"
                            && citations[0].title.as_deref() == Some("Rust Releases")
                            && citations[0].url.as_deref() == Some("https://example.com/rust")
                            && citations[0].encrypted_index.as_deref() == Some("enc-1")
                            && citations[0].cited_text.as_deref() == Some("Rust 1.90 shipped")
                )
        )));
    }

    #[tokio::test]
    async fn claude_stream_preserves_document_citation_locations() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": "The contract says so"}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "citations_delta",
                        "citation": {
                            "type": "page_location",
                            "document_index": 2,
                            "start_page_number": 3,
                            "end_page_number": 4,
                            "cited_text": "contract"
                        }
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "citations_delta",
                        "citation": {
                            "type": "char_location",
                            "document_index": 2,
                            "start_char_index": 14,
                            "end_char_index": 22
                        }
                    }
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. })
                if matches!(
                    content.as_slice(),
                    [ContentItem::OutputTextWithCitations { citations, .. }]
                        if citations.len() == 2
                            && citations[0].citation_type == "page_location"
                            && citations[0].document_index == Some(2)
                            && citations[0].start_page_number == Some(3)
                            && citations[0].end_page_number == Some(4)
                            && citations[0].cited_text.as_deref() == Some("contract")
                            && citations[1].citation_type == "char_location"
                            && citations[1].document_index == Some(2)
                            && citations[1].start_char_index == Some(14)
                            && citations[1].end_char_index == Some(22)
                )
        )));
    }

    #[tokio::test]
    async fn claude_stream_preserves_provider_state_block() {
        let compaction = json!({
            "type": "compaction",
            "content": "provider summary"
        });
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": compaction.clone()
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "pause_turn"}
                }),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Compaction {
                encrypted_content, ..
            })
                if encrypted_content == &compaction.to_string()
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::Completed {
                end_turn: Some(false),
                provider_stop_reason: Some(reason),
                ..
            } if reason == "pause_turn"
        )));
    }

    #[tokio::test]
    async fn claude_stream_maps_web_search_server_tool_use_and_preserves_provider_state() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "server_tool_use",
                        "id": "srvtoolu_1",
                        "name": "web_search"
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": "{\"query\":\"latest rust release\"}"
                    }
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall {
                id: Some(id),
                action: Some(WebSearchAction::Search { query: Some(query), queries: None }),
                ..
            }) if id.as_str() == "ws_srvtoolu_1" && query == "latest rust release"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Compaction {
                encrypted_content, ..
            })
                if serde_json::from_str::<Value>(encrypted_content).ok() == Some(json!({
                    "type": "server_tool_use",
                    "id": "srvtoolu_1",
                    "name": "web_search",
                    "input": {"query": "latest rust release"}
                }))
        )));
    }

    #[tokio::test]
    async fn claude_stream_preserves_web_search_tool_result_as_provider_state() {
        let search_result = json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_1",
            "content": [{
                "type": "web_search_result",
                "title": "Rust Releases",
                "url": "https://example.com/rust",
                "encrypted_content": "opaque"
            }]
        });
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": search_result.clone()
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Compaction {
                encrypted_content, ..
            })
                if serde_json::from_str::<Value>(encrypted_content).ok().as_ref() == Some(&search_result)
        )));
    }

    #[tokio::test]
    async fn claude_stream_preserves_native_server_mcp_and_tool_search_blocks_as_provider_state() {
        let provider_blocks = vec![
            json!({
                "type": "web_fetch_tool_result",
                "tool_use_id": "srvtoolu_fetch",
                "content": [{"type": "web_fetch_result", "url": "https://example.com"}]
            }),
            json!({
                "type": "bash_code_execution_tool_result",
                "tool_use_id": "srvtoolu_code",
                "content": [{"type": "code_execution_result", "stdout": "ok"}]
            }),
            json!({
                "type": "text_editor_code_execution_tool_result",
                "tool_use_id": "srvtoolu_edit",
                "content": [{"type": "text_editor_result", "path": "/tmp/out.txt"}]
            }),
            json!({
                "type": "advisor_tool_result",
                "tool_use_id": "srvtoolu_advisor",
                "content": [{"type": "advisor_result", "text": "reviewed"}],
                "usage": {"iterations": 2}
            }),
            json!({
                "type": "mcp_tool_use",
                "id": "mcptoolu_1",
                "server_name": "docs",
                "name": "search",
                "input": {"query": "claude"}
            }),
            json!({
                "type": "mcp_tool_result",
                "tool_use_id": "mcptoolu_1",
                "content": [{"type": "text", "text": "found"}]
            }),
            json!({
                "type": "tool_search_tool_result",
                "tool_use_id": "srvtoolu_tool_search",
                "content": [{"type": "tool_reference", "name": "lookup_order"}]
            }),
            json!({
                "type": "tool_reference",
                "name": "lookup_order"
            }),
        ];
        let mut stream_events = vec![json!({
            "type": "message_start",
            "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
        })];
        for (index, block) in provider_blocks.iter().enumerate() {
            stream_events.push(json!({
                "type": "content_block_start",
                "index": index,
                "content_block": block
            }));
            stream_events.push(json!({
                "type": "content_block_stop",
                "index": index
            }));
        }
        stream_events.push(json!({"type": "message_stop"}));

        let events = run_events(stream_events, HashMap::new()).await;
        let preserved = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputItemDone(ResponseItem::Compaction {
                    encrypted_content,
                    ..
                }) => serde_json::from_str::<Value>(encrypted_content).ok(),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(preserved, provider_blocks);
    }

    #[tokio::test]
    async fn claude_stream_preserves_redacted_thinking_as_provider_state() {
        let redacted = json!({
            "type": "redacted_thinking",
            "data": "opaque-provider-state"
        });
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": redacted.clone()
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Compaction {
                encrypted_content, ..
            })
                if serde_json::from_str::<Value>(encrypted_content).ok().as_ref() == Some(&redacted)
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ResponseEvent::OutputTextDelta(_)))
        );
    }

    #[tokio::test]
    async fn claude_stream_emits_final_items_in_content_block_order() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "thinking", "thinking": "first"}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "signature_delta", "signature": "sig1"}
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "lookup",
                        "input": {"id": 1}
                    }
                }),
                json!({"type": "content_block_stop", "index": 1}),
                json!({
                    "type": "content_block_start",
                    "index": 2,
                    "content_block": {"type": "thinking", "thinking": "second"}
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 2,
                    "delta": {"type": "signature_delta", "signature": "sig2"}
                }),
                json!({"type": "content_block_stop", "index": 2}),
                json!({
                    "type": "content_block_start",
                    "index": 3,
                    "content_block": {"type": "text", "text": "done"}
                }),
                json!({"type": "content_block_stop", "index": 3}),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        let done_items = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputItemDone(item) => Some(item),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(done_items.len(), 4);
        assert!(matches!(
            done_items[0],
            ResponseItem::Reasoning {
                content: Some(content),
                encrypted_content: Some(signature),
                ..
            } if content == &vec![ReasoningItemContent::ReasoningText {
                text: "first".to_string()
            }] && signature == "sig1"
        ));
        assert!(matches!(
            done_items[1],
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } if name == "lookup" && arguments == "{\"id\":1}" && call_id == "toolu_1"
        ));
        assert!(matches!(
            done_items[2],
            ResponseItem::Reasoning {
                content: Some(content),
                encrypted_content: Some(signature),
                ..
            } if content == &vec![ReasoningItemContent::ReasoningText {
                text: "second".to_string()
            }] && signature == "sig2"
        ));
        assert!(matches!(
            done_items[3],
            ResponseItem::Message { content, .. }
                if content == &vec![ContentItem::OutputText {
                    text: "done".to_string()
                }]
        ));
    }

    #[tokio::test]
    async fn claude_stream_starts_new_text_item_after_reasoning() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": "before"}
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {"type": "thinking", "thinking": "ponder"}
                }),
                json!({"type": "content_block_stop", "index": 1}),
                json!({
                    "type": "content_block_start",
                    "index": 2,
                    "content_block": {"type": "text", "text": "after"}
                }),
                json!({"type": "content_block_stop", "index": 2}),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        let item_label = |item: &ResponseItem| match item {
            ResponseItem::Message { id, .. } => id.as_ref().map(|id| format!("message:{id}")),
            ResponseItem::Reasoning { id, .. } => id.as_ref().map(|id| format!("reasoning:{id}")),
            _ => None,
        };
        let added = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputItemAdded(item) => item_label(item),
                _ => None,
            })
            .collect::<Vec<_>>();
        let done = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputItemDone(item) => item_label(item),
                _ => None,
            })
            .collect::<Vec<_>>();
        let lifecycle = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputItemAdded(item) => {
                    item_label(item).map(|label| format!("added:{label}"))
                }
                ResponseEvent::OutputItemDone(item) => {
                    item_label(item).map(|label| format!("done:{label}"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            added,
            vec![
                "message:msg_1".to_string(),
                "reasoning:rs_msg_1_reasoning_1".to_string(),
                "message:msg_1_text_2".to_string(),
            ]
        );
        assert_eq!(done, added);
        assert_eq!(
            lifecycle,
            vec![
                "added:message:msg_1".to_string(),
                "done:message:msg_1".to_string(),
                "added:reasoning:rs_msg_1_reasoning_1".to_string(),
                "done:reasoning:rs_msg_1_reasoning_1".to_string(),
                "added:message:msg_1_text_2".to_string(),
                "done:message:msg_1_text_2".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn claude_stream_marks_unknown_user_visible_block() {
        let events = run_events(
            vec![
                json!({
                    "type": "message_start",
                    "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
                }),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "chart", "data": [1, 2, 3]}
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "message_stop"}),
            ],
            HashMap::new(),
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. })
                if content == &vec![ContentItem::OutputText {
                    text: "[unsupported Claude content block: chart]".to_string()
                }]
        )));
    }

    #[tokio::test]
    async fn claude_stream_errors_on_invalid_tool_json() {
        let error = run_events_expect_error(vec![
            json!({
                "type": "message_start",
                "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
            }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "search",
                    "input": {}
                }
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"query\":"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
        ])
        .await;

        let ApiError::MalformedResponse { message } = error else {
            panic!("expected malformed provider response");
        };
        assert!(message.contains("invalid Claude tool input JSON for content block 0"));
        assert!(message.contains("tool `search`"));
        assert!(message.contains("input length 9 bytes"));
        assert!(!message.contains("{\"query\":"));
    }

    #[tokio::test]
    async fn claude_stream_rejects_delta_before_message_start() {
        let error = run_events_expect_error(vec![json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "oops"}
        })])
        .await;

        assert!(
            error
                .to_string()
                .contains("content_block_delta received before message_start")
        );
    }

    #[tokio::test]
    async fn claude_stream_rejects_content_block_start_before_message_start() {
        let error = run_events_expect_error(vec![json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        })])
        .await;

        assert!(
            error
                .to_string()
                .contains("content_block_start received before message_start")
        );
    }

    #[tokio::test]
    async fn claude_stream_surfaces_error_event() {
        let error = run_events_expect_error(vec![json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "bad stream"
            }
        })])
        .await;

        assert!(
            error
                .to_string()
                .contains("invalid_request_error: bad stream")
        );
    }

    #[tokio::test]
    async fn claude_stream_classifies_media_error_event() {
        let error = run_events_expect_error(vec![json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "image exceeds 5 MB maximum: 5316852 bytes > 5242880 bytes"
            }
        })])
        .await;

        let ApiError::ProviderMedia { kind, message } = error else {
            panic!("expected provider media error");
        };
        assert_eq!(kind, ProviderMediaErrorKind::ImageTooLarge);
        assert!(message.contains("image exceeds 5 MB maximum"));
    }

    #[tokio::test]
    async fn claude_stream_rejects_delta_for_unknown_index() {
        let error = run_events_expect_error(vec![
            json!({
                "type": "message_start",
                "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
            }),
            json!({
                "type": "content_block_delta",
                "index": 5,
                "delta": {"type": "text_delta", "text": "oops"}
            }),
        ])
        .await;

        assert!(
            error
                .to_string()
                .contains("text_delta received for unknown content block 5")
        );
    }

    #[tokio::test]
    async fn claude_stream_rejects_delta_after_content_block_stop() {
        let error = run_events_expect_error(vec![
            json!({
                "type": "message_start",
                "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
            }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": "done"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "oops"}
            }),
        ])
        .await;

        assert!(
            error
                .to_string()
                .contains("content_block_delta received after content_block_stop for block 0")
        );
    }

    #[tokio::test]
    async fn claude_stream_rejects_mismatched_delta_kind() {
        let error = run_events_expect_error(vec![
            json!({
                "type": "message_start",
                "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": []}
            }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{}"}
            }),
        ])
        .await;

        assert!(
            error
                .to_string()
                .contains("input_json_delta does not match content block 0")
        );
    }
}
