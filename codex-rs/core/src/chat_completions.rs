use crate::client_common::Prompt;
use crate::context::ChatFileToolGuidance;
use codex_api::ChatCompletionsApiRequest;
use codex_api::ChatContentPart;
use codex_api::ChatImageUrl;
use codex_api::ChatMessage;
use codex_api::ChatMessageContent;
use codex_api::ChatMessageRole;
use codex_api::ChatStreamOptions;
use codex_api::ChatToolCall;
use codex_api::ChatToolCallFunction;
use codex_api::ChatToolCallInfo as ApiChatToolCallInfo;
use codex_api::ChatToolCallKind as ApiChatToolCallKind;
use codex_api::ChatToolType;
use codex_config::config_toml::ChatFileToolMode;
use codex_context_fragments::ContextualUserFragment;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_tools::ChatToolCallInfo;
use codex_tools::ChatToolCallKind;
use codex_tools::chat_tool_name;
use codex_tools::create_tools_json_for_chat_completions;
use codex_utils_string::approx_token_count;
use codex_utils_string::truncate_middle_with_token_budget;
use serde_json::json;
use std::collections::HashMap;

const MAX_CHAT_CONTEXT_ITEM_TOKENS: usize = 1_000;
const MAX_CHAT_CONTEXT_ITEM_RENDER_TOKENS: usize = 900;
const MAX_CHAT_REQUEST_ITEM_TOKENS: usize = 10_000;
const MAX_CHAT_REQUEST_TOTAL_TOKENS: usize = 250_000;
const MAX_CHAT_MESSAGE_TOOL_CALLS: usize = 64;
const UNSUPPORTED_IMAGE_PLACEHOLDER: &str = "[unsupported image reference omitted]";

