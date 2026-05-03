use crate::error::ApiError;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::config_types::Verbosity as VerbosityConfig;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::ModelVerification;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::W3cTraceContext;
use futures::Stream;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio::sync::mpsc;

pub const WS_REQUEST_HEADER_TRACEPARENT_CLIENT_METADATA_KEY: &str = "ws_request_header_traceparent";
pub const WS_REQUEST_HEADER_TRACESTATE_CLIENT_METADATA_KEY: &str = "ws_request_header_tracestate";

/// Canonical input payload for the compaction endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct CompactionInput<'a> {
    pub model: &'a str,
    pub input: &'a [ResponseItem],
    #[serde(skip_serializing_if = "str::is_empty")]
    pub instructions: &'a str,
    pub tools: Vec<Value>,
    pub parallel_tool_calls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextControls>,
}

/// Canonical input payload for the memory summarize endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct MemorySummarizeInput {
    pub model: String,
    #[serde(rename = "traces")]
    pub raw_memories: Vec<RawMemory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawMemory {
    pub id: String,
    pub metadata: RawMemoryMetadata,
    pub items: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawMemoryMetadata {
    pub source_path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MemorySummarizeOutput {
    #[serde(rename = "trace_summary", alias = "raw_memory")]
    pub raw_memory: String,
    pub memory_summary: String,
}

#[derive(Debug)]
pub enum ResponseEvent {
    Created,
    OutputItemDone(ResponseItem),
    OutputItemAdded(ResponseItem),
    /// Emitted when the server includes `OpenAI-Model` on the stream response.
    /// This can differ from the requested model when backend safety routing applies.
    ServerModel(String),
    /// Emitted when the server recommends additional account verification.
    ModelVerifications(Vec<ModelVerification>),
    /// Emitted when `X-Reasoning-Included: true` is present on the response,
    /// meaning the server already accounted for past reasoning tokens and the
    /// client should not re-estimate them.
    ServerReasoningIncluded(bool),
    Completed {
        response_id: String,
        token_usage: Option<TokenUsage>,
        /// Did the model affirmatively end its turn? Some providers do not set this,
        /// so we rely on fallback logic when this is `None`.
        end_turn: Option<bool>,
        /// Provider-native stop reason when the wire API exposes one.
        provider_stop_reason: Option<String>,
    },
    OutputTextDelta(String),
    ToolCallInputDelta {
        item_id: String,
        call_id: Option<String>,
        delta: String,
    },
    ReasoningSummaryDelta {
        delta: String,
        summary_index: i64,
    },
    ReasoningContentDelta {
        delta: String,
        content_index: i64,
    },
    ReasoningSummaryPartAdded {
        summary_index: i64,
    },
    RateLimits(RateLimitSnapshot),
    ModelsEtag(String),
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Reasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffortConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummaryConfig>,
}

#[derive(Debug, Serialize, Default, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TextFormatType {
    #[default]
    JsonSchema,
}

#[derive(Debug, Serialize, Default, Clone, PartialEq)]
pub struct TextFormat {
    /// Format type used by the OpenAI text controls.
    pub r#type: TextFormatType,
    /// When true, the server is expected to strictly validate responses.
    pub strict: bool,
    /// JSON schema for the desired output.
    pub schema: Value,
    /// Friendly name for the format, used in telemetry/debugging.
    pub name: String,
}

/// Controls the `text` field for the Responses API, combining verbosity and
/// optional JSON schema output formatting.
#[derive(Debug, Serialize, Default, Clone, PartialEq)]
pub struct TextControls {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<OpenAiVerbosity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
}

#[derive(Debug, Serialize, Default, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiVerbosity {
    Low,
    #[default]
    Medium,
    High,
}

