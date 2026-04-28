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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

pub fn spawn_claude_response_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
) -> ResponseStream {
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(process_sse(
        stream_response.bytes,
        tx_event,
        idle_timeout,
        telemetry,
        tool_call_info,
    ));
    ResponseStream { rx_event }
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClaudeStopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeStreamContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default = "empty_json_object")]
        input: Value,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    #[serde(other)]
    Unknown,
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

    fn token_usage(&self) -> TokenUsage {
        let input_tokens = self.input_tokens.unwrap_or_default();
        let output_tokens = self.output_tokens.unwrap_or_default();
        let cached_input_tokens = self.cache_read_input_tokens.unwrap_or_default()
            + self.cache_creation_input_tokens.unwrap_or_default();
        TokenUsage {
            input_tokens,
            cached_input_tokens,
            output_tokens,
            reasoning_output_tokens: 0,
            total_tokens: input_tokens + output_tokens,
        }
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
    message_item_started: bool,
    reasoning_item_started: bool,
    block_kinds: BTreeMap<usize, ClaudeStreamBlockKind>,
    text_blocks: BTreeMap<usize, String>,
    reasoning_blocks: BTreeMap<usize, String>,
    reasoning_signatures: BTreeMap<usize, String>,
    tool_blocks: BTreeMap<usize, ToolUseState>,
    usage: ClaudeUsage,
    tool_call_info: HashMap<String, ClaudeToolCallInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeStreamBlockKind {
    Text,
    ToolUse,
    Thinking,
    Unknown,
}

#[derive(Default)]
struct ToolUseState {
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
    partial_json: String,
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
                self.handle_content_block_stop(index)?;
            }
            ClaudeStreamEvent::MessageDelta { delta, usage } => {
                self.ensure_message_started("message_delta")?;
                if let Some(ClaudeMessageDelta {
                    stop_reason,
                    stop_sequence,
                }) = delta
                {
                    trace!(?stop_reason, ?stop_sequence, "Claude message delta");
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
                self.ensure_message_item_started(tx_event).await?;
                if !text.is_empty() {
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
            }
            ClaudeStreamContentBlock::Thinking {
                thinking,
                signature,
            } => {
                self.block_kinds
                    .insert(index, ClaudeStreamBlockKind::Thinking);
                self.ensure_reasoning_item_started(tx_event).await?;
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
            ClaudeStreamContentBlock::Unknown => {
                self.block_kinds
                    .insert(index, ClaudeStreamBlockKind::Unknown);
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
        match delta {
            ClaudeStreamDelta::TextDelta { text } => {
                self.ensure_delta_matches(index, ClaudeStreamBlockKind::Text, "text_delta")?;
                self.ensure_message_item_started(tx_event).await?;
                self.push_text_delta(index, &text, tx_event).await?;
            }
            ClaudeStreamDelta::InputJsonDelta { partial_json } => {
                self.ensure_delta_matches(
                    index,
                    ClaudeStreamBlockKind::ToolUse,
                    "input_json_delta",
                )?;
                let state = self.tool_blocks.entry(index).or_default();
                state.partial_json.push_str(&partial_json);
                self.maybe_send_custom_tool_input_delta(index, &partial_json, tx_event)
                    .await?;
            }
            ClaudeStreamDelta::ThinkingDelta { thinking } => {
                self.ensure_delta_matches(
                    index,
                    ClaudeStreamBlockKind::Thinking,
                    "thinking_delta",
                )?;
                self.ensure_reasoning_item_started(tx_event).await?;
                self.push_reasoning_delta(index, &thinking, tx_event)
                    .await?;
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

    fn handle_content_block_stop(&mut self, index: usize) -> Result<(), ApiError> {
        self.ensure_message_started("content_block_stop")?;
        let Some(state) = self.tool_blocks.get_mut(&index) else {
            return Ok(());
        };
        if state.partial_json.trim().is_empty() {
            return Ok(());
        }

        let value = serde_json::from_str::<Value>(&state.partial_json).map_err(|err| {
            ApiError::Stream(format!(
                "invalid Claude tool input JSON for content block {index}: {err}"
            ))
        })?;
        state.input = Some(value);
        state.partial_json.clear();
        Ok(())
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

    async fn ensure_message_item_started(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.message_item_started {
            return Ok(());
        }
        self.message_item_started = true;
        tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                id: self.message_id.clone(),
                role: "assistant".to_string(),
                content: Vec::new(),
                end_turn: None,
                phase: None,
            })))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn ensure_reasoning_item_started(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.reasoning_item_started {
            return Ok(());
        }
        self.reasoning_item_started = true;
        tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(
                ResponseItem::Reasoning {
                    id: self.reasoning_id(),
                    summary: Vec::new(),
                    content: Some(Vec::new()),
                    encrypted_content: self.join_reasoning_signatures(),
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
        &self,
        index: usize,
        partial_json: &str,
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
        tx_event
            .send(Ok(ResponseEvent::ToolCallInputDelta {
                item_id: call_id.clone(),
                call_id: Some(call_id),
                delta: partial_json.to_string(),
            }))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    async fn finish(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.reasoning_item_started {
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                    id: self.reasoning_id(),
                    summary: Vec::new(),
                    content: Some(vec![ReasoningItemContent::ReasoningText {
                        text: self.join_reasoning(),
                    }]),
                    encrypted_content: self.join_reasoning_signatures(),
                })))
                .await
                .map_err(|err| ApiError::Stream(err.to_string()))?;
        }
        if self.message_item_started {
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Message {
                    id: self.message_id.clone(),
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: self.join_text(),
                    }],
                    end_turn: Some(true),
                    phase: None,
                })))
                .await
                .map_err(|err| ApiError::Stream(err.to_string()))?;
        }

        for item in self.tool_call_items()? {
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(item)))
                .await
                .map_err(|err| ApiError::Stream(err.to_string()))?;
        }

        tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id: self.response_id(),
                token_usage: Some(self.usage.token_usage()),
            }))
            .await
            .map_err(|err| ApiError::Stream(err.to_string()))
    }

    fn tool_call_items(&self) -> Result<Vec<ResponseItem>, ApiError> {
        self.tool_blocks
            .values()
            .filter_map(|state| {
                let claude_name = state.name.as_ref()?;
                let call_id = state.id.clone().unwrap_or_else(|| claude_name.clone());
                let input = match state.input_value() {
                    Ok(input) => input,
                    Err(err) => return Some(Err(err)),
                };
                let info = self
                    .tool_call_info
                    .get(claude_name)
                    .cloned()
                    .unwrap_or_else(|| ClaudeToolCallInfo {
                        name: claude_name.clone(),
                        namespace: None,
                        kind: ClaudeToolCallKind::Function,
                    });

                Some(match info.kind {
                    ClaudeToolCallKind::Function => Ok(ResponseItem::FunctionCall {
                        id: Some(call_id.clone()),
                        name: info.name,
                        namespace: info.namespace,
                        arguments: stringify_tool_input(&input),
                        call_id,
                    }),
                    ClaudeToolCallKind::Custom => Ok(ResponseItem::CustomToolCall {
                        id: Some(call_id.clone()),
                        status: None,
                        call_id,
                        name: info.name,
                        input: custom_tool_input(&input),
                    }),
                    ClaudeToolCallKind::ToolSearch => Ok(ResponseItem::ToolSearchCall {
                        id: Some(call_id.clone()),
                        call_id: Some(call_id),
                        status: None,
                        execution: "client".to_string(),
                        arguments: input,
                    }),
                })
            })
            .collect()
    }

    fn response_id(&self) -> String {
        self.response_id
            .clone()
            .unwrap_or_else(|| "claude-response".to_string())
    }

    fn reasoning_id(&self) -> String {
        format!("{}_reasoning", self.response_id())
    }

    fn join_text(&self) -> String {
        self.text_blocks.values().cloned().collect::<String>()
    }

    fn join_reasoning(&self) -> String {
        self.reasoning_blocks.values().cloned().collect::<String>()
    }

    fn join_reasoning_signatures(&self) -> Option<String> {
        let signatures = self
            .reasoning_signatures
            .values()
            .cloned()
            .collect::<String>();
        (!signatures.is_empty()).then_some(signatures)
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
}

fn stringify_tool_input(input: &Value) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string())
}

fn custom_tool_input(input: &Value) -> String {
    input
        .get("input")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| stringify_tool_input(input))
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
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(16);
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
                cached_input_tokens: 7,
                output_tokens: 8,
                reasoning_output_tokens: 0,
                total_tokens: 20,
            }
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
    }

    #[tokio::test]
    async fn claude_stream_maps_custom_tool_input() {
        let mut tool_call_info = HashMap::new();
        tool_call_info.insert(
            "apply_patch".to_string(),
            ClaudeToolCallInfo {
                name: "apply_patch".to_string(),
                namespace: None,
                kind: ClaudeToolCallKind::Custom,
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
                    "delta": {"type": "input_json_delta", "partial_json": "{\"input\":\"*** Begin Patch\"}"}
                }),
                json!({"type": "message_stop"}),
            ],
            tool_call_info,
        )
        .await;

        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemAdded(ResponseItem::CustomToolCall { name, .. })
                if name == "apply_patch"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall { input, .. })
                if input == "*** Begin Patch"
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