pub(crate) fn build_chat_completions_request(
    prompt: &Prompt,
    model_info: &ModelInfo,
    reasoning_effort: Option<ReasoningEffort>,
    service_tier: Option<String>,
) -> Result<ChatCompletionsApiRequest> {
    let codex_tools::ChatToolsJson {
        tools,
        tool_call_info: visible_tool_call_info,
    } = create_tools_json_for_chat_completions(&prompt.tools)?;
    let mut tool_call_info = visible_tool_call_info.clone();
    let hidden_tool_call_info =
        create_tools_json_for_chat_completions(&prompt.hidden_tools)?.tool_call_info;
    for info in hidden_tool_call_info {
        if !tool_call_info
            .iter()
            .any(|existing| existing.chat_name == info.chat_name)
        {
            tool_call_info.push(info);
        }
    }
    let tool_names = tool_names_by_identity(&tool_call_info);
    let mut messages = Vec::new();
    if !prompt.base_instructions.text.trim().is_empty() {
        messages.push(ChatMessage::text(
            ChatMessageRole::System,
            prompt.base_instructions.text.clone(),
        ));
    }
    if !matches!(prompt.chat_file_tool_mode, ChatFileToolMode::Legacy) {
        let guidance = dedicated_chat_guidance(prompt, &visible_tool_call_info)?;
        messages.push(ChatMessage::text(ChatMessageRole::Developer, guidance));
    }
    let mut pending_reasoning = String::new();

    for item in prompt.get_formatted_input_for_request(false) {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let Some(role) = chat_role(&role) else {
                    continue;
                };
                if role != ChatMessageRole::Assistant {
                    flush_pending_reasoning(&mut messages, &mut pending_reasoning);
                }
                let mut message = ChatMessage {
                    role,
                    content: Some(chat_content(content)),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    reasoning_content: None,
                };
                if role == ChatMessageRole::Assistant && !pending_reasoning.is_empty() {
                    message.reasoning_content = Some(std::mem::take(&mut pending_reasoning));
                }
                messages.push(message);
            }
            ResponseItem::Reasoning { content, .. } => {
                for item in content.into_iter().flatten() {
                    match item {
                        ReasoningItemContent::ReasoningText { text }
                        | ReasoningItemContent::Text { text } => pending_reasoning.push_str(&text),
                    }
                }
            }
            ResponseItem::FunctionCall {
                name,
                namespace,
                arguments,
                call_id,
                ..
            } => push_tool_call(
                &mut messages,
                ChatToolCall {
                    id: call_id,
                    kind: ChatToolType::Function,
                    function: ChatToolCallFunction {
                        name: mapped_chat_tool_name(
                            &tool_names,
                            namespace.as_deref(),
                            &name,
                            ChatToolCallKind::Function,
                        ),
                        arguments,
                    },
                },
                &mut pending_reasoning,
            ),
            ResponseItem::CustomToolCall {
                name,
                namespace,
                input,
                call_id,
                ..
            } => push_tool_call(
                &mut messages,
                ChatToolCall {
                    id: call_id,
                    kind: ChatToolType::Function,
                    function: ChatToolCallFunction {
                        name: mapped_chat_tool_name(
                            &tool_names,
                            namespace.as_deref(),
                            &name,
                            ChatToolCallKind::Custom,
                        ),
                        arguments: json!({"input": input}).to_string(),
                    },
                },
                &mut pending_reasoning,
            ),
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                execution,
                arguments,
                ..
            } if execution == "client" => push_tool_call(
                &mut messages,
                ChatToolCall {
                    id: call_id,
                    kind: ChatToolType::Function,
                    function: ChatToolCallFunction {
                        name: mapped_chat_tool_name(
                            &tool_names,
                            /*namespace*/ None,
                            "tool_search",
                            ChatToolCallKind::ToolSearch,
                        ),
                        arguments: arguments.to_string(),
                    },
                },
                &mut pending_reasoning,
            ),
            ResponseItem::LocalShellCall {
                id,
                call_id,
                action,
                ..
            } => {
                let call_id = call_id
                    .or_else(|| id.map(String::from))
                    .unwrap_or_else(|| "local_shell_call".to_string());
                push_tool_call(
                    &mut messages,
                    ChatToolCall {
                        id: call_id,
                        kind: ChatToolType::Function,
                        function: ChatToolCallFunction {
                            name: mapped_chat_tool_name(
                                &tool_names,
                                /*namespace*/ None,
                                "local_shell",
                                ChatToolCallKind::Function,
                            ),
                            arguments: serde_json::to_string(&action)?,
                        },
                    },
                    &mut pending_reasoning,
                );
            }
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            }
            | ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                flush_pending_reasoning(&mut messages, &mut pending_reasoning);
                messages.push(tool_result_message(call_id, output));
            }
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                status,
                tools,
                ..
            } => {
                flush_pending_reasoning(&mut messages, &mut pending_reasoning);
                messages.push(ChatMessage {
                    role: ChatMessageRole::Tool,
                    content: Some(ChatMessageContent::Text(bounded_chat_context_item(
                        serde_json::to_string(&tools).unwrap_or(status),
                    ))),
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call_id),
                    reasoning_content: None,
                });
            }
            ResponseItem::AgentMessage { .. } => {
                return Err(CodexErr::InvalidRequest(
                    "Chat Completions cannot serialize internal agent messages".to_string(),
                ));
            }
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::Other => {}
        }
    }
    flush_pending_reasoning(&mut messages, &mut pending_reasoning);

    if messages.is_empty()
        || messages.iter().all(|message| {
            matches!(
                message.role,
                ChatMessageRole::System | ChatMessageRole::Developer
            )
        })
    {
        return Err(CodexErr::InvalidRequest(
            "Chat Completions request has no model-visible messages".to_string(),
        ));
    }

    let tool_call_info = tool_call_info
        .into_iter()
        .map(|info| {
            (
                info.chat_name,
                ApiChatToolCallInfo {
                    name: info.name,
                    namespace: info.namespace,
                    kind: match info.kind {
                        ChatToolCallKind::Function => ApiChatToolCallKind::Function,
                        ChatToolCallKind::Custom => ApiChatToolCallKind::Custom,
                        ChatToolCallKind::ToolSearch => ApiChatToolCallKind::ToolSearch,
                    },
                },
            )
        })
        .collect();
    let has_tools = !tools.is_empty();
    let response_format = prompt.output_schema.as_ref().map(|schema| {
        json!({
            "type": "json_schema",
            "json_schema": {
                "name": "codex_output_schema",
                "strict": prompt.output_schema_strict,
                "schema": schema
            }
        })
    });

    let request = ChatCompletionsApiRequest {
        model: model_info.slug.clone(),
        messages,
        stream: true,
        stream_options: ChatStreamOptions {
            include_usage: true,
        },
        tools,
        tool_choice: has_tools.then(|| "auto".to_string()),
        parallel_tool_calls: has_tools.then_some(prompt.parallel_tool_calls),
        reasoning_effort,
        service_tier,
        response_format,
        tool_call_info,
    };
    validate_chat_request_context(&request)?;
    Ok(request)
}