impl From<VerbosityConfig> for OpenAiVerbosity {
    fn from(v: VerbosityConfig) -> Self {
        match v {
            VerbosityConfig::Low => OpenAiVerbosity::Low,
            VerbosityConfig::Medium => OpenAiVerbosity::Medium,
            VerbosityConfig::High => OpenAiVerbosity::High,
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ResponsesApiRequest {
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<serde_json::Value>,
    pub tool_choice: String,
    pub parallel_tool_calls: bool,
    pub reasoning: Option<Reasoning>,
    pub store: bool,
    pub stream: bool,
    pub include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextControls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeToolCallKind {
    Function,
    Custom,
    ToolSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeToolCallInfo {
    pub name: String,
    pub namespace: Option<String>,
    pub kind: ClaudeToolCallKind,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaudeMessagesApiRequest {
    pub model: String,
    pub max_tokens: u64,
    pub messages: Vec<ClaudeMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<ClaudeSystemPrompt>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ClaudeTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ClaudeToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ClaudeThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<ClaudeOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ClaudeServiceTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_management: Option<ClaudeContextManagement>,
    pub stream: bool,
    #[serde(skip)]
    pub tool_call_info: HashMap<String, ClaudeToolCallInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeCountTokensRequest {
    pub model: String,
    pub messages: Vec<ClaudeMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<ClaudeSystemPrompt>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ClaudeTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ClaudeToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ClaudeThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<ClaudeOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ClaudeServiceTier>,
}

impl From<&ClaudeMessagesApiRequest> for ClaudeCountTokensRequest {
    fn from(request: &ClaudeMessagesApiRequest) -> Self {
        Self {
            model: request.model.clone(),
            messages: request.messages.clone(),
            system: request.system.clone(),
            tools: request.tools.clone(),
            tool_choice: request.tool_choice.clone(),
            thinking: request.thinking.clone(),
            output_config: request.output_config,
            service_tier: request.service_tier,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ClaudeCountTokensResponse {
    pub input_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ClaudeSystemPrompt {
    Text(String),
    Blocks(Vec<ClaudeContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeMessage {
    pub role: ClaudeMessageRole,
    pub content: Vec<ClaudeContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeMessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeContentBlock {
    Text {
        text: String,
        cache_control: Option<ClaudeCacheControl>,
    },
    Image {
        source: ClaudeImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: ClaudeToolResultContent,
        is_error: bool,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    ProviderState {
        value: Value,
    },
}

impl Serialize for ClaudeContentBlock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ClaudeContentBlock::Text {
                text,
                cache_control,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "text")?;
                map.serialize_entry("text", text)?;
                if let Some(cache_control) = cache_control {
                    map.serialize_entry("cache_control", cache_control)?;
                }
                map.end()
            }
            ClaudeContentBlock::Image { source } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "image")?;
                map.serialize_entry("source", source)?;
                map.end()
            }
            ClaudeContentBlock::ToolUse { id, name, input } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("type", "tool_use")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("input", input)?;
                map.end()
            }
            ClaudeContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "tool_result")?;
                map.serialize_entry("tool_use_id", tool_use_id)?;
                map.serialize_entry("content", content)?;
                if *is_error {
                    map.serialize_entry("is_error", is_error)?;
                }
                map.end()
            }
            ClaudeContentBlock::Thinking {
                thinking,
                signature,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "thinking")?;
                map.serialize_entry("thinking", thinking)?;
                if let Some(signature) = signature {
                    map.serialize_entry("signature", signature)?;
                }
                map.end()
            }
            ClaudeContentBlock::ProviderState { value } => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ClaudeContentBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        deserialize_claude_content_block(value).map_err(serde::de::Error::custom)
    }
}

fn deserialize_claude_content_block(value: Value) -> Result<ClaudeContentBlock, String> {
    let Some(object) = value.as_object() else {
        return Err("Claude content block must be a JSON object".to_string());
    };
    let Some(block_type) = object.get("type").and_then(Value::as_str) else {
        return Err("Claude content block is missing type".to_string());
    };

    match block_type {
        "text" => Ok(ClaudeContentBlock::Text {
            text: object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            cache_control: object
                .get("cache_control")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|err| err.to_string())?,
        }),
        "image" => Ok(ClaudeContentBlock::Image {
            source: object
                .get("source")
                .cloned()
                .ok_or_else(|| "Claude image block is missing source".to_string())
                .and_then(|source| serde_json::from_value(source).map_err(|err| err.to_string()))?,
        }),
        "tool_use" => Ok(ClaudeContentBlock::ToolUse {
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
                .unwrap_or_else(|| Value::Object(Default::default())),
        }),
        "tool_result" => Ok(ClaudeContentBlock::ToolResult {
            tool_use_id: object
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            content: object
                .get("content")
                .cloned()
                .ok_or_else(|| "Claude tool_result block is missing content".to_string())
                .and_then(|content| {
                    serde_json::from_value(content).map_err(|err| err.to_string())
                })?,
            is_error: object
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
        "thinking" => Ok(ClaudeContentBlock::Thinking {
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
        _ => Ok(ClaudeContentBlock::ProviderState { value }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ClaudeToolResultContent {
    Text(String),
    Blocks(Vec<ClaudeContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<ClaudeCacheControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeCacheControl {
    #[serde(rename = "type")]
    pub kind: ClaudeCacheControlType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<ClaudeCacheTtl>,
}

impl ClaudeCacheControl {
    pub fn ephemeral(ttl: Option<ClaudeCacheTtl>) -> Self {
        Self {
            kind: ClaudeCacheControlType::Ephemeral,
            ttl,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeCacheControlType {
    Ephemeral,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClaudeCacheTtl {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeContextManagement {
    pub edits: Vec<ClaudeContextManagementEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeContextManagementEdit {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeToolChoice {
    Auto {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        disable_parallel_tool_use: bool,
    },
    Any {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        disable_parallel_tool_use: bool,
    },
    Tool {
        name: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        disable_parallel_tool_use: bool,
    },
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeThinkingConfig {
    Enabled { budget_tokens: u32 },
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeOutputConfig {
    pub effort: ClaudeOutputEffort,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeOutputEffort {
    High,
    Max,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeServiceTier {
    Auto,
    StandardOnly,
}

impl From<&ResponsesApiRequest> for ResponseCreateWsRequest {
    fn from(request: &ResponsesApiRequest) -> Self {
        Self {
            model: request.model.clone(),
            instructions: request.instructions.clone(),
            previous_response_id: None,
            input: request.input.clone(),
            tools: request.tools.clone(),
            tool_choice: request.tool_choice.clone(),
            parallel_tool_calls: request.parallel_tool_calls,
            reasoning: request.reasoning.clone(),
            store: request.store,
            stream: request.stream,
            include: request.include.clone(),
            service_tier: request.service_tier.clone(),
            prompt_cache_key: request.prompt_cache_key.clone(),
            text: request.text.clone(),
            generate: None,
            client_metadata: request.client_metadata.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ResponseCreateWsRequest {
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<Value>,
    pub tool_choice: String,
    pub parallel_tool_calls: bool,
    pub reasoning: Option<Reasoning>,
    pub store: bool,
    pub stream: bool,
    pub include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextControls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_metadata: Option<HashMap<String, String>>,
}

pub fn response_create_client_metadata(
    client_metadata: Option<HashMap<String, String>>,
    trace: Option<&W3cTraceContext>,
) -> Option<HashMap<String, String>> {
    let mut client_metadata = client_metadata.unwrap_or_default();

    if let Some(traceparent) = trace.and_then(|trace| trace.traceparent.as_deref()) {
        client_metadata.insert(
            WS_REQUEST_HEADER_TRACEPARENT_CLIENT_METADATA_KEY.to_string(),
            traceparent.to_string(),
        );
    }
    if let Some(tracestate) = trace.and_then(|trace| trace.tracestate.as_deref()) {
        client_metadata.insert(
            WS_REQUEST_HEADER_TRACESTATE_CLIENT_METADATA_KEY.to_string(),
            tracestate.to_string(),
        );
    }

    (!client_metadata.is_empty()).then_some(client_metadata)
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum ResponsesWsRequest {
    #[serde(rename = "response.create")]
    ResponseCreate(ResponseCreateWsRequest),
}

pub fn create_text_param_for_request(
    verbosity: Option<VerbosityConfig>,
    output_schema: &Option<Value>,
    output_schema_strict: bool,
) -> Option<TextControls> {
    if verbosity.is_none() && output_schema.is_none() {
        return None;
    }

    Some(TextControls {
        verbosity: verbosity.map(std::convert::Into::into),
        format: output_schema.as_ref().map(|schema| TextFormat {
            r#type: TextFormatType::JsonSchema,
            strict: output_schema_strict,
            schema: schema.clone(),
            name: "codex_output_schema".to_string(),
        }),
    })
}

pub struct ResponseStream {
    pub rx_event: mpsc::Receiver<Result<ResponseEvent, ApiError>>,
    /// Server-assigned `x-request-id` response header, when present.
    pub upstream_request_id: Option<String>,
}

impl Stream for ResponseStream {
    type Item = Result<ResponseEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx_event.poll_recv(cx)
    }
}

#[cfg(test)]
mod claude_wire_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    fn roundtrip<T>(value: &T, expected: Value)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(value).expect("serialize"), expected);
        assert_eq!(
            serde_json::from_value::<T>(expected).expect("deserialize"),
            *value
        );
    }

    #[test]
    fn claude_content_blocks_roundtrip_messages_api_shapes() {
        roundtrip(
            &ClaudeContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            },
            json!({"type": "text", "text": "hello"}),
        );
        roundtrip(
            &ClaudeContentBlock::Image {
                source: ClaudeImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "YmFzZTY0".to_string(),
                },
            },
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": "YmFzZTY0"
                }
            }),
        );
        roundtrip(
            &ClaudeContentBlock::Image {
                source: ClaudeImageSource::Url {
                    url: "https://example.com/a.png".to_string(),
                },
            },
            json!({
                "type": "image",
                "source": {
                    "type": "url",
                    "url": "https://example.com/a.png"
                }
            }),
        );
        roundtrip(
            &ClaudeContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "get_weather".to_string(),
                input: json!({"city": "Paris"}),
            },
            json!({
                "type": "tool_use",
                "id": "toolu_1",
                "name": "get_weather",
                "input": {"city": "Paris"}
            }),
        );
        roundtrip(
            &ClaudeContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: ClaudeToolResultContent::Text("sunny".to_string()),
                is_error: false,
            },
            json!({
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": "sunny"
            }),
        );
        roundtrip(
            &ClaudeContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: ClaudeToolResultContent::Text("boom".to_string()),
                is_error: true,
            },
            json!({
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "is_error": true,
                "content": "boom"
            }),
        );
        roundtrip(
            &ClaudeContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: ClaudeToolResultContent::Blocks(vec![ClaudeContentBlock::Text {
                    text: "inner".to_string(),
                    cache_control: None,
                }]),
                is_error: false,
            },
            json!({
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": [{"type": "text", "text": "inner"}]
            }),
        );
        roundtrip(
            &ClaudeContentBlock::Thinking {
                thinking: "musing".to_string(),
                signature: Some("sig".to_string()),
            },
            json!({
                "type": "thinking",
                "thinking": "musing",
                "signature": "sig"
            }),
        );
    }

    #[test]
    fn claude_tool_choice_thinking_and_service_tier_roundtrip() {
        roundtrip(
            &ClaudeToolChoice::Auto {
                disable_parallel_tool_use: false,
            },
            json!({"type": "auto"}),
        );
        roundtrip(
            &ClaudeToolChoice::Any {
                disable_parallel_tool_use: true,
            },
            json!({"type": "any", "disable_parallel_tool_use": true}),
        );
        roundtrip(
            &ClaudeToolChoice::Tool {
                name: "get_weather".to_string(),
                disable_parallel_tool_use: false,
            },
            json!({"type": "tool", "name": "get_weather"}),
        );
        roundtrip(&ClaudeToolChoice::None, json!({"type": "none"}));
        roundtrip(
            &ClaudeThinkingConfig::Enabled {
                budget_tokens: 1024,
            },
            json!({"type": "enabled", "budget_tokens": 1024}),
        );
        roundtrip(&ClaudeThinkingConfig::Disabled, json!({"type": "disabled"}));
        roundtrip(
            &ClaudeOutputConfig {
                effort: ClaudeOutputEffort::Max,
            },
            json!({"effort": "max"}),
        );
        roundtrip(&ClaudeServiceTier::StandardOnly, json!("standard_only"));
    }

    #[test]
    fn claude_messages_request_skips_none_and_side_table_fields() {
        let mut tool_call_info = HashMap::new();
        tool_call_info.insert(
            "get_weather".to_string(),
            ClaudeToolCallInfo {
                name: "get_weather".to_string(),
                namespace: None,
                kind: ClaudeToolCallKind::Function,
            },
        );
        let request = ClaudeMessagesApiRequest {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 128,
            messages: vec![ClaudeMessage {
                role: ClaudeMessageRole::User,
                content: vec![ClaudeContentBlock::Text {
                    text: "hi".to_string(),
                    cache_control: None,
                }],
            }],
            system: None,
            tools: Vec::new(),
            tool_choice: None,
            thinking: None,
            output_config: None,
            service_tier: None,
            context_management: None,
            stream: true,
            tool_call_info,
        };

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request"),
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 128,
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "hi"}]
                }],
                "stream": true
            })
        );
    }
}
