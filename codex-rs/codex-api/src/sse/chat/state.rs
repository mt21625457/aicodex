use super::ChatCompletionChunk;
use super::ChatToolCallDelta;
use super::ChatUsage;
use super::MAX_CHAT_CONTEXT_ITEM_BYTES;
use super::MAX_CHAT_RESPONSE_CONTEXT_BYTES;
use super::MAX_CHAT_TOOL_CALLS;
use super::MAX_CHAT_WIRE_IDENTIFIER_BYTES;
use super::provider_stream_error;
use crate::ChatToolCallInfo;
use crate::ChatToolCallKind;
use crate::common::ResponseEvent;
use crate::error::ApiError;
use crate::error::ProviderStreamErrorKind;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Default)]
struct ChatToolState {
    dense_index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

pub(super) struct ChatStreamState {
    pub(super) response_id: Option<String>,
    created_emitted: bool,
    assistant_started: bool,
    assistant_id: Option<ResponseItemId>,
    assistant_text: String,
    reasoning_started: bool,
    reasoning_done: bool,
    reasoning_id: Option<ResponseItemId>,
    reasoning_text: String,
    context_bytes: usize,
    tool_calls: BTreeMap<usize, ChatToolState>,
    next_dense_index: usize,
    last_provider_tool_index: Option<usize>,
    usage: Option<ChatUsage>,
    pub(super) finish_reason: Option<String>,
    tool_call_info: HashMap<String, ChatToolCallInfo>,
}

impl ChatStreamState {
    pub(super) fn new(tool_call_info: HashMap<String, ChatToolCallInfo>) -> Self {
        Self {
            response_id: None,
            created_emitted: false,
            assistant_started: false,
            assistant_id: None,
            assistant_text: String::new(),
            reasoning_started: false,
            reasoning_done: false,
            reasoning_id: None,
            reasoning_text: String::new(),
            context_bytes: 0,
            tool_calls: BTreeMap::new(),
            next_dense_index: 0,
            last_provider_tool_index: None,
            usage: None,
            finish_reason: None,
            tool_call_info,
        }
    }

    pub(super) async fn handle_chunk(
        &mut self,
        chunk: ChatCompletionChunk,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.response_id.is_none() {
            if let Some(response_id) = chunk.id.as_deref() {
                validate_wire_identifier(response_id, "response ID")?;
            }
            self.response_id = chunk.id;
        }
        if !self.created_emitted {
            tx_event
                .send(Ok(ResponseEvent::Created))
                .await
                .map_err(|error| ApiError::Stream(error.to_string()))?;
            self.created_emitted = true;
        }
        if let Some(usage) = chunk.usage {
            self.usage = Some(usage);
        }

        for choice in chunk.choices {
            if choice.index != 0 {
                tracing::trace!(
                    choice_index = choice.index,
                    "ignoring extra Chat completion choice"
                );
                continue;
            }
            if let Some(text) = choice.delta.reasoning_text() {
                self.push_reasoning_text(text, tx_event).await?;
            }
            if let Some(text) = choice
                .delta
                .content
                .as_ref()
                .and_then(super::text_from_value)
                && !text.is_empty()
            {
                self.push_assistant_text(text, tx_event).await?;
            }
            for tool_call in choice.delta.tool_calls {
                if !tool_call.is_meaningful() {
                    continue;
                }
                self.finish_reasoning(tx_event).await?;
                self.push_tool_delta(tool_call, tx_event).await?;
            }
            if choice.finish_reason.is_some() {
                if let Some(finish_reason) = choice.finish_reason.as_deref() {
                    validate_wire_identifier(finish_reason, "finish reason")?;
                }
                self.finish_reason = choice.finish_reason;
            }
        }
        Ok(())
    }

