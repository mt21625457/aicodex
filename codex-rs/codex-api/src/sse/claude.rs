use crate::common::ClaudeToolCallInfo;
use crate::common::ClaudeToolCallKind;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
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

pub fn spawn_claude_response_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
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
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageStart {
    id: String,
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
            Self::EndTurn
            | Self::MaxTokens
            | Self::StopSequence
            | Self::Refusal
            | Self::ModelContextWindowExceeded => Some(true),
            Self::ToolUse | Self::PauseTurn => Some(false),
            Self::Unknown(_) => None,
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
    #[serde(other)]
    Unknown,
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
}

impl ClaudeUsage {
    fn merge(&mut self, usage: ClaudeUsage) {
        self.input_tokens = usage.input_tokens.or(self.input_tokens);
        self.output_tokens = usage.output_tokens.or(self.output_tokens);
        self.cache_read_input_tokens = usage
            .cache_read_input_tokens
            .or(self.cache_read_input_tokens);
        self.cache_creation_input_tokens = usage
            .cache_creation_input_tokens
            .or(self.cache_creation_input_tokens);
    }

    fn token_usage(&self) -> Option<TokenUsage> {
        let input_tokens = self.input_tokens.unwrap_or_default();
        let output_tokens = self.output_tokens.unwrap_or_default();
        let cached_input_tokens = self
            .cache_read_input_tokens
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
            output_tokens,
            reasoning_output_tokens: 0,
            total_tokens,
        })
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
    reasoning_blocks: BTreeMap<usize, String>,
    reasoning_signatures: BTreeMap<usize, String>,
    tool_blocks: BTreeMap<usize, ToolUseState>,
    provider_state_blocks: BTreeMap<usize, Value>,
    usage: ClaudeUsage,
    stop_reason: Option<ClaudeStopReason>,
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeStreamBlockKind {
    Text,
    ToolUse,
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
        Self {
            tool_call_info,
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
                return Err(ApiError::Stream(message));
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
                let state = self.tool_blocks.entry(index).or_default();
                state.id = Some(id);
                state.name = Some(name);
                state.input = Some(input);
                self.maybe_send_custom_tool_item_added(index, tx_event)
                    .await?;
                self.maybe_send_custom_tool_input_delta(index, tx_event)
                    .await?;
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
                self.ensure_delta_matches(
                    index,
                    ClaudeStreamBlockKind::ToolUse,
                    "input_json_delta",
                )?;
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
            let value = serde_json::from_str::<Value>(&state.partial_json).map_err(|err| {
                ApiError::Stream(format!(
                    "invalid Claude tool input JSON for content block {index}: {err}"
                ))
            })?;
            state.input = Some(value);
            state.partial_json.clear();
        };
        self.maybe_send_custom_tool_input_delta(index, tx_event)
            .await?;

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
                    id: self.reasoning_id_for_block(index),
                    summary: Vec::new(),
                    content: Some(Vec::new()),
                    encrypted_content: None,
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
                    id: Some(call_id.clone()),
                    status: None,
                    call_id,
                    name: info.name.clone(),
                    input: String::new(),
                },
            )))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
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
        let Some(item) = self.final_response_item_for_block(index)? else {
            return Ok(());
        };
        tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    fn final_response_item_for_block(
        &self,
        index: usize,
    ) -> Result<Option<ResponseItem>, ApiError> {
        let Some(kind) = self.block_kinds.get(&index) else {
            return Ok(None);
        };
        match kind {
            ClaudeStreamBlockKind::Text => {
                if let Some(text) = self.text_blocks.get(&index)
                    && !text.is_empty()
                {
                    return Ok(Some(ResponseItem::Message {
                        id: self.message_id_for_block(index),
                        role: "assistant".to_string(),
                        content: vec![ContentItem::OutputText { text: text.clone() }],
                        phase: None,
                    }));
                }
                Ok(None)
            }
            ClaudeStreamBlockKind::Thinking => Ok(Some(self.reasoning_item_for_block(index))),
            ClaudeStreamBlockKind::ToolUse => self.tool_call_item_for_block(index),
            ClaudeStreamBlockKind::ProviderState => {
                Ok(self
                    .provider_state_blocks
                    .get(&index)
                    .map(|value| ResponseItem::Compaction {
                        encrypted_content: value.to_string(),
                    }))
            }
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
            id: self.reasoning_id_for_block(index),
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText { text }]),
            encrypted_content: signature,
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
        let input = state.input_value()?;
        let info = self
            .tool_call_info
            .get(claude_name)
            .cloned()
            .unwrap_or_else(|| ClaudeToolCallInfo {
                name: claude_name.clone(),
                namespace: None,
                kind: ClaudeToolCallKind::Function,
            });

        Ok(Some(match info.kind {
            ClaudeToolCallKind::Function => ResponseItem::FunctionCall {
                id: Some(call_id.clone()),
                name: info.name,
                namespace: info.namespace,
                arguments: stringify_tool_input(&input),
                call_id,
            },
            ClaudeToolCallKind::Custom => match custom_tool_input(&info.name, &input) {
                Ok(input) => ResponseItem::CustomToolCall {
                    id: Some(call_id.clone()),
                    status: None,
                    call_id,
                    name: info.name,
                    input,
                },
                Err(message) => ResponseItem::CustomToolCall {
                    id: Some(call_id.clone()),
                    status: Some(format!(
                        "{INVALID_CLAUDE_CUSTOM_TOOL_INPUT_STATUS_PREFIX}{message}"
                    )),
                    call_id,
                    name: info.name,
                    input: String::new(),
                },
            },
            ClaudeToolCallKind::ToolSearch => ResponseItem::ToolSearchCall {
                id: Some(call_id.clone()),
                call_id: Some(call_id),
                status: None,
                execution: "client".to_string(),
                arguments: input,
            },
        }))
    }

    fn response_id(&self) -> String {
        self.response_id
            .clone()
            .unwrap_or_else(|| "claude-response".to_string())
    }

    fn message_id_for_block(&self, index: usize) -> Option<String> {
        self.message_id.as_ref().map(|message_id| {
            if index == 0 {
                message_id.clone()
            } else {
                format!("{message_id}_text_{index}")
            }
        })
    }

    fn reasoning_id_for_block(&self, index: usize) -> String {
        format!("{}_reasoning_{index}", self.response_id())
    }
}

