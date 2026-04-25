use anthropic_sdk::types::messages::MessageContent;
use anthropic_sdk::types::messages::MessageCreateParams;
use anthropic_sdk::types::messages::MessageParam;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use codex_protocol::openai_models::ReasoningEffort;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

use crate::dto::AnthropicTurnRequest;
use crate::tool_mapping::anthropic_tool_result_message;
use crate::tool_mapping::anthropic_tool_use_message;
use crate::tool_mapping::build_anthropic_tools;
use crate::tool_mapping::image_url_to_anthropic_block;
use crate::tool_mapping::parse_json_object_or_wrapped;

const DEFAULT_MAX_TOKENS: u64 = 8_192;
const DEFAULT_THINKING_RESPONSE_TOKENS: u64 = 4_096;
const MAX_THINKING_RESPONSE_TOKENS: u64 = 64_000;
const OUTPUT_SCHEMA_INSTRUCTION: &str =
    "Respond with JSON only. It must strictly match this schema:";
const ANTHROPIC_REASONING_ENVELOPE_PROVIDER: &str = "anthropic";
const ANTHROPIC_REASONING_ENVELOPE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicReasoningEnvelope {
    provider: String,
    version: u32,
    blocks: Vec<Value>,
}

pub(crate) fn build_request(request: &AnthropicTurnRequest) -> Result<MessageCreateParams> {
    validate_request_controls(request)?;

    let mut system_segments = vec![request.base_instructions.text.clone()];
    if let Some(output_schema) = &request.output_schema {
        let schema =
            serde_json::to_string(output_schema).unwrap_or_else(|_| output_schema.to_string());
        system_segments.push(format!("{OUTPUT_SCHEMA_INSTRUCTION} {schema}"));
    }

    let mut messages = Vec::<MessageParam>::new();
    let mut pending_role: Option<&str> = None;
    let mut pending_blocks = Vec::<Value>::new();
    for item in &request.input {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let blocks = content_blocks(content)?;
                if blocks.is_empty() {
                    continue;
                }
                match role.as_str() {
                    "system" | "developer" => {
                        flush_pending_message(
                            &mut messages,
                            &mut pending_role,
                            &mut pending_blocks,
                        );
                        let text = content_text(content)?;
                        if !text.trim().is_empty() {
                            system_segments.push(text);
                        }
                    }
                    "user" => push_blocks(
                        &mut messages,
                        &mut pending_role,
                        &mut pending_blocks,
                        "user",
                        blocks,
                    ),
                    "assistant" => push_blocks(
                        &mut messages,
                        &mut pending_role,
                        &mut pending_blocks,
                        "assistant",
                        blocks,
                    ),
                    other => {
                        return Err(CodexErr::UnsupportedOperation(format!(
                            "anthropic does not support message role `{other}`"
                        )));
                    }
                }
            }
            ResponseItem::Reasoning {
                content,
                summary,
                encrypted_content,
                ..
            } => {
                let blocks = anthropic_reasoning_blocks(content, summary, encrypted_content)?;
                if !blocks.is_empty() {
                    push_blocks(
                        &mut messages,
                        &mut pending_role,
                        &mut pending_blocks,
                        "assistant",
                        blocks,
                    );
                }
            }
            ResponseItem::FunctionCall {
                name,
                call_id,
                arguments,
                ..
            } => {
                push_block(
                    &mut messages,
                    &mut pending_role,
                    &mut pending_blocks,
                    "assistant",
                    tool_use_block(
                        name.clone(),
                        call_id.clone(),
                        parse_json_object_or_wrapped(arguments),
                    ),
                );
            }
            ResponseItem::CustomToolCall {
                name,
                call_id,
                input,
                ..
            } => {
                push_block(
                    &mut messages,
                    &mut pending_role,
                    &mut pending_blocks,
                    "assistant",
                    tool_use_block(
                        name.clone(),
                        call_id.clone(),
                        parse_json_object_or_wrapped(input),
                    ),
                );
            }
            ResponseItem::LocalShellCall {
                id,
                call_id,
                action,
                ..
            } => {
                let resolved_call_id = call_id
                    .clone()
                    .or(id.clone())
                    .unwrap_or_else(|| "local_shell_call".to_string());
                push_block(
                    &mut messages,
                    &mut pending_role,
                    &mut pending_blocks,
                    "assistant",
                    tool_use_block(
                        "local_shell".to_string(),
                        resolved_call_id,
                        local_shell_input(action),
                    ),
                );
            }
            ResponseItem::ToolSearchCall {
                call_id,
                execution,
                arguments,
                ..
            } => {
                let Some(call_id) = call_id.clone() else {
                    return Err(CodexErr::InvalidRequest(
                        "anthropic tool_search history is missing call_id".to_string(),
                    ));
                };
                if execution != "client" {
                    return Err(CodexErr::UnsupportedOperation(format!(
                        "anthropic does not support tool_search execution mode `{execution}`"
                    )));
                }
                push_block(
                    &mut messages,
                    &mut pending_role,
                    &mut pending_blocks,
                    "assistant",
                    tool_use_block("tool_search".to_string(), call_id, arguments.clone()),
                );
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                flush_pending_message(&mut messages, &mut pending_role, &mut pending_blocks);
                let message = anthropic_tool_result_message(
                    call_id.clone(),
                    output,
                    output.success == Some(false),
                )?;
                messages.push(message);
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                flush_pending_message(&mut messages, &mut pending_role, &mut pending_blocks);
                let message = anthropic_tool_result_message(
                    call_id.clone(),
                    output,
                    output.success == Some(false),
                )?;
                messages.push(message);
            }
            ResponseItem::ToolSearchOutput {
                call_id,
                status,
                execution,
                tools,
            } => {
                let Some(call_id) = call_id.clone() else {
                    return Err(CodexErr::InvalidRequest(
                        "anthropic tool_search output is missing call_id".to_string(),
                    ));
                };
                flush_pending_message(&mut messages, &mut pending_role, &mut pending_blocks);
                let message = anthropic_tool_result_message(
                    call_id,
                    &tool_search_output_payload(status, execution, tools),
                    /*is_error*/ false,
                )?;
                messages.push(message);
            }
            ResponseItem::WebSearchCall { id, status, action } => {
                let Some(id) = id.clone() else {
                    return Err(CodexErr::InvalidRequest(
                        "anthropic web_search history is missing id".to_string(),
                    ));
                };
                push_block(
                    &mut messages,
                    &mut pending_role,
                    &mut pending_blocks,
                    "assistant",
                    json!({
                        "type": "server_tool_use",
                        "id": id,
                        "name": "web_search",
                        "input": web_search_input(action),
                    }),
                );
                if status.as_deref() == Some("completed") {
                    push_block(
                        &mut messages,
                        &mut pending_role,
                        &mut pending_blocks,
                        "assistant",
                        json!({
                            "type": "web_search_tool_result",
                            "tool_use_id": id,
                            "content": [],
                        }),
                    );
                }
            }
            ResponseItem::ImageGenerationCall {
                id,
                status,
                revised_prompt,
                result,
            } => {
                let mut input = serde_json::Map::new();
                if let Some(revised_prompt) = revised_prompt {
                    input.insert("prompt".to_string(), Value::String(revised_prompt.clone()));
                }
                push_block(
                    &mut messages,
                    &mut pending_role,
                    &mut pending_blocks,
                    "assistant",
                    json!({
                        "type": "server_tool_use",
                        "id": id,
                        "name": "image_generation",
                        "input": input,
                    }),
                );
                if status == "completed" {
                    let mut result_block = json!({
                        "type": "image_generation_tool_result",
                        "tool_use_id": id,
                        "result": result,
                    });
                    if let Some(revised_prompt) = revised_prompt
                        && let Some(object) = result_block.as_object_mut()
                    {
                        object.insert(
                            "revised_prompt".to_string(),
                            Value::String(revised_prompt.clone()),
                        );
                    }
                    if !result.is_empty()
                        && let Some(object) = result_block.as_object_mut()
                    {
                        object.insert(
                            "content".to_string(),
                            Value::Array(vec![image_url_to_anthropic_block(&format!(
                                "data:image/png;base64,{result}"
                            ))?]),
                        );
                    }
                    push_block(
                        &mut messages,
                        &mut pending_role,
                        &mut pending_blocks,
                        "assistant",
                        result_block,
                    );
                }
            }
            ResponseItem::GhostSnapshot { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::Other => {}
        }
    }
    flush_pending_message(&mut messages, &mut pending_role, &mut pending_blocks);

    if messages.is_empty() {
        messages.push(MessageParam::user(String::new()));
    }

    let mut extra = BTreeMap::<String, Value>::new();
    let system = system_segments
        .into_iter()
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system.is_empty() {
        extra.insert("system".to_string(), Value::String(system));
    }

    let tools = build_anthropic_tools(&request.tools)?;
    if !tools.is_empty() {
        extra.insert("tools".to_string(), Value::Array(tools));
        if !request.parallel_tool_calls {
            extra.insert(
                "tool_choice".to_string(),
                json!({
                    "type": "auto",
                    "disable_parallel_tool_use": true,
                }),
            );
        }
    }

    if let Some(thinking) = anthropic_thinking(request) {
        extra.insert("thinking".to_string(), thinking);
    }
    if let Some(service_tier) = anthropic_service_tier(request.service_tier) {
        extra.insert(
            "service_tier".to_string(),
            Value::String(service_tier.to_string()),
        );
    }

    Ok(MessageCreateParams {
        model: request.model.clone(),
        max_tokens: max_tokens_for_request(request),
        messages,
        stream: Some(true),
        extra,
    })
}

