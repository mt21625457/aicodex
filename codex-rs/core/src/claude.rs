use crate::client_common::Prompt;
use codex_api::ClaudeContentBlock;
use codex_api::ClaudeImageSource;
use codex_api::ClaudeMessage;
use codex_api::ClaudeMessageRole;
use codex_api::ClaudeMessagesApiRequest;
use codex_api::ClaudeServiceTier;
use codex_api::ClaudeThinkingConfig;
use codex_api::ClaudeTool;
use codex_api::ClaudeToolCallInfo as ApiClaudeToolCallInfo;
use codex_api::ClaudeToolCallKind as ApiClaudeToolCallKind;
use codex_api::ClaudeToolChoice;
use codex_api::ClaudeToolResultContent;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_tools::ClaudeToolCallKind;
use codex_tools::claude_tool_name;
use codex_tools::create_tools_json_for_claude_messages;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;

const DEFAULT_MAX_TOKENS: u64 = 8_192;
const CLAUDE_THINKING_MIN_BUDGET_TOKENS: u32 = 1_024;
const CLAUDE_THINKING_MEDIUM_BUDGET_TOKENS: u32 = 2_048;
const CLAUDE_THINKING_HIGH_BUDGET_TOKENS: u32 = 4_096;
const CLAUDE_THINKING_XHIGH_BUDGET_TOKENS: u32 = 6_144;
const TOOL_INPUT_FIELD: &str = "input";
const OUTPUT_SCHEMA_INSTRUCTIONS: &str =
    "Respond with JSON only. It must strictly match this schema:";

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ClaudeRequestOptions {
    pub(crate) reasoning_effort: Option<ReasoningEffortConfig>,
    pub(crate) service_tier: Option<ServiceTier>,
}