    async fn push_assistant_text(
        &mut self,
        text: String,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        append_bounded_chat_item(
            &mut self.assistant_text,
            &mut self.context_bytes,
            &text,
            "assistant text",
        )?;
        self.finish_reasoning(tx_event).await?;
        if !self.assistant_started {
            let assistant_id = ResponseItemId::new("chat_message");
            tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                    id: Some(assistant_id.clone()),
                    role: "assistant".to_string(),
                    content: Vec::new(),
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                })))
                .await
                .map_err(|error| ApiError::Stream(error.to_string()))?;
            self.assistant_started = true;
            self.assistant_id = Some(assistant_id);
        }
        tx_event
            .send(Ok(ResponseEvent::OutputTextDelta(text)))
            .await
            .map_err(|error| ApiError::Stream(error.to_string()))
    }

    async fn push_reasoning_text(
        &mut self,
        text: String,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if self.reasoning_done {
            return Err(provider_stream_error(
                ProviderStreamErrorKind::ParseError,
                "Chat reasoning content arrived after assistant content or tool calls",
            ));
        }
        append_bounded_chat_item(
            &mut self.reasoning_text,
            &mut self.context_bytes,
            &text,
            "reasoning content",
        )?;
        if !self.reasoning_started {
            let reasoning_id = ResponseItemId::new("chat_reasoning");
            tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(
                    ResponseItem::Reasoning {
                        id: Some(reasoning_id.clone()),
                        summary: Vec::new(),
                        content: Some(Vec::new()),
                        encrypted_content: None,
                        internal_chat_message_metadata_passthrough: None,
                    },
                )))
                .await
                .map_err(|error| ApiError::Stream(error.to_string()))?;
            self.reasoning_started = true;
            self.reasoning_id = Some(reasoning_id);
        }
        tx_event
            .send(Ok(ResponseEvent::ReasoningContentDelta {
                delta: text,
                content_index: 0,
            }))
            .await
            .map_err(|error| ApiError::Stream(error.to_string()))
    }

    async fn finish_reasoning(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if !self.reasoning_started || self.reasoning_done {
            return Ok(());
        }
        let reasoning_id = self.reasoning_id.clone().ok_or_else(|| {
            provider_stream_error(
                ProviderStreamErrorKind::ParseError,
                "Chat reasoning item is missing its stable identifier",
            )
        })?;
        tx_event
            .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                id: Some(reasoning_id),
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: self.reasoning_text.clone(),
                }]),
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            })))
            .await
            .map_err(|error| ApiError::Stream(error.to_string()))?;
        self.reasoning_done = true;
        Ok(())
    }

    async fn push_tool_delta(
        &mut self,
        delta: ChatToolCallDelta,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        let provider_index = delta
            .index
            .or(self.last_provider_tool_index)
            .unwrap_or(self.next_dense_index);
        if !self.tool_calls.contains_key(&provider_index) {
            if self.tool_calls.len() >= MAX_CHAT_TOOL_CALLS {
                return Err(provider_stream_error(
                    ProviderStreamErrorKind::ParseError,
                    format!("Chat response exceeded the {MAX_CHAT_TOOL_CALLS} tool-call limit"),
                ));
            }
            let dense_index = self.next_dense_index;
            self.next_dense_index += 1;
            self.tool_calls.insert(
                provider_index,
                ChatToolState {
                    dense_index,
                    ..Default::default()
                },
            );
        }
        let state = self.tool_calls.get_mut(&provider_index).ok_or_else(|| {
            provider_stream_error(
                ProviderStreamErrorKind::ParseError,
                "Chat tool state disappeared after insertion",
            )
        })?;
        if let Some(id) = delta.id.filter(|id| !id.is_empty()) {
            replace_bounded_context_value(
                &mut state.id,
                &mut self.context_bytes,
                id,
                "tool-call ID",
            )?;
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name.filter(|name| !name.is_empty()) {
                replace_bounded_context_value(
                    &mut state.name,
                    &mut self.context_bytes,
                    name,
                    "tool name",
                )?;
            }
            if let Some(arguments) = function.arguments.filter(|arguments| !arguments.is_empty()) {
                append_bounded_chat_item(
                    &mut state.arguments,
                    &mut self.context_bytes,
                    &arguments,
                    "tool-call arguments",
                )?;
                let call_id = state
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("chat_tool_{}", state.dense_index));
                tx_event
                    .send(Ok(ResponseEvent::ToolCallInputDelta {
                        item_id: call_id.clone(),
                        call_id: Some(call_id),
                        delta: arguments,
                    }))
                    .await
                    .map_err(|error| ApiError::Stream(error.to_string()))?;
            }
        }
        self.last_provider_tool_index = Some(provider_index);
        Ok(())
    }

    pub(super) async fn finish(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    ) -> Result<(), ApiError> {
        if !self.created_emitted {
            return Err(provider_stream_error(
                ProviderStreamErrorKind::ClosedBeforeMessageStart,
                "Chat stream completed without a response chunk",
            ));
        }
        if self.assistant_text.is_empty()
            && self.reasoning_text.is_empty()
            && self.tool_calls.is_empty()
        {
            return Err(provider_stream_error(
                ProviderStreamErrorKind::ClosedAfterMessageStartBeforeStop,
                "Chat stream completed without assistant content or tool calls",
            ));
        }

        self.finish_reasoning(tx_event).await?;
        if !self.assistant_text.is_empty() {
            let assistant_id = self.assistant_id.clone().ok_or_else(|| {
                provider_stream_error(
                    ProviderStreamErrorKind::ParseError,
                    "Chat assistant item is missing its stable identifier",
                )
            })?;
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Message {
                    id: Some(assistant_id),
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: self.assistant_text.clone(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                })))
                .await
                .map_err(|error| ApiError::Stream(error.to_string()))?;
        }
        for state in std::mem::take(&mut self.tool_calls).into_values() {
            let item = self.tool_item(state)?;
            tx_event
                .send(Ok(ResponseEvent::OutputItemDone(item)))
                .await
                .map_err(|error| ApiError::Stream(error.to_string()))?;
        }

        let finish_reason = self.finish_reason.clone();
        tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id: self.response_id.clone().unwrap_or_default(),
                token_usage: self.usage.as_ref().map(ChatUsage::token_usage),
                end_turn: finish_reason
                    .as_deref()
                    .map(|reason| reason != "tool_calls"),
                provider_stop_reason: finish_reason,
            }))
            .await
            .map_err(|error| ApiError::Stream(error.to_string()))
    }

    fn tool_item(&self, state: ChatToolState) -> Result<ResponseItem, ApiError> {
        let ChatToolState {
            dense_index,
            id,
            name,
            arguments,
        } = state;
        let wire_name = name.ok_or_else(|| {
            provider_stream_error(
                ProviderStreamErrorKind::ParseError,
                format!("Chat tool call {dense_index} is missing function.name"),
            )
        })?;
        let call_id = id.unwrap_or_else(|| format!("chat_tool_{dense_index}"));
        let info = self
            .tool_call_info
            .get(&wire_name)
            .cloned()
            .unwrap_or(ChatToolCallInfo {
                name: wire_name,
                namespace: None,
                kind: ChatToolCallKind::Function,
            });
        Ok(match info.kind {
            ChatToolCallKind::Function => ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", &call_id)),
                name: info.name,
                namespace: info.namespace,
                arguments,
                call_id,
                internal_chat_message_metadata_passthrough: None,
            },
            ChatToolCallKind::Custom => {
                let input = serde_json::from_str::<Value>(&arguments)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("input")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .ok_or_else(|| {
                        provider_stream_error(
                            ProviderStreamErrorKind::ParseError,
                            format!(
                                "Chat custom tool `{}` must provide a string `input` field",
                                info.name
                            ),
                        )
                    })?;
                ResponseItem::CustomToolCall {
                    id: Some(ResponseItemId::with_suffix("ctc", &call_id)),
                    status: None,
                    call_id,
                    name: info.name,
                    namespace: info.namespace,
                    input,
                    internal_chat_message_metadata_passthrough: None,
                }
            }
            ChatToolCallKind::ToolSearch => ResponseItem::ToolSearchCall {
                id: Some(ResponseItemId::with_suffix("tsc", &call_id)),
                call_id: Some(call_id),
                status: Some("completed".to_string()),
                execution: "client".to_string(),
                arguments: serde_json::from_str(&arguments).map_err(|error| {
                    provider_stream_error(
                        ProviderStreamErrorKind::ParseError,
                        format!("invalid Chat tool_search arguments: {error}"),
                    )
                })?,
                internal_chat_message_metadata_passthrough: None,
            },
        })
    }
}