fn dedicated_chat_guidance(prompt: &Prompt, tool_call_info: &[ChatToolCallInfo]) -> Result<String> {
    let mode = prompt.chat_file_tool_mode;
    if !prompt.dedicated_file_tools_enabled {
        return Err(CodexErr::InvalidRequest(format!(
            "Chat file tool mode `{mode:?}` requires the dedicated_file_tools gate; enable the gate or set chat_file_tool_mode = legacy"
        )));
    }
    let find_unique_first_party = |name: &str| {
        if !crate::tools::dedicated_file_tool_plan::has_first_party_dedicated_file_tool(
            &prompt.tools,
            name,
        ) {
            return None;
        }
        let matches = tool_call_info
            .iter()
            .filter(|info| {
                info.name == name
                    && info.namespace.is_none()
                    && info.kind == ChatToolCallKind::Function
            })
            .collect::<Vec<_>>();
        (matches.len() == 1).then(|| matches[0].chat_name.clone())
    };
    let Some(read_name) = find_unique_first_party("read_file") else {
        return Err(CodexErr::InvalidRequest(format!(
            "Chat file tool mode `{mode:?}` requires a unique first-party read_file tool in the request plan (gate on alone is not enough; same-named third-party tools are rejected)"
        )));
    };
    let Some(edit_name) = find_unique_first_party("edit_file") else {
        return Err(CodexErr::InvalidRequest(format!(
            "Chat file tool mode `{mode:?}` requires a unique first-party edit_file tool in the request plan (gate on alone is not enough; same-named third-party tools are rejected)"
        )));
    };
    let Some(write_name) = find_unique_first_party("write_file") else {
        return Err(CodexErr::InvalidRequest(format!(
            "Chat file tool mode `{mode:?}` requires a unique first-party write_file tool in the request plan (gate on alone is not enough; same-named third-party tools are rejected)"
        )));
    };
    let guidance = ChatFileToolGuidance::new(read_name, edit_name, write_name).render();
    if approx_token_count(&guidance) >= MAX_CHAT_CONTEXT_ITEM_TOKENS {
        return Err(CodexErr::InvalidRequest(
            "Chat dedicated file guidance exceeds the context-item limit".to_string(),
        ));
    }
    Ok(guidance)
}

fn validate_chat_request_context(request: &ChatCompletionsApiRequest) -> Result<()> {
    let mut total_tokens = 0usize;
    for (index, message) in request.messages.iter().enumerate() {
        if message.tool_calls.len() > MAX_CHAT_MESSAGE_TOOL_CALLS {
            return Err(CodexErr::InvalidRequest(format!(
                "Chat message {index} exceeds the {MAX_CHAT_MESSAGE_TOOL_CALLS} tool-call limit"
            )));
        }
        validate_chat_request_item(
            &format!("message {index}"),
            &serde_json::to_string(message)?,
            &mut total_tokens,
        )?;
    }
    for (index, tool) in request.tools.iter().enumerate() {
        validate_chat_request_item(
            &format!("tool {index}"),
            &serde_json::to_string(tool)?,
            &mut total_tokens,
        )?;
    }
    if let Some(response_format) = request.response_format.as_ref() {
        validate_chat_request_item(
            "response format",
            &serde_json::to_string(response_format)?,
            &mut total_tokens,
        )?;
    }
    Ok(())
}

fn validate_chat_request_item(
    item_kind: &str,
    serialized: &str,
    total_tokens: &mut usize,
) -> Result<()> {
    let item_tokens = approx_token_count(serialized);
    if item_tokens > MAX_CHAT_REQUEST_ITEM_TOKENS {
        return Err(CodexErr::InvalidRequest(format!(
            "Chat {item_kind} exceeds the {MAX_CHAT_REQUEST_ITEM_TOKENS}-token model-context limit"
        )));
    }
    *total_tokens = total_tokens.saturating_add(item_tokens);
    if *total_tokens > MAX_CHAT_REQUEST_TOTAL_TOKENS {
        return Err(CodexErr::InvalidRequest(format!(
            "Chat request exceeds the {MAX_CHAT_REQUEST_TOTAL_TOKENS}-token model-context limit"
        )));
    }
    Ok(())
}

fn chat_role(role: &str) -> Option<ChatMessageRole> {
    match role {
        "system" => Some(ChatMessageRole::System),
        "developer" => Some(ChatMessageRole::Developer),
        "user" => Some(ChatMessageRole::User),
        "assistant" => Some(ChatMessageRole::Assistant),
        _ => None,
    }
}