pub(crate) fn build_claude_messages_request(
    prompt: &Prompt,
    model_info: &ModelInfo,
    options: ClaudeRequestOptions,
) -> codex_protocol::error::Result<ClaudeMessagesApiRequest> {
    let mut messages = Vec::new();
    let mut system_segments = vec![prompt.base_instructions.text.clone()];
    if let Some(output_schema) = &prompt.output_schema {
        system_segments.push(output_schema_instruction(output_schema));
    }
    let tools_json = create_tools_json_for_claude_messages(&prompt.tools)?;
    let codex_tools::ClaudeToolsJson {
        tools,
        tool_call_info: tool_call_metadata,
    } = tools_json;
    let tools = tools
        .into_iter()
        .map(serde_json::from_value::<ClaudeTool>)
        .collect::<Result<Vec<_>, _>>()?;
    let tool_names = tool_names_by_identity(&tool_call_metadata);

    for item in prompt.get_formatted_input() {
        match item {
            ResponseItem::Message { role, content, .. } => {
                if matches!(role.as_str(), "system" | "developer") {
                    let text = content_text(&content);
                    if !text.trim().is_empty() {
                        system_segments.push(text);
                    }
                } else if matches!(role.as_str(), "user" | "assistant") {
                    let role = if role == "user" {
                        ClaudeMessageRole::User
                    } else {
                        ClaudeMessageRole::Assistant
                    };
                    push_message(&mut messages, role, content_blocks(&content));
                }
            }
            ResponseItem::FunctionCall {
                name,
                namespace,
                call_id,
                arguments,
                ..
            } => {
                let claude_name = mapped_claude_tool_name(
                    &tool_names,
                    namespace.as_deref(),
                    &name,
                    ClaudeToolCallKind::Function,
                );
                push_message(
                    &mut messages,
                    ClaudeMessageRole::Assistant,
                    vec![tool_use_block(
                        &call_id,
                        &claude_name,
                        parse_json_object_or_wrapped(&arguments),
                    )],
                );
            }
            ResponseItem::CustomToolCall {
                name,
                call_id,
                input,
                ..
            } => {
                let claude_name = mapped_claude_tool_name(
                    &tool_names,
                    /*namespace*/ None,
                    &name,
                    ClaudeToolCallKind::Custom,
                );
                push_message(
                    &mut messages,
                    ClaudeMessageRole::Assistant,
                    vec![tool_use_block(
                        &call_id,
                        &claude_name,
                        json!({ TOOL_INPUT_FIELD: input }),
                    )],
                );
            }
            ResponseItem::LocalShellCall {
                id,
                call_id,
                action,
                ..
            } => {
                let call_id = call_id
                    .or(id)
                    .unwrap_or_else(|| "local_shell_call".to_string());
                push_message(
                    &mut messages,
                    ClaudeMessageRole::Assistant,
                    vec![tool_use_block(
                        &call_id,
                        &mapped_claude_tool_name(
                            &tool_names,
                            /*namespace*/ None,
                            "local_shell",
                            ClaudeToolCallKind::Function,
                        ),
                        local_shell_input(&action),
                    )],
                );
            }
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                execution,
                arguments,
                ..
            } if execution == "client" => {
                push_message(
                    &mut messages,
                    ClaudeMessageRole::Assistant,
                    vec![tool_use_block(
                        &call_id,
                        &mapped_claude_tool_name(
                            &tool_names,
                            /*namespace*/ None,
                            "tool_search",
                            ClaudeToolCallKind::ToolSearch,
                        ),
                        arguments,
                    )],
                );
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                push_message(
                    &mut messages,
                    ClaudeMessageRole::User,
                    vec![tool_result_block(
                        &call_id,
                        function_output_content(&output),
                        output.success == Some(false),
                    )],
                );
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                push_message(
                    &mut messages,
                    ClaudeMessageRole::User,
                    vec![tool_result_block(
                        &call_id,
                        function_output_content(&output),
                        output.success == Some(false),
                    )],
                );
            }
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                status,
                tools,
                ..
            } => {
                push_message(
                    &mut messages,
                    ClaudeMessageRole::User,
                    vec![tool_result_block(
                        &call_id,
                        ClaudeToolResultContent::Text(
                            serde_json::to_string(&tools).unwrap_or(status),
                        ),
                        false,
                    )],
                );
            }
            ResponseItem::Reasoning {
                content,
                encrypted_content,
                ..
            } => {
                if let Some(block) = thinking_block(content.as_deref(), encrypted_content.as_ref())
                {
                    push_message(&mut messages, ClaudeMessageRole::Assistant, vec![block]);
                }
            }
            ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::GhostSnapshot { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::Other => {}
        }
    }

    if messages.is_empty() {
        push_message(
            &mut messages,
            ClaudeMessageRole::User,
            vec![ClaudeContentBlock::Text {
                text: " ".to_string(),
            }],
        );
    }

    let tool_call_info = tool_call_metadata
        .into_iter()
        .map(|info| {
            (
                info.claude_name,
                ApiClaudeToolCallInfo {
                    name: info.name,
                    namespace: info.namespace,
                    kind: match info.kind {
                        ClaudeToolCallKind::Function => ApiClaudeToolCallKind::Function,
                        ClaudeToolCallKind::Custom => ApiClaudeToolCallKind::Custom,
                        ClaudeToolCallKind::ToolSearch => ApiClaudeToolCallKind::ToolSearch,
                    },
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let tool_choice = (!tools.is_empty()).then_some(ClaudeToolChoice::Auto {
        disable_parallel_tool_use: !prompt.parallel_tool_calls,
    });
    let system = system_segments
        .into_iter()
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(ClaudeMessagesApiRequest {
        model: model_info.slug.clone(),
        max_tokens: DEFAULT_MAX_TOKENS,
        messages,
        system: (!system.is_empty()).then_some(system),
        tools,
        tool_choice,
        thinking: claude_thinking_config(options.reasoning_effort),
        service_tier: claude_service_tier(options.service_tier),
        stream: true,
        tool_call_info,
    })
}

fn claude_thinking_config(
    reasoning_effort: Option<ReasoningEffortConfig>,
) -> Option<ClaudeThinkingConfig> {
    let budget_tokens = match reasoning_effort? {
        ReasoningEffortConfig::None => return None,
        ReasoningEffortConfig::Minimal | ReasoningEffortConfig::Low => {
            CLAUDE_THINKING_MIN_BUDGET_TOKENS
        }
        ReasoningEffortConfig::Medium => CLAUDE_THINKING_MEDIUM_BUDGET_TOKENS,
        ReasoningEffortConfig::High => CLAUDE_THINKING_HIGH_BUDGET_TOKENS,
        ReasoningEffortConfig::XHigh => CLAUDE_THINKING_XHIGH_BUDGET_TOKENS,
    };
    Some(ClaudeThinkingConfig::Enabled { budget_tokens })
}

fn claude_service_tier(service_tier: Option<ServiceTier>) -> Option<ClaudeServiceTier> {
    match service_tier {
        Some(ServiceTier::Fast) => Some(ClaudeServiceTier::Auto),
        Some(ServiceTier::Flex) => Some(ClaudeServiceTier::StandardOnly),
        None => None,
    }
}

fn push_message(
    messages: &mut Vec<ClaudeMessage>,
    role: ClaudeMessageRole,
    mut content: Vec<ClaudeContentBlock>,
) {
    if content.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut()
        && last.role == role
    {
        last.content.append(&mut content);
        return;
    }
    messages.push(ClaudeMessage { role, content });
}

fn tool_names_by_identity(
    tool_call_info: &[codex_tools::ClaudeToolCallInfo],
) -> HashMap<(Option<String>, String, ClaudeToolCallKind), String> {
    tool_call_info
        .iter()
        .map(|info| {
            (
                (info.namespace.clone(), info.name.clone(), info.kind),
                info.claude_name.clone(),
            )
        })
        .collect()
}

fn mapped_claude_tool_name(
    tool_names: &HashMap<(Option<String>, String, ClaudeToolCallKind), String>,
    namespace: Option<&str>,
    name: &str,
    kind: ClaudeToolCallKind,
) -> String {
    tool_names
        .get(&(namespace.map(str::to_string), name.to_string(), kind))
        .cloned()
        .unwrap_or_else(|| claude_tool_name(namespace, name))
}

fn output_schema_instruction(output_schema: &Value) -> String {
    let schema = serde_json::to_string(output_schema).unwrap_or_else(|_| output_schema.to_string());
    format!("{OUTPUT_SCHEMA_INSTRUCTIONS} {schema}")
}

fn content_text(content: &[ContentItem]) -> String {
    content
        .iter()
        .map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => text.clone(),
            ContentItem::InputImage { image_url, .. }
                if parse_base64_data_url(image_url).is_some() =>
            {
                "[image: data-url]".to_string()
            }
            ContentItem::InputImage { image_url, .. } => format!("[image: {image_url}]"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_blocks(content: &[ContentItem]) -> Vec<ClaudeContentBlock> {
    content
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text }
                if !text.is_empty() =>
            {
                Some(ClaudeContentBlock::Text { text: text.clone() })
            }
            ContentItem::InputImage { image_url, .. } => Some(image_content_block(image_url)),
            ContentItem::InputText { .. } | ContentItem::OutputText { .. } => None,
        })
        .collect()
}

fn parse_base64_data_url(image_url: &str) -> Option<(&str, &str)> {
    let rest = image_url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if data.trim().is_empty() {
        return None;
    }

    let mut parts = meta.split(';');
    let media_type = parts.next()?.trim();
    if media_type.is_empty() {
        return None;
    }
    let is_base64 = parts.any(|part| part.trim().eq_ignore_ascii_case("base64"));
    is_base64.then_some((media_type, data))
}

fn tool_use_block(call_id: &str, name: &str, input: Value) -> ClaudeContentBlock {
    ClaudeContentBlock::ToolUse {
        id: call_id.to_string(),
        name: name.to_string(),
        input,
    }
}

fn tool_result_block(
    call_id: &str,
    content: ClaudeToolResultContent,
    is_error: bool,
) -> ClaudeContentBlock {
    ClaudeContentBlock::ToolResult {
        tool_use_id: call_id.to_string(),
        content,
        is_error,
    }
}

fn thinking_block(
    content: Option<&[codex_protocol::models::ReasoningItemContent]>,
    encrypted_content: Option<&String>,
) -> Option<ClaudeContentBlock> {
    let thinking = content
        .unwrap_or_default()
        .iter()
        .map(|item| match item {
            codex_protocol::models::ReasoningItemContent::ReasoningText { text }
            | codex_protocol::models::ReasoningItemContent::Text { text } => text.as_str(),
        })
        .collect::<String>();
    let signature = encrypted_content.filter(|signature| !signature.trim().is_empty());
    if thinking.trim().is_empty() && signature.is_none() {
        return None;
    }

    Some(ClaudeContentBlock::Thinking {
        thinking,
        signature: signature.cloned(),
    })
}

fn function_output_content(output: &FunctionCallOutputPayload) -> ClaudeToolResultContent {
    match &output.body {
        FunctionCallOutputBody::Text(text) => ClaudeToolResultContent::Text(text.clone()),
        FunctionCallOutputBody::ContentItems(items) => {
            let blocks = function_output_content_blocks(items);
            if blocks.is_empty() {
                ClaudeToolResultContent::Text(output.to_string())
            } else {
                ClaudeToolResultContent::Blocks(blocks)
            }
        }
    }
}

fn function_output_content_blocks(
    items: &[FunctionCallOutputContentItem],
) -> Vec<ClaudeContentBlock> {
    items
        .iter()
        .filter_map(|item| match item {
            FunctionCallOutputContentItem::InputText { text } if !text.is_empty() => {
                Some(ClaudeContentBlock::Text { text: text.clone() })
            }
            FunctionCallOutputContentItem::InputImage { image_url, .. } => {
                Some(image_content_block(image_url))
            }
            FunctionCallOutputContentItem::InputText { .. } => None,
        })
        .collect()
}

fn image_content_block(image_url: &str) -> ClaudeContentBlock {
    if let Some((media_type, data)) = parse_base64_data_url(image_url) {
        ClaudeContentBlock::Image {
            source: ClaudeImageSource::Base64 {
                media_type: media_type.to_string(),
                data: data.to_string(),
            },
        }
    } else if is_http_url(image_url) {
        ClaudeContentBlock::Image {
            source: ClaudeImageSource::Url {
                url: image_url.to_string(),
            },
        }
    } else {
        ClaudeContentBlock::Text {
            text: format!("[image: {image_url}]"),
        }
    }
}

fn is_http_url(image_url: &str) -> bool {
    image_url.starts_with("http://") || image_url.starts_with("https://")
}

fn local_shell_input(action: &LocalShellAction) -> Value {
    match action {
        LocalShellAction::Exec(exec) => json!({
            "command": exec.command,
            "workdir": exec.working_directory,
            "timeout_ms": exec.timeout_ms
        }),
    }
}

fn parse_json_object_or_wrapped(input: &str) -> Value {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(object)) => Value::Object(object),
        Ok(Value::Null) => Value::Object(Map::new()),
        Ok(other) => json!({ TOOL_INPUT_FIELD: other }),
        Err(_) => json!({ TOOL_INPUT_FIELD: input }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::BaseInstructions;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ImageDetail;
    use codex_protocol::models::ReasoningItemContent;
    use codex_tools::AdditionalProperties;
    use codex_tools::JsonSchema;
    use codex_tools::ResponsesApiNamespace;
    use codex_tools::ResponsesApiNamespaceTool;
    use codex_tools::ResponsesApiTool;
    use codex_tools::ToolSpec;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;

    fn model_info() -> ModelInfo {
        serde_json::from_value(json!({
            "slug": "claude-sonnet-4-5",
            "display_name": "Claude Sonnet",
            "description": "desc",
            "default_reasoning_level": null,
            "supported_reasoning_levels": [],
            "shell_type": "local",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 1,
            "upgrade": null,
            "base_instructions": "base instructions",
            "model_messages": null,
            "supports_reasoning_summaries": false,
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10000},
            "supports_parallel_tool_calls": false,
            "supports_image_detail_original": false,
            "context_window": 200000,
            "auto_compact_token_limit": null,
            "experimental_supported_tools": []
        }))
        .expect("deserialize model info")
    }

    #[test]
    fn builds_claude_request_with_system_images_and_namespace_tools() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![
                    ContentItem::InputText {
                        text: "look".to_string(),
                    },
                    ContentItem::InputImage {
                        image_url: "data:image/png;base64,abcd".to_string(),
                        detail: Some(ImageDetail::High),
                    },
                ],
                end_turn: None,
                phase: None,
            }],
            tools: vec![ToolSpec::Namespace(ResponsesApiNamespace {
                name: "mcp__demo__".to_string(),
                description: "Demo tools".to_string(),
                tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                    name: "search".to_string(),
                    description: "Search".to_string(),
                    strict: false,
                    defer_loading: None,
                    parameters: JsonSchema::object(
                        BTreeMap::from([(
                            "query".to_string(),
                            JsonSchema::string(/*description*/ None),
                        )]),
                        Some(vec!["query".to_string()]),
                        Some(AdditionalProperties::Boolean(false)),
                    ),
                    output_schema: None,
                })],
            })],
            base_instructions: BaseInstructions {
                text: "be useful".to_string(),
            },
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request"),
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": DEFAULT_MAX_TOKENS,
                "messages": [{
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "look"
                        },
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "abcd"
                            }
                        }
                    ]
                }],
                "system": "be useful",
                "tools": [{
                    "name": "mcp__demo__search",
                    "description": "Demo tools\n\nSearch",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        },
                        "required": ["query"],
                        "additionalProperties": false
                    }
                }],
                "tool_choice": {
                    "type": "auto",
                    "disable_parallel_tool_use": true
                },
                "stream": true
            })
        );
        assert!(request.tool_call_info.contains_key("mcp__demo__search"));
    }

    #[test]
    fn builds_claude_tool_result_history() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{\"id\":1}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
            ],
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            request.messages,
            vec![
                ClaudeMessage {
                    role: ClaudeMessageRole::Assistant,
                    content: vec![ClaudeContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "lookup".to_string(),
                        input: json!({ "id": 1 }),
                    }],
                },
                ClaudeMessage {
                    role: ClaudeMessageRole::User,
                    content: vec![ClaudeContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: ClaudeToolResultContent::Text("ok".to_string()),
                        is_error: false,
                    }],
                },
            ]
        );
    }

    #[test]
    fn builds_claude_error_tool_result_history() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{\"id\":1}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload {
                        body: FunctionCallOutputBody::Text("city not found".to_string()),
                        success: Some(false),
                    },
                },
            ],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request"),
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": DEFAULT_MAX_TOKENS,
                "messages": [
                    {
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "lookup",
                            "input": {"id": 1}
                        }]
                    },
                    {
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": "call_1",
                            "content": "city not found",
                            "is_error": true
                        }]
                    }
                ],
                "stream": true
            })
        );
    }

    #[test]
    fn builds_claude_url_image_and_structured_tool_result_blocks() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputImage {
                        image_url: "https://example.com/screenshot.png".to_string(),
                        detail: Some(ImageDetail::High),
                    }],
                    end_turn: None,
                    phase: None,
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "inspect".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_content_items(vec![
                        FunctionCallOutputContentItem::InputText {
                            text: "see image".to_string(),
                        },
                        FunctionCallOutputContentItem::InputImage {
                            image_url: "data:image/png;base64,Zm9v".to_string(),
                            detail: Some(ImageDetail::High),
                        },
                        FunctionCallOutputContentItem::InputImage {
                            image_url: "https://example.com/tool-output.png".to_string(),
                            detail: Some(ImageDetail::High),
                        },
                    ]),
                },
            ],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request"),
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": DEFAULT_MAX_TOKENS,
                "messages": [
                    {
                        "role": "user",
                        "content": [{
                            "type": "image",
                            "source": {
                                "type": "url",
                                "url": "https://example.com/screenshot.png"
                            }
                        }]
                    },
                    {
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "inspect",
                            "input": {}
                        }]
                    },
                    {
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": "call_1",
                            "content": [
                                {
                                    "type": "text",
                                    "text": "see image"
                                },
                                {
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": "image/png",
                                        "data": "Zm9v"
                                    }
                                },
                                {
                                    "type": "image",
                                    "source": {
                                        "type": "url",
                                        "url": "https://example.com/tool-output.png"
                                    }
                                }
                            ]
                        }]
                    }
                ],
                "stream": true
            })
        );
    }

    #[test]
    fn builds_claude_unsupported_image_reference_as_text_placeholder() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputImage {
                    image_url: "file:///tmp/screenshot.png".to_string(),
                    detail: Some(ImageDetail::High),
                }],
                end_turn: None,
                phase: None,
            }],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            request.messages,
            vec![ClaudeMessage {
                role: ClaudeMessageRole::User,
                content: vec![ClaudeContentBlock::Text {
                    text: "[image: file:///tmp/screenshot.png]".to_string(),
                }],
            }]
        );
    }

    #[test]
    fn builds_claude_request_with_thinking_and_service_tier() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "think".to_string(),
                }],
                end_turn: None,
                phase: None,
            }],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                reasoning_effort: Some(ReasoningEffortConfig::High),
                service_tier: Some(ServiceTier::Fast),
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request"),
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": DEFAULT_MAX_TOKENS,
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "think"
                    }]
                }],
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": CLAUDE_THINKING_HIGH_BUDGET_TOKENS
                },
                "service_tier": "auto",
                "stream": true
            })
        );
    }

    #[test]
    fn builds_claude_request_with_flex_service_tier() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "standard".to_string(),
                }],
                end_turn: None,
                phase: None,
            }],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                reasoning_effort: None,
                service_tier: Some(ServiceTier::Flex),
            },
        )
        .expect("request");

        assert_eq!(request.service_tier, Some(ClaudeServiceTier::StandardOnly));
        assert_eq!(
            serde_json::to_value(&request).expect("serialize request")["service_tier"],
            json!("standard_only")
        );
    }

    #[test]
    fn builds_claude_history_with_reasoning_signature_and_colliding_tool_name() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Reasoning {
                    id: "reasoning_1".to_string(),
                    summary: Vec::new(),
                    content: Some(vec![ReasoningItemContent::ReasoningText {
                        text: "thinking".to_string(),
                    }]),
                    encrypted_content: Some("signature".to_string()),
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "a/b".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
            ],
            tools: vec![
                ToolSpec::Function(ResponsesApiTool {
                    name: "a.b".to_string(),
                    description: "Dot".to_string(),
                    strict: false,
                    defer_loading: None,
                    parameters: JsonSchema::object(
                        BTreeMap::new(),
                        /*required*/ None,
                        /*additional_properties*/ None,
                    ),
                    output_schema: None,
                }),
                ToolSpec::Function(ResponsesApiTool {
                    name: "a/b".to_string(),
                    description: "Slash".to_string(),
                    strict: false,
                    defer_loading: None,
                    parameters: JsonSchema::object(
                        BTreeMap::new(),
                        /*required*/ None,
                        /*additional_properties*/ None,
                    ),
                    output_schema: None,
                }),
            ],
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        let expected_tool_name = request.tool_call_info["a_b"].name.clone();
        assert_eq!(expected_tool_name, "a.b");
        let colliding_name = request
            .tool_call_info
            .iter()
            .find_map(|(claude_name, info)| (info.name == "a/b").then_some(claude_name.clone()))
            .expect("colliding tool mapping");
        assert_eq!(
            request.messages,
            vec![ClaudeMessage {
                role: ClaudeMessageRole::Assistant,
                content: vec![
                    ClaudeContentBlock::Thinking {
                        thinking: "thinking".to_string(),
                        signature: Some("signature".to_string()),
                    },
                    ClaudeContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: colliding_name,
                        input: json!({}),
                    },
                ],
            }]
        );
    }
}