pub(crate) fn encode_anthropic_reasoning_blocks(blocks: &[Value]) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }

    serde_json::to_string(&AnthropicReasoningEnvelope {
        provider: ANTHROPIC_REASONING_ENVELOPE_PROVIDER.to_string(),
        version: ANTHROPIC_REASONING_ENVELOPE_VERSION,
        blocks: blocks.to_vec(),
    })
    .ok()
}

pub(crate) fn decode_anthropic_reasoning_blocks(encrypted_content: &str) -> Option<Vec<Value>> {
    let envelope = serde_json::from_str::<AnthropicReasoningEnvelope>(encrypted_content).ok()?;
    if envelope.provider != ANTHROPIC_REASONING_ENVELOPE_PROVIDER
        || envelope.version != ANTHROPIC_REASONING_ENVELOPE_VERSION
    {
        return None;
    }
    Some(envelope.blocks)
}

fn validate_request_controls(request: &AnthropicTurnRequest) -> Result<()> {
    let _ = &request.turn_metadata_header;
    Ok(())
}

fn anthropic_service_tier(service_tier: Option<ServiceTier>) -> Option<&'static str> {
    match service_tier {
        Some(ServiceTier::Fast) => Some("auto"),
        Some(ServiceTier::Flex) => Some("standard_only"),
        None => None,
    }
}