fn chat_content(content: Vec<ContentItem>) -> ChatMessageContent {
    let has_images = content
        .iter()
        .any(|item| matches!(item, ContentItem::InputImage { .. }));
    if !has_images {
        return ChatMessageContent::Text(
            content
                .into_iter()
                .filter_map(|item| match item {
                    ContentItem::InputText { text }
                    | ContentItem::OutputText { text }
                    | ContentItem::OutputTextWithCitations { text, .. } => Some(text),
                    ContentItem::InputImage { .. } => None,
                })
                .collect(),
        );
    }

    ChatMessageContent::Parts(
        content
            .into_iter()
            .map(|item| match item {
                ContentItem::InputText { text }
                | ContentItem::OutputText { text }
                | ContentItem::OutputTextWithCitations { text, .. } => {
                    ChatContentPart::Text { text }
                }
                ContentItem::InputImage { image_url, detail }
                    if is_supported_image_url(&image_url) =>
                {
                    ChatContentPart::ImageUrl {
                        image_url: ChatImageUrl {
                            url: image_url,
                            detail,
                        },
                    }
                }
                ContentItem::InputImage { .. } => ChatContentPart::Text {
                    text: UNSUPPORTED_IMAGE_PLACEHOLDER.to_string(),
                },
            })
            .collect(),
    )
}

fn tool_result_message(call_id: String, output: FunctionCallOutputPayload) -> ChatMessage {
    let content = match output.body {
        FunctionCallOutputBody::Text(text) => ChatMessageContent::Text(text),
        FunctionCallOutputBody::ContentItems(items) => ChatMessageContent::Parts(
            items
                .into_iter()
                .map(|item| match item {
                    FunctionCallOutputContentItem::InputText { text } => {
                        ChatContentPart::Text { text }
                    }
                    FunctionCallOutputContentItem::InputImage { image_url, detail }
                        if is_supported_image_url(&image_url) =>
                    {
                        ChatContentPart::ImageUrl {
                            image_url: ChatImageUrl {
                                url: image_url,
                                detail,
                            },
                        }
                    }
                    FunctionCallOutputContentItem::InputImage { .. } => ChatContentPart::Text {
                        text: UNSUPPORTED_IMAGE_PLACEHOLDER.to_string(),
                    },
                    FunctionCallOutputContentItem::EncryptedContent { .. } => {
                        ChatContentPart::Text {
                            text: "[encrypted tool output omitted]".to_string(),
                        }
                    }
                })
                .collect(),
        ),
    };
    ChatMessage {
        role: ChatMessageRole::Tool,
        content: Some(content),
        tool_calls: Vec::new(),
        tool_call_id: Some(call_id),
        reasoning_content: None,
    }
}

fn push_tool_call(
    messages: &mut Vec<ChatMessage>,
    tool_call: ChatToolCall,
    pending_reasoning: &mut String,
) {
    // Merge into the current assistant turn even when it already has text. Chat
    // providers reject back-to-back assistant messages for text-then-tool turns.
    if let Some(message) = messages.last_mut()
        && message.role == ChatMessageRole::Assistant
    {
        message.tool_calls.push(tool_call);
        if !pending_reasoning.is_empty() {
            message
                .reasoning_content
                .get_or_insert_default()
                .push_str(&std::mem::take(pending_reasoning));
        }
        return;
    }

    messages.push(ChatMessage {
        role: ChatMessageRole::Assistant,
        content: None,
        tool_calls: vec![tool_call],
        tool_call_id: None,
        reasoning_content: (!pending_reasoning.is_empty())
            .then(|| std::mem::take(pending_reasoning)),
    });
}

fn flush_pending_reasoning(messages: &mut Vec<ChatMessage>, pending_reasoning: &mut String) {
    if pending_reasoning.is_empty() {
        return;
    }
    messages.push(ChatMessage {
        role: ChatMessageRole::Assistant,
        content: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
        reasoning_content: Some(std::mem::take(pending_reasoning)),
    });
}

fn tool_names_by_identity(
    tool_call_info: &[ChatToolCallInfo],
) -> HashMap<(Option<String>, String, ChatToolCallKind), String> {
    tool_call_info
        .iter()
        .map(|info| {
            (
                (info.namespace.clone(), info.name.clone(), info.kind),
                info.chat_name.clone(),
            )
        })
        .collect()
}

fn mapped_chat_tool_name(
    tool_names: &HashMap<(Option<String>, String, ChatToolCallKind), String>,
    namespace: Option<&str>,
    name: &str,
    kind: ChatToolCallKind,
) -> String {
    tool_names
        .get(&(namespace.map(str::to_string), name.to_string(), kind))
        .cloned()
        .unwrap_or_else(|| chat_tool_name(namespace, name, kind))
}

fn bounded_chat_context_item(text: String) -> String {
    let bounded = truncate_middle_with_token_budget(&text, MAX_CHAT_CONTEXT_ITEM_RENDER_TOKENS).0;
    debug_assert!(approx_token_count(&bounded) <= MAX_CHAT_CONTEXT_ITEM_TOKENS);
    bounded
}

fn is_supported_image_url(url: &str) -> bool {
    url.starts_with("data:") || url.starts_with("https://") || url.starts_with("http://")
}

#[cfg(test)]
#[path = "chat_completions_tests.rs"]
mod tests;