impl ToolUseState {
    fn input_value(&self) -> Result<Value, ApiError> {
        if !self.partial_json.trim().is_empty() {
            return serde_json::from_str::<Value>(&self.partial_json).map_err(|err| {
                ApiError::Stream(format!(
                    "invalid Claude tool input JSON at message stop: {err}"
                ))
            });
        }
        Ok(self
            .input
            .clone()
            .unwrap_or_else(|| Value::Object(Map::new())))
    }

    fn complete_input_value_if_available(&self) -> Option<Value> {
        if !self.partial_json.trim().is_empty() {
            return serde_json::from_str::<Value>(&self.partial_json).ok();
        }
        self.input.clone()
    }
}

fn stringify_tool_input(input: &Value) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string())
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
        || kind.ends_with("_tool_use")
        || kind.ends_with("_tool_result")
}

fn parse_claude_stream_event(event_name: &str, data: &str) -> Result<ClaudeStreamEvent, ApiError> {
    if data.trim().is_empty() && !event_name.trim().is_empty() {
        return Ok(ClaudeStreamEvent::event_name_only(event_name));
    }

    serde_json::from_str(data)
        .map_err(|err| ApiError::Stream(format!("failed to parse Claude SSE event: {err}")))
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

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("Claude SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before message_stop".to_string(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("Claude SSE event: {} {}", &sse.event, &sse.data);
        let event: ClaudeStreamEvent = match parse_claude_stream_event(&sse.event, &sse.data) {
            Ok(event) => event,
            Err(e) => {
                debug!("failed to parse Claude SSE event: {e}, data: {}", &sse.data);
                let _ = tx_event.send(Err(e)).await;
                return;
            }
        };

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
    fn claude_usage_maps_cache_creation_without_cached_input() {
        let usage = ClaudeUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: Some(6),
        };

        assert_eq!(
            usage.token_usage(),
            Some(TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
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
        };

        assert_eq!(
            usage.token_usage(),
            Some(TokenUsage {
                input_tokens: 3,
                cached_input_tokens: 3,
                output_tokens: 1,
                reasoning_output_tokens: 0,
                total_tokens: 4,
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
            ResponseEvent::OutputItemDone(ResponseItem::Compaction { encrypted_content })
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
            ResponseEvent::OutputItemDone(ResponseItem::Compaction { encrypted_content })
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
            ResponseItem::Reasoning { id, .. } => Some(format!("reasoning:{id}")),
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
                "reasoning:msg_1_reasoning_1".to_string(),
                "message:msg_1_text_2".to_string(),
            ]
        );
        assert_eq!(done, added);
        assert_eq!(
            lifecycle,
            vec![
                "added:message:msg_1".to_string(),
                "done:message:msg_1".to_string(),
                "added:reasoning:msg_1_reasoning_1".to_string(),
                "done:reasoning:msg_1_reasoning_1".to_string(),
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

        assert!(
            error
                .to_string()
                .contains("invalid Claude tool input JSON for content block 0")
        );
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
