use anthropic_sdk::Anthropic;
use anthropic_sdk::RequestOptions;
use anthropic_sdk::types::messages::MessageDeltaUsage;
use anthropic_sdk::types::messages::RawContentBlockDelta;
use anthropic_sdk::types::messages::RawMessageStreamEvent;
use codex_api::ResponseEvent;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::TokenUsage;
use futures::StreamExt;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use std::collections::BTreeMap;
use std::collections::HashSet;
use tokio::sync::mpsc;

use crate::auth::build_client_options;
use crate::dto::AnthropicTurnRequest;
use crate::error::map_anthropic_error;
use crate::request::build_request;
use crate::request::encode_anthropic_reasoning_blocks;
use crate::tool_mapping::ToolUseState;
use crate::tool_mapping::build_tool_views;
use crate::tool_mapping::finalize_tool_use;
use crate::tool_mapping::freeform_tool_names;
use crate::tool_mapping::tool_use_to_response_item;

pub async fn stream_anthropic(
    request: AnthropicTurnRequest,
) -> Result<mpsc::Receiver<Result<ResponseEvent>>> {
    let client = Anthropic::new(build_client_options(&request)?).map_err(map_anthropic_error)?;
    let message_request = build_request(&request)?;
    let request_options = build_request_options(&request)?;
    let tool_views = build_tool_views(&request.tools);
    let freeform_tool_names = freeform_tool_names(&request.tools);

    let (tx_event, rx_event) = mpsc::channel(128);
    tokio::spawn(async move {
        let result = stream_to_channel(
            &client,
            message_request,
            request_options,
            tool_views,
            freeform_tool_names,
            &tx_event,
        )
        .await;
        if let Err(err) = result {
            let _ = tx_event.send(Err(err)).await;
        }
    });

    Ok(rx_event)
}

async fn stream_to_channel(
    client: &Anthropic,
    request: anthropic_sdk::types::messages::MessageCreateParams,
    request_options: RequestOptions,
    _tool_views: Vec<crate::tool_mapping::AnthropicToolView>,
    freeform_tool_names: HashSet<String>,
    tx_event: &mpsc::Sender<Result<ResponseEvent>>,
) -> Result<()> {
    let mut stream = client
        .messages
        .create_stream(request, Some(request_options))
        .await
        .map_err(map_anthropic_error)?;

    let mut state = StreamState {
        freeform_tool_names,
        ..StreamState::default()
    };
    while let Some(event_result) = stream.next().await {
        let event = event_result.map_err(map_anthropic_error)?;
        state.handle_event(event, tx_event).await?;
    }
    state.finish(tx_event).await
}

fn build_request_options(request: &AnthropicTurnRequest) -> Result<RequestOptions> {
    let mut options = RequestOptions::default();
    if let Some(turn_metadata_header) = &request.turn_metadata_header
        && let Ok(value) = HeaderValue::from_str(turn_metadata_header)
    {
        options = options.header(HeaderName::from_static("x-codex-turn-metadata"), value);
    }
    Ok(options)
}

#[derive(Debug, Default)]
struct StreamState {
    response_id: Option<String>,
    message_id: Option<String>,
    message_started: bool,
    reasoning_started: bool,
    reasoning_done: bool,
    reasoning_blocks: BTreeMap<usize, serde_json::Value>,
    reasoning_summary_started: BTreeMap<usize, bool>,
    reasoning_text: BTreeMap<usize, String>,
    tool_uses: BTreeMap<usize, ToolUseState>,
    web_searches: BTreeMap<usize, WebSearchState>,
    image_generations: BTreeMap<usize, ImageGenerationState>,
    freeform_tool_names: HashSet<String>,
    completed: bool,
    text: String,
    stop_reason: Option<String>,
    usage: Option<MessageDeltaUsage>,
}

#[derive(Debug, Clone, Default)]
struct WebSearchState {
    id: Option<String>,
    action: Option<WebSearchAction>,
    completed: bool,
}

#[derive(Debug, Clone, Default)]
struct ImageGenerationState {
    id: Option<String>,
    prompt: Option<String>,
    revised_prompt: Option<String>,
    result: Option<String>,
    completed: bool,
}