fn anthropic_thinking(request: &AnthropicTurnRequest) -> Option<Value> {
    let budget_tokens = thinking_budget_tokens(request.effort?)?;
    let display = match request.summary {
        ReasoningSummary::None => "omitted",
        ReasoningSummary::Auto | ReasoningSummary::Concise | ReasoningSummary::Detailed => {
            "summarized"
        }
    };
    Some(json!({
        "type": "enabled",
        "budget_tokens": budget_tokens,
        "display": display,
    }))
}

fn max_tokens_for_request(request: &AnthropicTurnRequest) -> u64 {
    match request.effort.and_then(thinking_budget_tokens) {
        Some(budget_tokens) => budget_tokens
            .saturating_add(DEFAULT_THINKING_RESPONSE_TOKENS)
            .min(MAX_THINKING_RESPONSE_TOKENS),
        None => DEFAULT_MAX_TOKENS,
    }
}

fn thinking_budget_tokens(effort: ReasoningEffort) -> Option<u64> {
    match effort {
        ReasoningEffort::None => None,
        ReasoningEffort::Minimal => Some(1_024),
        ReasoningEffort::Low => Some(2_048),
        ReasoningEffort::Medium => Some(4_096),
        ReasoningEffort::High => Some(8_192),
        ReasoningEffort::XHigh => Some(16_384),
    }
}