fn append_bounded_chat_item(
    output: &mut String,
    context_bytes: &mut usize,
    delta: &str,
    item_kind: &str,
) -> Result<(), ApiError> {
    if output.len().saturating_add(delta.len()) > MAX_CHAT_CONTEXT_ITEM_BYTES {
        return Err(provider_stream_error(
            ProviderStreamErrorKind::ParseError,
            format!(
                "Chat {item_kind} exceeded the {MAX_CHAT_CONTEXT_ITEM_BYTES}-byte model-context limit"
            ),
        ));
    }
    reserve_context_bytes(context_bytes, delta.len(), item_kind)?;
    output.push_str(delta);
    Ok(())
}

fn replace_bounded_context_value(
    output: &mut Option<String>,
    context_bytes: &mut usize,
    value: String,
    item_kind: &str,
) -> Result<(), ApiError> {
    validate_wire_identifier(&value, item_kind)?;
    let previous_len = output.as_ref().map_or(0, String::len);
    *context_bytes = context_bytes.saturating_sub(previous_len);
    reserve_context_bytes(context_bytes, value.len(), item_kind)?;
    *output = Some(value);
    Ok(())
}

fn reserve_context_bytes(
    context_bytes: &mut usize,
    additional_bytes: usize,
    item_kind: &str,
) -> Result<(), ApiError> {
    let next = context_bytes.saturating_add(additional_bytes);
    if next > MAX_CHAT_RESPONSE_CONTEXT_BYTES {
        return Err(provider_stream_error(
            ProviderStreamErrorKind::ParseError,
            format!(
                "Chat {item_kind} exceeded the {MAX_CHAT_RESPONSE_CONTEXT_BYTES}-byte response context limit"
            ),
        ));
    }
    *context_bytes = next;
    Ok(())
}

fn validate_wire_identifier(value: &str, item_kind: &str) -> Result<(), ApiError> {
    if value.len() > MAX_CHAT_WIRE_IDENTIFIER_BYTES {
        return Err(provider_stream_error(
            ProviderStreamErrorKind::ParseError,
            format!(
                "Chat {item_kind} exceeded the {MAX_CHAT_WIRE_IDENTIFIER_BYTES}-byte identifier limit"
            ),
        ));
    }
    Ok(())
}