impl StreamState {
    async fn handle_event(
        &mut self,
        event: RawMessageStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        match event {
            RawMessageStreamEvent::MessageStart { message } => {
                self.response_id = Some(message.id.clone());
                self.message_id = Some(message.id);
                self.message_started = false;
                self.reasoning_started = false;
                self.reasoning_done = false;
                self.reasoning_blocks.clear();
                self.reasoning_summary_started.clear();
                self.reasoning_text.clear();
                self.tool_uses.clear();
                self.web_searches.clear();
                self.image_generations.clear();
                self.completed = false;
                self.text.clear();
                self.stop_reason = None;
                self.usage = None;
                send_event(tx_event, ResponseEvent::Created).await?;
            }
            RawMessageStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                match content_block
                    .get("type")
                    .and_then(ValueExt::as_str)
                    .unwrap_or_default()
                {
                    "tool_use" => {
                        let entry = self.tool_uses.entry(index).or_default();
                        if let Some(id) =
                            content_block.get("id").and_then(serde_json::Value::as_str)
                        {
                            entry.id = Some(id.to_string());
                        }
                        if let Some(name) = content_block
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                        {
                            entry.name = Some(name.to_string());
                        }
                        if let Some(input) = content_block.get("input") {
                            entry.input = Some(input.clone());
                        }
                    }
                    "thinking" | "redacted_thinking" => {
                        self.reasoning_blocks.insert(index, content_block);
                    }
                    "server_tool_use" => {
                        match content_block
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                        {
                            "web_search" => {
                                let state = self.web_searches.entry(index).or_default();
                                state.id = content_block
                                    .get("id")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToString::to_string);
                                state.action = Some(WebSearchAction::Search {
                                    query: content_block
                                        .get("input")
                                        .and_then(|input| input.get("query"))
                                        .and_then(serde_json::Value::as_str)
                                        .map(ToString::to_string),
                                    queries: content_block
                                        .get("input")
                                        .and_then(|input| input.get("queries"))
                                        .and_then(serde_json::Value::as_array)
                                        .map(|queries| {
                                            queries
                                                .iter()
                                                .filter_map(serde_json::Value::as_str)
                                                .map(ToString::to_string)
                                                .collect::<Vec<_>>()
                                        }),
                                });
                            }
                            "image_generation" => {
                                let state = self.image_generations.entry(index).or_default();
                                state.id = content_block
                                    .get("id")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToString::to_string);
                                state.prompt = content_block
                                    .get("input")
                                    .and_then(|input| {
                                        input.get("prompt").or_else(|| input.get("revised_prompt"))
                                    })
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToString::to_string);
                            }
                            _ => {
                                return Err(CodexErr::UnsupportedOperation(
                                    "anthropic server_tool_use is not implemented yet".to_string(),
                                ));
                            }
                        }
                    }
                    "web_search_tool_result" => {
                        if let Some(tool_use_id) = content_block
                            .get("tool_use_id")
                            .and_then(serde_json::Value::as_str)
                            && let Some((_, state)) = self
                                .web_searches
                                .iter_mut()
                                .find(|(_, state)| state.id.as_deref() == Some(tool_use_id))
                        {
                            state.completed = true;
                        }
                    }
                    "image_generation_tool_result" => {
                        if let Some(tool_use_id) = content_block
                            .get("tool_use_id")
                            .and_then(serde_json::Value::as_str)
                        {
                            let state = if let Some((_, state)) = self
                                .image_generations
                                .iter_mut()
                                .find(|(_, state)| state.id.as_deref() == Some(tool_use_id))
                            {
                                state
                            } else {
                                let state = self.image_generations.entry(index).or_default();
                                state.id = Some(tool_use_id.to_string());
                                state
                            };
                            state.completed = true;
                            state.revised_prompt = content_block
                                .get("revised_prompt")
                                .and_then(serde_json::Value::as_str)
                                .map(ToString::to_string)
                                .or_else(|| state.prompt.clone());
                            state.result = content_block
                                .get("result")
                                .and_then(serde_json::Value::as_str)
                                .map(ToString::to_string)
                                .or_else(|| {
                                    content_block
                                        .get("content")
                                        .and_then(serde_json::Value::as_array)
                                        .and_then(|content| {
                                            content.iter().find_map(|block| {
                                                let source = block.get("source")?;
                                                (source.get("type")?.as_str() == Some("base64"))
                                                    .then(|| source.get("data"))?
                                                    .and_then(serde_json::Value::as_str)
                                                    .map(ToString::to_string)
                                            })
                                        })
                                });
                        }
                    }
                    _ => {}
                }
            }
            RawMessageStreamEvent::ContentBlockDelta { index, delta } => match delta {
                RawContentBlockDelta::TextDelta { text } => {
                    self.ensure_message_started(tx_event).await?;
                    self.text.push_str(&text);
                    send_event(tx_event, ResponseEvent::OutputTextDelta(text)).await?;
                }
                RawContentBlockDelta::ThinkingDelta { thinking } => {
                    if let Some(block) = self.reasoning_blocks.get_mut(&index) {
                        set_string_field(block, "thinking", &thinking)?;
                    }
                    self.handle_reasoning_delta(index, thinking, tx_event)
                        .await?;
                }
                RawContentBlockDelta::SignatureDelta { signature } => {
                    if let Some(block) = self.reasoning_blocks.get_mut(&index) {
                        set_string_field(block, "signature", &signature)?;
                    }
                }
                RawContentBlockDelta::CitationsDelta { .. } => {}
                RawContentBlockDelta::InputJsonDelta { partial_json } => {
                    self.tool_uses
                        .entry(index)
                        .or_default()
                        .partial_json
                        .push_str(&partial_json);
                }
                RawContentBlockDelta::Unknown => {}
            },
            RawMessageStreamEvent::MessageDelta { delta, usage } => {
                self.stop_reason = delta.stop_reason;
                self.usage = Some(usage);
            }
            RawMessageStreamEvent::ContentBlockStop { index } => {
                if let Some(tool_use) = self.tool_uses.get_mut(&index) {
                    finalize_tool_use(tool_use);
                }
            }
            RawMessageStreamEvent::MessageStop => {
                self.finish(tx_event).await?;
            }
        }
        Ok(())
    }

    async fn ensure_message_started(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        if self.message_started {
            return Ok(());
        }

        self.finish_reasoning_if_needed(tx_event).await?;
        self.message_started = true;
        send_event(
            tx_event,
            ResponseEvent::OutputItemAdded(ResponseItem::Message {
                id: self.message_id.clone(),
                role: "assistant".to_string(),
                content: Vec::new(),
                end_turn: None,
                phase: None,
            }),
        )
        .await
    }

    async fn handle_reasoning_delta(
        &mut self,
        index: usize,
        thinking: String,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        if !self.reasoning_started {
            self.reasoning_started = true;
            self.reasoning_done = false;
            send_event(
                tx_event,
                ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
                    id: self.message_id.clone().unwrap_or_default(),
                    summary: Vec::new(),
                    content: None,
                    encrypted_content: None,
                }),
            )
            .await?;
        }

        if self.reasoning_summary_started.insert(index, true).is_none() {
            send_event(
                tx_event,
                ResponseEvent::ReasoningSummaryPartAdded {
                    summary_index: index as i64,
                },
            )
            .await?;
        }

        self.reasoning_text
            .entry(index)
            .or_default()
            .push_str(&thinking);
        send_event(
            tx_event,
            ResponseEvent::ReasoningSummaryDelta {
                delta: thinking,
                summary_index: index as i64,
            },
        )
        .await
    }

    async fn finish_reasoning_if_needed(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        if !self.reasoning_started || self.reasoning_done {
            return Ok(());
        }
        self.reasoning_done = true;

        let text = self
            .reasoning_text
            .values()
            .map(String::as_str)
            .collect::<String>();
        let summary = if text.is_empty() {
            Vec::new()
        } else {
            vec![ReasoningItemReasoningSummary::SummaryText { text: text.clone() }]
        };
        let content = if text.is_empty() {
            None
        } else {
            Some(vec![ReasoningItemContent::ReasoningText { text }])
        };
        let raw_reasoning_blocks = self.reasoning_blocks.values().cloned().collect::<Vec<_>>();

        send_event(
            tx_event,
            ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                id: self.message_id.clone().unwrap_or_default(),
                summary,
                content,
                encrypted_content: encode_anthropic_reasoning_blocks(&raw_reasoning_blocks),
            }),
        )
        .await
    }

    async fn finish(&mut self, tx_event: &mpsc::Sender<Result<ResponseEvent>>) -> Result<()> {
        if self.completed {
            return Ok(());
        }
        self.completed = true;

        self.finish_reasoning_if_needed(tx_event).await?;
        if !self.message_started && !self.text.is_empty() {
            self.ensure_message_started(tx_event).await?;
        }

        if self.message_started {
            send_event(
                tx_event,
                ResponseEvent::OutputItemDone(ResponseItem::Message {
                    id: self.message_id.clone(),
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: self.text.clone(),
                    }],
                    end_turn: self.message_end_turn(),
                    phase: None,
                }),
            )
            .await?;
        }

        for (index, tool_use) in &self.tool_uses {
            let item = tool_use_to_response_item(*index, tool_use, &self.freeform_tool_names)?;
            send_event(tx_event, ResponseEvent::OutputItemDone(item)).await?;
        }
        for web_search in self.web_searches.values() {
            if let Some(id) = &web_search.id {
                send_event(
                    tx_event,
                    ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall {
                        id: Some(id.clone()),
                        status: Some(if web_search.completed {
                            "completed".to_string()
                        } else {
                            "in_progress".to_string()
                        }),
                        action: web_search.action.clone(),
                    }),
                )
                .await?;
            }
        }
        for image_generation in self.image_generations.values() {
            if let Some(id) = &image_generation.id {
                send_event(
                    tx_event,
                    ResponseEvent::OutputItemDone(ResponseItem::ImageGenerationCall {
                        id: id.clone(),
                        status: if image_generation.completed {
                            "completed".to_string()
                        } else {
                            "in_progress".to_string()
                        },
                        revised_prompt: image_generation
                            .revised_prompt
                            .clone()
                            .or_else(|| image_generation.prompt.clone()),
                        result: image_generation.result.clone().unwrap_or_default(),
                    }),
                )
                .await?;
            }
        }

        self.validate_stop_reason()?;

        send_event(
            tx_event,
            ResponseEvent::Completed {
                response_id: self
                    .response_id
                    .clone()
                    .unwrap_or_else(|| "anthropic-response".to_string()),
                token_usage: self.token_usage(),
            },
        )
        .await
    }

    fn message_end_turn(&self) -> Option<bool> {
        match self.stop_reason.as_deref() {
            Some("end_turn") => Some(true),
            Some("tool_use") => Some(false),
            _ => None,
        }
    }

    fn validate_stop_reason(&self) -> Result<()> {
        match self.stop_reason.as_deref() {
            None | Some("end_turn") | Some("tool_use") | Some("stop_sequence") => Ok(()),
            Some("max_tokens") => Err(CodexErr::InvalidRequest(
                "anthropic response hit max_tokens before completion; retry with a higher token budget"
                    .to_string(),
            )),
            Some("pause_turn") => Err(CodexErr::UnsupportedOperation(
                "anthropic pause_turn continuation is not implemented yet".to_string(),
            )),
            Some("refusal") => Err(CodexErr::InvalidRequest(
                "anthropic refused to continue this turn".to_string(),
            )),
            Some("model_context_window_exceeded") => Err(CodexErr::ContextWindowExceeded),
            Some(stop_reason) => Err(CodexErr::InvalidRequest(format!(
                "anthropic response stopped with unsupported stop_reason `{stop_reason}`"
            ))),
        }
    }

    fn token_usage(&self) -> Option<TokenUsage> {
        self.usage.as_ref().map(|usage| {
            let input_tokens = usage.input_tokens.unwrap_or_default() as i64;
            let cache_creation_input_tokens =
                usage.cache_creation_input_tokens.unwrap_or_default() as i64;
            let cache_read_input_tokens = usage.cache_read_input_tokens.unwrap_or_default() as i64;
            let output_tokens = usage.output_tokens as i64;
            TokenUsage {
                input_tokens,
                cached_input_tokens: cache_creation_input_tokens + cache_read_input_tokens,
                output_tokens,
                reasoning_output_tokens: 0,
                total_tokens: input_tokens
                    + cache_creation_input_tokens
                    + cache_read_input_tokens
                    + output_tokens,
            }
        })
    }
}

async fn send_event(
    tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    event: ResponseEvent,
) -> Result<()> {
    tx_event
        .send(Ok(event))
        .await
        .map_err(|_| CodexErr::TurnAborted)
}

trait ValueExt {
    fn as_str(&self) -> Option<&str>;
}

impl ValueExt for serde_json::Value {
    fn as_str(&self) -> Option<&str> {
        serde_json::Value::as_str(self)
    }
}

fn set_string_field(block: &mut serde_json::Value, key: &str, delta: &str) -> Result<()> {
    let object = block.as_object_mut().ok_or_else(|| {
        CodexErr::Stream(
            "anthropic content block must be an object".to_string(),
            None,
        )
    })?;
    let entry = object
        .entry(key.to_string())
        .or_insert_with(|| serde_json::Value::String(String::new()));
    match entry {
        serde_json::Value::String(text) => {
            text.push_str(delta);
            Ok(())
        }
        _ => Err(CodexErr::Stream(
            format!("anthropic content block field `{key}` must be a string"),
            None,
        )),
    }
}