fn anthropic_reasoning_blocks(
    content: &Option<Vec<codex_protocol::models::ReasoningItemContent>>,
    summary: &[codex_protocol::models::ReasoningItemReasoningSummary],
    encrypted_content: &Option<String>,
) -> Result<Vec<Value>> {
    if let Some(encrypted_content) = encrypted_content
        && let Some(blocks) = decode_anthropic_reasoning_blocks(encrypted_content)
    {
        return Ok(blocks);
    }

    if content.as_ref().is_some_and(|entries| !entries.is_empty()) || !summary.is_empty() {
        return Err(CodexErr::UnsupportedOperation(
            "anthropic reasoning history requires raw thinking blocks".to_string(),
        ));
    }

    Ok(Vec::new())
}

fn content_blocks(content: &[ContentItem]) -> Result<Vec<Value>> {
    content
        .iter()
        .map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => Ok(json!({
                "type": "text",
                "text": text,
            })),
            ContentItem::InputImage { image_url } => image_url_to_anthropic_block(image_url),
        })
        .collect()
}

fn content_text(content: &[ContentItem]) -> Result<String> {
    let mut text_parts = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                text_parts.push(text.clone());
            }
            ContentItem::InputImage { .. } => {
                return Err(CodexErr::UnsupportedOperation(
                    "anthropic system prompts do not support image content".to_string(),
                ));
            }
        }
    }
    Ok(text_parts.join("\n"))
}

fn tool_use_block(name: String, call_id: String, input: Value) -> Value {
    match anthropic_tool_use_message(name, call_id, input).content {
        MessageContent::Blocks(mut blocks) => blocks.remove(0),
        MessageContent::Text(_) => unreachable!(),
    }
}

fn push_block(
    messages: &mut Vec<MessageParam>,
    pending_role: &mut Option<&str>,
    pending_blocks: &mut Vec<Value>,
    target_role: &'static str,
    block: Value,
) {
    push_blocks(
        messages,
        pending_role,
        pending_blocks,
        target_role,
        vec![block],
    );
}

fn push_blocks(
    messages: &mut Vec<MessageParam>,
    pending_role: &mut Option<&str>,
    pending_blocks: &mut Vec<Value>,
    target_role: &'static str,
    blocks: Vec<Value>,
) {
    if pending_role.is_some_and(|role| role != target_role) {
        flush_pending_message(messages, pending_role, pending_blocks);
    }
    *pending_role = Some(target_role);
    pending_blocks.extend(blocks);
}

fn flush_pending_message(
    messages: &mut Vec<MessageParam>,
    pending_role: &mut Option<&str>,
    pending_blocks: &mut Vec<Value>,
) {
    let Some(role) = *pending_role else {
        return;
    };
    if pending_blocks.is_empty() {
        *pending_role = None;
        return;
    }

    messages.push(MessageParam {
        role: role.to_string(),
        content: MessageContent::Blocks(std::mem::take(pending_blocks)),
    });
    *pending_role = None;
}

fn local_shell_input(action: &LocalShellAction) -> Value {
    match action {
        LocalShellAction::Exec(exec) => json!({
            "command": exec.command,
            "workdir": exec.working_directory,
            "timeout_ms": exec.timeout_ms,
        }),
    }
}

fn web_search_input(action: &Option<WebSearchAction>) -> Value {
    match action {
        Some(WebSearchAction::Search { query, queries }) => {
            if let Some(query) = query {
                json!({ "query": query })
            } else if let Some(queries) = queries {
                json!({ "queries": queries })
            } else {
                json!({})
            }
        }
        Some(WebSearchAction::OpenPage { url }) => json!({ "url": url }),
        Some(WebSearchAction::FindInPage { url, pattern }) => json!({
            "url": url,
            "pattern": pattern,
        }),
        Some(WebSearchAction::Other) | None => json!({}),
    }
}

fn tool_search_output_payload(
    status: &str,
    execution: &str,
    tools: &[Value],
) -> FunctionCallOutputPayload {
    FunctionCallOutputPayload::from_text(
        json!({
            "status": status,
            "execution": execution,
            "tools": tools,
        })
        .to_string(),
    )
}
