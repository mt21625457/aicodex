use crate::client_common::Prompt;
use crate::client_common::is_claude_reasoning_item_id;
use codex_api::ClaudeCacheControl;
use codex_api::ClaudeCacheEdit;
use codex_api::ClaudeCacheTtl;
use codex_api::ClaudeContentBlock;
use codex_api::ClaudeContextManagement;
use codex_api::ClaudeImageSource;
use codex_api::ClaudeMcpServer as ApiClaudeMcpServer;
use codex_api::ClaudeMessage;
use codex_api::ClaudeMessageRole;
use codex_api::ClaudeMessagesApiRequest;
use codex_api::ClaudeOutputConfig;
use codex_api::ClaudeOutputEffort;
use codex_api::ClaudeServiceTier;
use codex_api::ClaudeSystemPrompt;
use codex_api::ClaudeThinkingConfig;
use codex_api::ClaudeTool;
use codex_api::ClaudeToolCallInfo as ApiClaudeToolCallInfo;
use codex_api::ClaudeToolCallKind as ApiClaudeToolCallKind;
use codex_api::ClaudeToolChoice;
use codex_api::ClaudeToolResultContent;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::error::CodexErr;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_tools::ClaudeHistoryRequirements;
use codex_tools::ClaudeLocalExecutorCapability;
use codex_tools::ClaudeMessagesToolOptions;
use codex_tools::ClaudeNativeToolKind;
use codex_tools::ClaudeNativeToolSelection;
use codex_tools::ClaudeProviderPlatform;
use codex_tools::ClaudeToolCallKind;
use codex_tools::ClaudeWebSearchToolKind;
use codex_tools::claude_tool_name;
use codex_tools::create_tools_json_for_claude_messages_with_options;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use tracing::debug;
use tracing::trace;
use url::Url;

const DEFAULT_MAX_TOKENS: u64 = 8_192;
const CLAUDE_THINKING_MIN_BUDGET_TOKENS: u32 = 1_024;
const CLAUDE_THINKING_MEDIUM_BUDGET_TOKENS: u32 = 2_048;
const CLAUDE_THINKING_HIGH_BUDGET_TOKENS: u32 = 4_096;
const CLAUDE_THINKING_XHIGH_BUDGET_TOKENS: u32 = 6_144;
const CLAUDE_MAX_MEDIA_PER_REQUEST: usize = 100;
const NON_TEXT_ERROR_TOOL_RESULT_PLACEHOLDER: &str = "[Non-text tool error content omitted]";
const PRUNED_TOOL_RESULT_MEDIA_PLACEHOLDER: &str =
    "[Media content omitted to stay within Claude request limits]";
const SYNTHETIC_TOOL_RESULT_PLACEHOLDER: &str = "[Tool result missing due to internal error]";
const INTERRUPTED_TOOL_USE_PLACEHOLDER: &str = "[Tool use interrupted]";
const TOOL_INPUT_FIELD: &str = "input";
const OUTPUT_SCHEMA_INSTRUCTIONS: &str =
    "Respond with JSON only. It must strictly match this schema:";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ClaudeToolHistoryRepairStats {
    duplicate_tool_uses_dropped: usize,
    duplicate_tool_results_dropped: usize,
    orphan_tool_results_dropped: usize,
    synthetic_tool_results_inserted: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ClaudeNormalizationStats {
    empty_text_blocks_dropped: usize,
    empty_cached_text_blocks_padded: usize,
    empty_messages_dropped: usize,
    error_tool_result_empty_text_replaced: usize,
    error_tool_result_blocks_flattened: usize,
    error_tool_result_nested_blocks_dropped: usize,
    unsigned_trailing_thinking_blocks_dropped: usize,
    unsigned_orphan_thinking_messages_dropped: usize,
    provider_state_dropped: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ClaudeMediaPruningStats {
    media_blocks_before: usize,
    media_blocks_removed: usize,
    tool_result_placeholders_inserted: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ClaudePromptCacheStats {
    tool_cache_controls_added: usize,
    system_cache_controls_added: usize,
    conversation_cache_controls_added: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ClaudeCacheEditingStats {
    pinned_cache_edit_blocks_inserted: usize,
    pinned_cache_edits_inserted: usize,
    new_cache_edit_blocks_inserted: usize,
    new_cache_edits_inserted: usize,
    duplicate_or_empty_delete_references_dropped: usize,
    cache_references_added: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ClaudeRequestOptions {
    pub(crate) reasoning_effort: Option<ReasoningEffortConfig>,
    pub(crate) service_tier: Option<ServiceTier>,
    pub(crate) prompt_cache: ClaudePromptCacheOptions,
    pub(crate) cache_editing: ClaudeCacheEditingOptions,
    pub(crate) context_management: Option<ClaudeContextManagement>,
    pub(crate) provider_compat: ClaudeProviderCompat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ClaudeProviderCompat {
    #[default]
    Anthropic,
    DeepSeek,
}

pub(crate) fn provider_compat_for_base_url(base_url: Option<&str>) -> ClaudeProviderCompat {
    let Some(base_url) = base_url else {
        return ClaudeProviderCompat::Anthropic;
    };
    let Ok(url) = Url::parse(base_url) else {
        return ClaudeProviderCompat::Anthropic;
    };
    if url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.deepseek.com"))
        && matches!(
            url.path().trim_end_matches('/'),
            "/anthropic" | "/anthropic/v1"
        )
    {
        ClaudeProviderCompat::DeepSeek
    } else {
        ClaudeProviderCompat::Anthropic
    }
}

pub(crate) fn provider_compat_for_provider(
    provider_name: &str,
    base_url: Option<&str>,
    model_slug: Option<&str>,
) -> ClaudeProviderCompat {
    if provider_compat_for_base_url(base_url) == ClaudeProviderCompat::DeepSeek
        || looks_like_deepseek_identifier(provider_name)
        || model_slug.is_some_and(looks_like_deepseek_identifier)
    {
        ClaudeProviderCompat::DeepSeek
    } else {
        ClaudeProviderCompat::Anthropic
    }
}

pub(crate) fn cache_editing_options_for_provider(
    provider_compat: ClaudeProviderCompat,
) -> ClaudeCacheEditingOptions {
    ClaudeCacheEditingOptions {
        capability: match provider_compat {
            ClaudeProviderCompat::Anthropic => ClaudeCacheEditingCapability::Enabled,
            ClaudeProviderCompat::DeepSeek => ClaudeCacheEditingCapability::Disabled,
        },
        ..Default::default()
    }
}

fn looks_like_deepseek_identifier(value: &str) -> bool {
    value
        .to_ascii_lowercase()
        .replace([' ', '-', '_'], "")
        .contains("deepseek")
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ClaudePromptCacheOptions {
    pub(crate) mode: ClaudePromptCacheMode,
    pub(crate) ttl: Option<ClaudeCacheTtl>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ClaudePromptCacheMode {
    #[default]
    Off,
    System,
    Conversation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClaudeCacheEditingOptions {
    pub(crate) capability: ClaudeCacheEditingCapability,
    pub(crate) new_delete_references: Vec<String>,
    pub(crate) pinned_deletes: Vec<ClaudePinnedCacheEdits>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ClaudeCacheEditingCapability {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudePinnedCacheEdits {
    pub(crate) user_message_index: usize,
    pub(crate) delete_references: Vec<String>,
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
    let tools_json = create_tools_json_for_claude_messages_with_options(
        &prompt.tools,
        ClaudeMessagesToolOptions {
            web_search_tool_kind: match options.provider_compat {
                ClaudeProviderCompat::Anthropic => ClaudeWebSearchToolKind::NativeServerTool,
                ClaudeProviderCompat::DeepSeek => ClaudeWebSearchToolKind::LocalFunctionTool,
            },
            native_tool_selection: native_tool_selection_for_prompt(
                &prompt.tools,
                model_info,
                options.provider_compat,
            ),
        },
    )?;
    let codex_tools::ClaudeToolsJson {
        tools,
        tool_call_info: tool_call_metadata,
        mcp_servers,
        beta_headers,
        history_requirements,
        ..
    } = tools_json;
    let mut tools = tools
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
                id,
                content,
                encrypted_content,
                ..
            } => {
                if let Some(block) =
                    thinking_block(&id, content.as_deref(), encrypted_content.as_ref())
                {
                    push_message(&mut messages, ClaudeMessageRole::Assistant, vec![block]);
                }
            }
            ResponseItem::Compaction { encrypted_content } => {
                if let Some(block) = provider_state_block(&encrypted_content) {
                    push_message(&mut messages, ClaudeMessageRole::Assistant, vec![block]);
                }
            }
            ResponseItem::ContextCompaction { encrypted_content } => {
                if let Some(encrypted_content) = encrypted_content
                    && let Some(block) = provider_state_block(&encrypted_content)
                {
                    push_message(&mut messages, ClaudeMessageRole::Assistant, vec![block]);
                }
            }
            ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::CompactionTrigger
            | ResponseItem::Other => {}
        }
    }

    let (repaired_messages, repair_stats) = repair_tool_result_history(messages);
    let (normalized_messages, first_normalization_stats) =
        normalize_claude_messages_with_stats(repaired_messages, &history_requirements);
    messages = normalized_messages;
    let media_pruning_stats =
        prune_excess_claude_media(&mut messages, CLAUDE_MAX_MEDIA_PER_REQUEST);
    let (normalized_messages, second_normalization_stats) =
        normalize_claude_messages_with_stats(messages, &history_requirements);
    messages = normalized_messages;

    if messages.is_empty() {
        push_message(
            &mut messages,
            ClaudeMessageRole::User,
            vec![text_block(" ")],
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
    let prompt_cache_mode = options.prompt_cache.mode;
    let cache_editing_capability = options.cache_editing.capability;
    let prompt_cache_stats =
        apply_prompt_cache_policy(options.prompt_cache, &mut tools, &system, &mut messages);
    let cache_editing_stats = apply_cache_editing_policy(options.cache_editing, &mut messages);
    debug!(
        provider_compat = ?options.provider_compat,
        prompt_cache_mode = ?prompt_cache_mode,
        cache_editing_capability = ?cache_editing_capability,
        message_count = messages.len(),
        tool_count = tools.len(),
        mcp_server_count = mcp_servers.len(),
        media_blocks_before = media_pruning_stats.media_blocks_before,
        media_blocks_removed = media_pruning_stats.media_blocks_removed,
        cache_references_added = cache_editing_stats.cache_references_added,
        cache_edit_blocks_inserted = cache_editing_stats.pinned_cache_edit_blocks_inserted
            + cache_editing_stats.new_cache_edit_blocks_inserted,
        cache_edits_inserted = cache_editing_stats.pinned_cache_edits_inserted
            + cache_editing_stats.new_cache_edits_inserted,
        synthetic_tool_results_inserted = repair_stats.synthetic_tool_results_inserted,
        provider_state_dropped = first_normalization_stats.provider_state_dropped
            + second_normalization_stats.provider_state_dropped,
        "Claude protocol processing counters"
    );
    trace!(
        ?repair_stats,
        ?first_normalization_stats,
        ?media_pruning_stats,
        ?second_normalization_stats,
        ?prompt_cache_stats,
        ?cache_editing_stats,
        "Claude protocol processing detailed counters"
    );
    validate_tool_result_history(&messages)?;
    let (thinking, output_config) = match options.provider_compat {
        ClaudeProviderCompat::Anthropic => (claude_thinking_config(options.reasoning_effort), None),
        ClaudeProviderCompat::DeepSeek => {
            let thinking = match options.reasoning_effort {
                Some(ReasoningEffortConfig::None) => Some(ClaudeThinkingConfig::Disabled),
                Some(_) => claude_thinking_config(options.reasoning_effort),
                None => None,
            };
            let output_config = match options.reasoning_effort {
                Some(ReasoningEffortConfig::None) | None => None,
                Some(
                    ReasoningEffortConfig::Minimal
                    | ReasoningEffortConfig::Low
                    | ReasoningEffortConfig::Medium
                    | ReasoningEffortConfig::High,
                ) => Some(ClaudeOutputConfig {
                    effort: ClaudeOutputEffort::High,
                }),
                Some(ReasoningEffortConfig::XHigh) => Some(ClaudeOutputConfig {
                    effort: ClaudeOutputEffort::Max,
                }),
            };
            (thinking, output_config)
        }
    };

    Ok(ClaudeMessagesApiRequest {
        model: model_info.slug.clone(),
        max_tokens: DEFAULT_MAX_TOKENS,
        messages,
        system: system_prompt(system, options.prompt_cache),
        tools,
        mcp_servers: mcp_servers.into_iter().map(api_claude_mcp_server).collect(),
        tool_choice,
        thinking,
        output_config,
        service_tier: claude_service_tier(options.service_tier),
        context_management: options.context_management,
        stream: true,
        tool_call_info,
        beta_headers: beta_headers
            .into_iter()
            .map(|feature| feature.header_value().to_string())
            .collect(),
    })
}

fn native_tool_selection_for_prompt(
    tools: &[codex_tools::ToolSpec],
    model_info: &ModelInfo,
    provider_compat: ClaudeProviderCompat,
) -> ClaudeNativeToolSelection {
    let provider_platform = match provider_compat {
        ClaudeProviderCompat::Anthropic => ClaudeProviderPlatform::AnthropicApi,
        ClaudeProviderCompat::DeepSeek => ClaudeProviderPlatform::DeepSeekCompatible,
    };
    let mut selection = ClaudeNativeToolSelection {
        provider_platform,
        model: Some(model_info.slug.clone()),
        ..ClaudeNativeToolSelection::default()
    };

    if provider_compat == ClaudeProviderCompat::Anthropic && has_tool_spec(tools, "apply_patch") {
        selection.allowed_tools.extend([
            ClaudeNativeToolKind::TextEditor20250728,
            ClaudeNativeToolKind::TextEditor20250124,
        ]);
        selection.text_editor_executor = ClaudeLocalExecutorCapability::Available;
    }

    selection
}

fn has_tool_spec(tools: &[codex_tools::ToolSpec], name: &str) -> bool {
    tools.iter().any(|tool| match tool {
        codex_tools::ToolSpec::Function(tool) => tool.name == name,
        codex_tools::ToolSpec::Freeform(tool) => tool.name == name,
        codex_tools::ToolSpec::ToolSearch { .. } => name == "tool_search",
        codex_tools::ToolSpec::WebSearch { .. } | codex_tools::ToolSpec::ImageGeneration { .. } => {
            false
        }
        codex_tools::ToolSpec::Namespace(namespace) => {
            namespace.tools.iter().any(|tool| match tool {
                codex_tools::ResponsesApiNamespaceTool::Function(tool) => tool.name == name,
            })
        }
    })
}

fn api_claude_mcp_server(server: codex_tools::ClaudeMcpServer) -> ApiClaudeMcpServer {
    ApiClaudeMcpServer {
        kind: "url".to_string(),
        name: server.name,
        url: server.url,
        authorization_token: server.authorization_token,
    }
}

fn apply_prompt_cache_policy(
    options: ClaudePromptCacheOptions,
    tools: &mut [ClaudeTool],
    system: &str,
    messages: &mut [ClaudeMessage],
) -> ClaudePromptCacheStats {
    let mut stats = ClaudePromptCacheStats::default();
    if options.mode == ClaudePromptCacheMode::Off {
        return stats;
    }
    let cache_control = Some(ClaudeCacheControl::ephemeral(options.ttl));
    if !tools.is_empty()
        && let Some(last_tool) = tools.last_mut()
    {
        last_tool.set_cache_control(cache_control.clone());
        stats.tool_cache_controls_added += 1;
    }
    if !system.trim().is_empty() && options.mode == ClaudePromptCacheMode::System {
        stats.system_cache_controls_added += 1;
        return stats;
    }
    if options.mode == ClaudePromptCacheMode::Conversation {
        stats.conversation_cache_controls_added +=
            mark_latest_stable_prior_text_block(messages, cache_control);
    }
    stats
}

fn apply_cache_editing_policy(
    options: ClaudeCacheEditingOptions,
    messages: &mut [ClaudeMessage],
) -> ClaudeCacheEditingStats {
    let mut stats = ClaudeCacheEditingStats::default();
    if options.capability == ClaudeCacheEditingCapability::Disabled {
        return stats;
    }

    let mut seen_delete_references = HashSet::new();
    for pinned in options.pinned_deletes {
        let (edits, dropped) =
            cache_delete_edits(pinned.delete_references, &mut seen_delete_references);
        stats.duplicate_or_empty_delete_references_dropped += dropped;
        if edits.is_empty() {
            continue;
        }
        let edit_count = edits.len();
        if let Some(message) = messages.get_mut(pinned.user_message_index)
            && message.role == ClaudeMessageRole::User
        {
            insert_cache_edits_after_tool_results(&mut message.content, edits);
            stats.pinned_cache_edit_blocks_inserted += 1;
            stats.pinned_cache_edits_inserted += edit_count;
        }
    }

    let (edits, dropped) =
        cache_delete_edits(options.new_delete_references, &mut seen_delete_references);
    stats.duplicate_or_empty_delete_references_dropped += dropped;
    if !edits.is_empty()
        && let Some(message) = messages
            .iter_mut()
            .rev()
            .find(|message| message.role == ClaudeMessageRole::User)
    {
        let edit_count = edits.len();
        insert_cache_edits_after_tool_results(&mut message.content, edits);
        stats.new_cache_edit_blocks_inserted += 1;
        stats.new_cache_edits_inserted += edit_count;
    }

    stats.cache_references_added = mark_cached_prefix_tool_results(messages);
    stats
}

fn cache_delete_edits(
    references: Vec<String>,
    seen: &mut HashSet<String>,
) -> (Vec<ClaudeCacheEdit>, usize) {
    let mut edits = Vec::new();
    let mut dropped = 0;
    for reference in references {
        let reference = reference.trim().to_string();
        if reference.is_empty() || !seen.insert(reference.clone()) {
            dropped += 1;
            continue;
        }
        edits.push(ClaudeCacheEdit::delete(reference));
    }
    (edits, dropped)
}

fn insert_cache_edits_after_tool_results(
    content: &mut Vec<ClaudeContentBlock>,
    edits: Vec<ClaudeCacheEdit>,
) {
    let insert_at = content
        .iter()
        .position(|block| !is_tool_result_block(block))
        .unwrap_or(content.len());
    content.insert(insert_at, ClaudeContentBlock::CacheEdits { edits });
}

fn mark_cached_prefix_tool_results(messages: &mut [ClaudeMessage]) -> usize {
    let Some(last_cache_control_message_index) = messages
        .iter()
        .rposition(|message| message.content.iter().any(has_cache_control))
    else {
        return 0;
    };

    let mut added = 0;
    for message in messages.iter_mut().take(last_cache_control_message_index) {
        if message.role != ClaudeMessageRole::User {
            continue;
        }
        for block in &mut message.content {
            if let ClaudeContentBlock::ToolResult {
                tool_use_id,
                cache_reference,
                ..
            } = block
                && cache_reference.is_none()
            {
                *cache_reference = Some(tool_use_id.clone());
                added += 1;
            }
        }
    }
    added
}

fn has_cache_control(block: &ClaudeContentBlock) -> bool {
    matches!(
        block,
        ClaudeContentBlock::Text {
            cache_control: Some(_),
            ..
        }
    )
}

fn mark_latest_stable_prior_text_block(
    messages: &mut [ClaudeMessage],
    cache_control: Option<ClaudeCacheControl>,
) -> usize {
    if messages.len() < 2 {
        return 0;
    }
    for message in messages.iter_mut().rev().skip(1) {
        if message.role != ClaudeMessageRole::User {
            continue;
        }
        for block in message.content.iter_mut().rev() {
            if let ClaudeContentBlock::Text {
                text,
                cache_control: block_cache_control,
            } = block
                && !text.trim().is_empty()
            {
                *block_cache_control = cache_control;
                return 1;
            }
        }
    }
    0
}

fn system_prompt(system: String, options: ClaudePromptCacheOptions) -> Option<ClaudeSystemPrompt> {
    if system.is_empty() {
        return None;
    }
    if options.mode == ClaudePromptCacheMode::System {
        Some(ClaudeSystemPrompt::Blocks(vec![ClaudeContentBlock::Text {
            text: system,
            cache_control: Some(ClaudeCacheControl::ephemeral(options.ttl)),
        }]))
    } else {
        Some(ClaudeSystemPrompt::Text(system))
    }
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

fn normalize_claude_messages_with_stats(
    messages: Vec<ClaudeMessage>,
    history_requirements: &ClaudeHistoryRequirements,
) -> (Vec<ClaudeMessage>, ClaudeNormalizationStats) {
    let mut stats = ClaudeNormalizationStats::default();
    let mut normalized = Vec::with_capacity(messages.len());
    for message in messages {
        let mut content = Vec::with_capacity(message.content.len());
        for block in message.content {
            match block {
                ClaudeContentBlock::Text {
                    text,
                    cache_control,
                } if text.trim().is_empty() => {
                    if cache_control.is_some() {
                        stats.empty_cached_text_blocks_padded += 1;
                        content.push(ClaudeContentBlock::Text {
                            text: " ".to_string(),
                            cache_control,
                        });
                    } else {
                        stats.empty_text_blocks_dropped += 1;
                    }
                }
                ClaudeContentBlock::ToolResult {
                    tool_use_id,
                    content: result_content,
                    is_error: true,
                    cache_reference,
                } => content.push(ClaudeContentBlock::ToolResult {
                    tool_use_id,
                    content: match result_content {
                        ClaudeToolResultContent::Text(text) if text.trim().is_empty() => {
                            stats.error_tool_result_empty_text_replaced += 1;
                            ClaudeToolResultContent::Text(
                                NON_TEXT_ERROR_TOOL_RESULT_PLACEHOLDER.to_string(),
                            )
                        }
                        ClaudeToolResultContent::Text(text) => ClaudeToolResultContent::Text(text),
                        ClaudeToolResultContent::Blocks(blocks) => {
                            stats.error_tool_result_blocks_flattened += 1;
                            let block_count = blocks.len();
                            let mut kept_text_count = 0;
                            let text = blocks
                                .into_iter()
                                .filter_map(|block| match block {
                                    ClaudeContentBlock::Text { text, .. }
                                        if !text.trim().is_empty() =>
                                    {
                                        kept_text_count += 1;
                                        Some(text.trim().to_string())
                                    }
                                    ClaudeContentBlock::Text { .. }
                                    | ClaudeContentBlock::Image { .. }
                                    | ClaudeContentBlock::ToolUse { .. }
                                    | ClaudeContentBlock::ToolResult { .. }
                                    | ClaudeContentBlock::CacheEdits { .. }
                                    | ClaudeContentBlock::Thinking { .. }
                                    | ClaudeContentBlock::ProviderState { .. } => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            stats.error_tool_result_nested_blocks_dropped +=
                                block_count.saturating_sub(kept_text_count);
                            ClaudeToolResultContent::Text(if text.is_empty() {
                                NON_TEXT_ERROR_TOOL_RESULT_PLACEHOLDER.to_string()
                            } else {
                                text
                            })
                        }
                    },
                    is_error: true,
                    cache_reference,
                }),
                ClaudeContentBlock::ProviderState { value }
                    if !should_preserve_provider_state(&value, history_requirements) =>
                {
                    stats.provider_state_dropped += 1;
                }
                block => content.push(block),
            }
        }
        if message.role == ClaudeMessageRole::Assistant {
            let only_unsigned_thinking = !content.is_empty()
                && content.iter().all(|block| {
                    matches!(
                        block,
                        ClaudeContentBlock::Thinking {
                            signature: None,
                            ..
                        }
                    )
                });

            while matches!(
                content.last(),
                Some(ClaudeContentBlock::Thinking {
                    signature: None,
                    ..
                })
            ) {
                content.pop();
                stats.unsigned_trailing_thinking_blocks_dropped += 1;
            }

            if only_unsigned_thinking {
                stats.unsigned_orphan_thinking_messages_dropped += 1;
            }
        }

        if content.is_empty() {
            stats.empty_messages_dropped += 1;
        } else {
            push_message(&mut normalized, message.role, content);
        }
    }
    (normalized, stats)
}

fn should_preserve_provider_state(
    value: &Value,
    history_requirements: &ClaudeHistoryRequirements,
) -> bool {
    let Some(kind) = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
    else {
        return true;
    };
    match kind {
        "compaction" | "redacted_thinking" => true,
        "mcp_tool_use" | "mcp_tool_result" => history_requirements.preserve_mcp_tool_results,
        "server_tool_use"
        | "web_search_tool_result"
        | "web_fetch_tool_result"
        | "code_execution_tool_result"
        | "bash_code_execution_tool_result"
        | "text_editor_code_execution_tool_result"
        | "tool_search_tool_result"
        | "advisor_tool_result"
        | "advisor_tool_result_error"
        | "container_upload"
        | "tool_reference" => history_requirements.preserve_server_tool_results,
        "citation"
        | "citations"
        | "search_result_location"
        | "web_search_result_location"
        | "web_fetch_result_location" => history_requirements.preserve_structured_citations,
        _ => true,
    }
}

fn prune_excess_claude_media(
    messages: &mut [ClaudeMessage],
    limit: usize,
) -> ClaudeMediaPruningStats {
    let media_blocks_before = messages
        .iter()
        .map(|message| count_claude_media(&message.content))
        .sum::<usize>();
    let mut stats = ClaudeMediaPruningStats {
        media_blocks_before,
        ..ClaudeMediaPruningStats::default()
    };
    let mut to_remove = media_blocks_before.saturating_sub(limit);
    if to_remove == 0 {
        return stats;
    }

    for message in messages {
        if to_remove == 0 {
            return stats;
        }

        let mut content = Vec::with_capacity(message.content.len());
        for block in std::mem::take(&mut message.content) {
            match block {
                ClaudeContentBlock::ToolResult {
                    tool_use_id,
                    content: ClaudeToolResultContent::Blocks(blocks),
                    is_error,
                    cache_reference,
                } => {
                    let mut kept_blocks = Vec::with_capacity(blocks.len());
                    let mut removed_nested_media = false;
                    for block in blocks {
                        if to_remove > 0 && is_claude_media_block(&block) {
                            to_remove -= 1;
                            stats.media_blocks_removed += 1;
                            removed_nested_media = true;
                        } else {
                            kept_blocks.push(block);
                        }
                    }
                    if kept_blocks.is_empty() && removed_nested_media {
                        stats.tool_result_placeholders_inserted += 1;
                        kept_blocks.push(text_block(PRUNED_TOOL_RESULT_MEDIA_PLACEHOLDER));
                    }
                    content.push(ClaudeContentBlock::ToolResult {
                        tool_use_id,
                        content: ClaudeToolResultContent::Blocks(kept_blocks),
                        is_error,
                        cache_reference,
                    });
                }
                block if to_remove > 0 && is_claude_media_block(&block) => {
                    to_remove -= 1;
                    stats.media_blocks_removed += 1;
                }
                block => content.push(block),
            }
        }
        message.content = content;
    }
    stats
}

fn count_claude_media(content: &[ClaudeContentBlock]) -> usize {
    content
        .iter()
        .map(|block| match block {
            ClaudeContentBlock::ToolResult {
                content: ClaudeToolResultContent::Blocks(blocks),
                ..
            } => count_claude_media(blocks),
            ClaudeContentBlock::Image { .. } => 1,
            ClaudeContentBlock::Text { .. }
            | ClaudeContentBlock::ToolUse { .. }
            | ClaudeContentBlock::ToolResult { .. }
            | ClaudeContentBlock::CacheEdits { .. }
            | ClaudeContentBlock::Thinking { .. }
            | ClaudeContentBlock::ProviderState { .. } => 0,
        })
        .sum()
}

fn is_claude_media_block(block: &ClaudeContentBlock) -> bool {
    matches!(block, ClaudeContentBlock::Image { .. })
}

fn validate_tool_result_history(messages: &[ClaudeMessage]) -> codex_protocol::error::Result<()> {
    let mut pending_tool_use_ids: Vec<String> = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        if pending_tool_use_ids.is_empty() {
            if message.role == ClaudeMessageRole::User
                && message.content.iter().any(is_tool_result_block)
            {
                return invalid_claude_history(format!(
                    "user message {message_index} contains tool_result without a preceding assistant tool_use"
                ));
            }
        } else {
            validate_pending_tool_results(message_index, message, &pending_tool_use_ids)?;
            pending_tool_use_ids.clear();
        }

        if message.role == ClaudeMessageRole::Assistant {
            pending_tool_use_ids = message
                .content
                .iter()
                .filter_map(tool_use_id)
                .map(str::to_string)
                .collect();
        }
    }

    if !pending_tool_use_ids.is_empty() {
        return invalid_claude_history(format!(
            "assistant tool_use blocks were not followed by matching user tool_result blocks: {}",
            pending_tool_use_ids.join(", ")
        ));
    }

    Ok(())
}

fn repair_tool_result_history(
    messages: Vec<ClaudeMessage>,
) -> (Vec<ClaudeMessage>, ClaudeToolHistoryRepairStats) {
    let mut repaired = Vec::with_capacity(messages.len());
    let mut stats = ClaudeToolHistoryRepairStats::default();
    let mut seen_tool_use_ids = HashSet::new();
    let mut index = 0;

    while index < messages.len() {
        let message = &messages[index];
        match message.role {
            ClaudeMessageRole::Assistant => {
                let (message, pending_tool_use_ids, duplicate_tool_uses_dropped) =
                    repair_assistant_tool_uses(message, &mut seen_tool_use_ids);
                stats.duplicate_tool_uses_dropped += duplicate_tool_uses_dropped;
                repaired.push(message);

                if pending_tool_use_ids.is_empty() {
                    index += 1;
                    continue;
                }

                match messages.get(index + 1) {
                    Some(next_message) if next_message.role == ClaudeMessageRole::User => {
                        let (message, user_repair_stats) =
                            repair_user_tool_results(next_message, &pending_tool_use_ids);
                        stats.duplicate_tool_results_dropped +=
                            user_repair_stats.duplicate_tool_results_dropped;
                        stats.orphan_tool_results_dropped +=
                            user_repair_stats.orphan_tool_results_dropped;
                        stats.synthetic_tool_results_inserted +=
                            user_repair_stats.synthetic_tool_results_inserted;
                        repaired.push(message);
                        index += 2;
                    }
                    _ => {
                        stats.synthetic_tool_results_inserted += pending_tool_use_ids.len();
                        repaired.push(ClaudeMessage {
                            role: ClaudeMessageRole::User,
                            content: pending_tool_use_ids
                                .iter()
                                .map(|id| synthetic_tool_result_block(id))
                                .collect(),
                        });
                        index += 1;
                    }
                }
            }
            ClaudeMessageRole::User => {
                stats.orphan_tool_results_dropped += message
                    .content
                    .iter()
                    .filter(|block| is_tool_result_block(block))
                    .count();
                let content = strip_tool_result_blocks(&message.content);
                if !content.is_empty() {
                    repaired.push(ClaudeMessage {
                        role: ClaudeMessageRole::User,
                        content,
                    });
                }
                index += 1;
            }
        }
    }

    (repaired, stats)
}

fn repair_assistant_tool_uses(
    message: &ClaudeMessage,
    seen_tool_use_ids: &mut HashSet<String>,
) -> (ClaudeMessage, Vec<String>, usize) {
    let mut pending_tool_use_ids = Vec::new();
    let mut content = Vec::with_capacity(message.content.len());
    let mut dropped_tool_use = false;
    let mut duplicate_tool_uses_dropped = 0;

    for block in &message.content {
        if let Some(id) = tool_use_id(block) {
            if seen_tool_use_ids.insert(id.to_string()) {
                pending_tool_use_ids.push(id.to_string());
                content.push(block.clone());
            } else {
                dropped_tool_use = true;
                duplicate_tool_uses_dropped += 1;
            }
        } else {
            content.push(block.clone());
        }
    }

    if content.is_empty() && dropped_tool_use {
        content.push(text_block(INTERRUPTED_TOOL_USE_PLACEHOLDER));
    }

    (
        ClaudeMessage {
            role: ClaudeMessageRole::Assistant,
            content,
        },
        pending_tool_use_ids,
        duplicate_tool_uses_dropped,
    )
}

fn repair_user_tool_results(
    message: &ClaudeMessage,
    pending_tool_use_ids: &[String],
) -> (ClaudeMessage, ClaudeToolHistoryRepairStats) {
    let mut stats = ClaudeToolHistoryRepairStats::default();
    let pending = pending_tool_use_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut matching_results: HashMap<String, ClaudeContentBlock> = HashMap::new();
    let mut ordinary_content = Vec::new();

    for block in &message.content {
        if let Some(id) = tool_result_id(block) {
            if pending.contains(id) {
                if matching_results.contains_key(id) {
                    stats.duplicate_tool_results_dropped += 1;
                } else {
                    matching_results.insert(id.to_string(), block.clone());
                }
            } else {
                stats.orphan_tool_results_dropped += 1;
            }
        } else {
            ordinary_content.push(block.clone());
        }
    }

    let mut content = pending_tool_use_ids
        .iter()
        .map(|id| {
            if let Some(block) = matching_results.remove(id) {
                block
            } else {
                stats.synthetic_tool_results_inserted += 1;
                synthetic_tool_result_block(id)
            }
        })
        .collect::<Vec<_>>();
    content.append(&mut ordinary_content);

    (
        ClaudeMessage {
            role: ClaudeMessageRole::User,
            content,
        },
        stats,
    )
}

fn strip_tool_result_blocks(content: &[ClaudeContentBlock]) -> Vec<ClaudeContentBlock> {
    content
        .iter()
        .filter(|block| !is_tool_result_block(block))
        .cloned()
        .collect()
}

fn synthetic_tool_result_block(call_id: &str) -> ClaudeContentBlock {
    tool_result_block(
        call_id,
        ClaudeToolResultContent::Text(SYNTHETIC_TOOL_RESULT_PLACEHOLDER.to_string()),
        true,
    )
}

fn validate_pending_tool_results(
    message_index: usize,
    message: &ClaudeMessage,
    pending_tool_use_ids: &[String],
) -> codex_protocol::error::Result<()> {
    if message.role != ClaudeMessageRole::User {
        return invalid_claude_history(format!(
            "assistant tool_use blocks must be followed by a user tool_result message, got {:?} message {message_index}",
            message.role
        ));
    }

    if message.content.len() < pending_tool_use_ids.len() {
        return invalid_claude_history(format!(
            "user message {message_index} has fewer tool_result blocks than preceding tool_use blocks"
        ));
    }

    for (offset, expected_id) in pending_tool_use_ids.iter().enumerate() {
        let Some(block) = message.content.get(offset) else {
            return invalid_claude_history(format!(
                "user message {message_index} has fewer tool_result blocks than preceding tool_use blocks"
            ));
        };
        let Some(actual_id) = tool_result_id(block) else {
            return invalid_claude_history(format!(
                "user message {message_index} has ordinary content before required tool_result {offset} for preceding tool_use id {expected_id}"
            ));
        };
        if actual_id != expected_id {
            return invalid_claude_history(format!(
                "user message {message_index} tool_result {offset} does not match preceding tool_use id {expected_id}"
            ));
        }
    }

    if message
        .content
        .iter()
        .skip(pending_tool_use_ids.len())
        .any(is_tool_result_block)
    {
        return invalid_claude_history(format!(
            "user message {message_index} contains tool_result blocks after ordinary content"
        ));
    }

    Ok(())
}

fn is_tool_result_block(block: &ClaudeContentBlock) -> bool {
    matches!(block, ClaudeContentBlock::ToolResult { .. })
}

fn tool_result_id(block: &ClaudeContentBlock) -> Option<&str> {
    match block {
        ClaudeContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
        _ => None,
    }
}

fn tool_use_id(block: &ClaudeContentBlock) -> Option<&str> {
    match block {
        ClaudeContentBlock::ToolUse { id, .. } => Some(id.as_str()),
        _ => None,
    }
}

fn invalid_claude_history<T>(message: String) -> codex_protocol::error::Result<T> {
    Err(CodexErr::InvalidRequest(format!(
        "invalid Claude tool history: {message}"
    )))
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
            ContentItem::InputText { text }
            | ContentItem::OutputText { text }
            | ContentItem::OutputTextWithCitations { text, .. } => text.clone(),
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
            ContentItem::InputText { text }
            | ContentItem::OutputText { text }
            | ContentItem::OutputTextWithCitations { text, .. }
                if !text.is_empty() =>
            {
                Some(text_block(text))
            }
            ContentItem::InputImage { image_url, .. } => Some(image_content_block(image_url)),
            ContentItem::InputText { .. }
            | ContentItem::OutputText { .. }
            | ContentItem::OutputTextWithCitations { .. } => None,
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

fn text_block(text: &str) -> ClaudeContentBlock {
    ClaudeContentBlock::Text {
        text: text.to_string(),
        cache_control: None,
    }
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
        cache_reference: None,
    }
}

fn thinking_block(
    id: &str,
    content: Option<&[codex_protocol::models::ReasoningItemContent]>,
    encrypted_content: Option<&String>,
) -> Option<ClaudeContentBlock> {
    if !is_claude_reasoning_item_id(id) {
        return None;
    }

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

fn provider_state_block(encrypted_content: &str) -> Option<ClaudeContentBlock> {
    let value = serde_json::from_str::<Value>(encrypted_content).ok()?;
    value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)?;
    Some(ClaudeContentBlock::ProviderState { value })
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
                Some(text_block(text))
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
        text_block(&format!("[image: {image_url}]"))
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
    use codex_protocol::models::ReasoningItemReasoningSummary;
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
    fn detects_deepseek_anthropic_base_url() {
        assert_eq!(
            provider_compat_for_base_url(Some("https://api.deepseek.com/anthropic")),
            ClaudeProviderCompat::DeepSeek
        );
        assert_eq!(
            provider_compat_for_base_url(Some("https://api.deepseek.com/anthropic/")),
            ClaudeProviderCompat::DeepSeek
        );
        assert_eq!(
            provider_compat_for_base_url(Some("https://api.deepseek.com/anthropic/v1")),
            ClaudeProviderCompat::DeepSeek
        );
        assert_eq!(
            provider_compat_for_base_url(Some("https://api.deepseek.com/anthropic/v1/")),
            ClaudeProviderCompat::DeepSeek
        );
        assert_eq!(
            provider_compat_for_base_url(Some("https://api.anthropic.com/v1")),
            ClaudeProviderCompat::Anthropic
        );
        assert_eq!(
            provider_compat_for_base_url(Some("https://notapi.deepseek.com/anthropic")),
            ClaudeProviderCompat::Anthropic
        );
        assert_eq!(
            provider_compat_for_base_url(Some("https://api.deepseek.com/other")),
            ClaudeProviderCompat::Anthropic
        );
        assert_eq!(
            provider_compat_for_base_url(Some(
                "https://api.anthropic.com/v1?proxy=api.deepseek.com/anthropic"
            )),
            ClaudeProviderCompat::Anthropic
        );
        assert_eq!(
            provider_compat_for_provider("DeepSeek", Some("http://localhost/v1"), None),
            ClaudeProviderCompat::DeepSeek
        );
        assert_eq!(
            provider_compat_for_provider(
                "custom claude",
                Some("http://localhost/v1"),
                Some("deepseek-v4-pro")
            ),
            ClaudeProviderCompat::DeepSeek
        );
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
    fn builds_claude_request_with_native_web_search_server_tool() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "search".to_string(),
                }],
                phase: None,
            }],
            tools: vec![ToolSpec::WebSearch {
                external_web_access: Some(true),
                filters: Some(codex_tools::ResponsesApiWebSearchFilters {
                    allowed_domains: Some(vec!["example.com".to_string()]),
                }),
                user_location: Some(codex_tools::ResponsesApiWebSearchUserLocation {
                    r#type: codex_protocol::config_types::WebSearchUserLocationType::Approximate,
                    country: Some("US".to_string()),
                    region: None,
                    city: Some("San Francisco".to_string()),
                    timezone: Some("America/Los_Angeles".to_string()),
                }),
                search_context_size: None,
                search_content_types: None,
            }],
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            serde_json::to_value(&request.tools).expect("serialize tools"),
            json!([{
                "type": "web_search_20250305",
                "name": "web_search",
                "allowed_domains": ["example.com"],
                "user_location": {
                    "type": "approximate",
                    "country": "US",
                    "city": "San Francisco",
                    "timezone": "America/Los_Angeles"
                }
            }])
        );
        assert!(request.tool_call_info.is_empty());
    }

    #[test]
    fn builds_deepseek_request_with_local_web_search_function_tool() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "search".to_string(),
                }],
                phase: None,
            }],
            tools: vec![ToolSpec::WebSearch {
                external_web_access: Some(true),
                filters: Some(codex_tools::ResponsesApiWebSearchFilters {
                    allowed_domains: Some(vec!["example.com".to_string()]),
                }),
                user_location: None,
                search_context_size: None,
                search_content_types: None,
            }],
            ..Default::default()
        };

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                provider_compat: ClaudeProviderCompat::DeepSeek,
                ..Default::default()
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request.tools).expect("serialize tools"),
            json!([{
                "name": "web_search",
                "description": "Search the web using Codex's local web search handler and return relevant text results. Use `query` for one search or `queries` for a small batch.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query."
                        },
                        "queries": {
                            "type": "array",
                            "description": "Optional batch of search queries. Use only when several closely related searches are needed.",
                            "items": { "type": "string" }
                        }
                    },
                    "additionalProperties": false
                }
            }])
        );
        assert_eq!(
            request.tool_call_info.get("web_search"),
            Some(&ApiClaudeToolCallInfo {
                name: "web_search".to_string(),
                namespace: None,
                kind: ApiClaudeToolCallKind::Function,
            })
        );
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
                        cache_reference: None,
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
    fn normalizes_claude_error_tool_result_to_text_only_content() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload {
                        body: FunctionCallOutputBody::ContentItems(vec![
                            FunctionCallOutputContentItem::InputText {
                                text: "first".to_string(),
                            },
                            FunctionCallOutputContentItem::InputImage {
                                image_url: "data:image/png;base64,Zm9v".to_string(),
                                detail: Some(ImageDetail::High),
                            },
                            FunctionCallOutputContentItem::InputText {
                                text: " second ".to_string(),
                            },
                        ]),
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
            request.messages[1].content,
            vec![ClaudeContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: ClaudeToolResultContent::Text("first\n\nsecond".to_string()),
                is_error: true,
                cache_reference: None,
            }]
        );
    }

    #[test]
    fn normalizes_image_only_error_tool_result_to_text_placeholder() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload {
                        body: FunctionCallOutputBody::ContentItems(vec![
                            FunctionCallOutputContentItem::InputImage {
                                image_url: "data:image/png;base64,Zm9v".to_string(),
                                detail: Some(ImageDetail::High),
                            },
                        ]),
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
            request.messages[1].content,
            vec![ClaudeContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: ClaudeToolResultContent::Text(
                    NON_TEXT_ERROR_TOOL_RESULT_PLACEHOLDER.to_string()
                ),
                is_error: true,
                cache_reference: None,
            }]
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
                    cache_control: None,
                }],
            }]
        );
    }

    #[test]
    fn prunes_oldest_top_level_media_blocks_before_sending_claude_request() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: (0..=CLAUDE_MAX_MEDIA_PER_REQUEST)
                    .map(|index| ContentItem::InputImage {
                        image_url: format!("https://example.com/image-{index:03}.png"),
                        detail: Some(ImageDetail::High),
                    })
                    .collect(),
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
        let urls = request.messages[0]
            .content
            .iter()
            .filter_map(|block| match block {
                ClaudeContentBlock::Image {
                    source: ClaudeImageSource::Url { url },
                } => Some(url.as_str()),
                ClaudeContentBlock::Image { .. }
                | ClaudeContentBlock::Text { .. }
                | ClaudeContentBlock::ToolUse { .. }
                | ClaudeContentBlock::ToolResult { .. }
                | ClaudeContentBlock::CacheEdits { .. }
                | ClaudeContentBlock::Thinking { .. }
                | ClaudeContentBlock::ProviderState { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(urls.len(), CLAUDE_MAX_MEDIA_PER_REQUEST);
        assert_eq!(
            urls.first().copied(),
            Some("https://example.com/image-001.png")
        );
        assert_eq!(
            urls.last().copied(),
            Some("https://example.com/image-100.png")
        );
    }

    #[test]
    fn counts_nested_tool_result_media_and_preserves_tool_result_id_when_pruned() {
        let prompt = Prompt {
            input: vec![
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
                        FunctionCallOutputContentItem::InputImage {
                            image_url: "data:image/png;base64,Zm9v".to_string(),
                            detail: Some(ImageDetail::High),
                        },
                    ]),
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: (0..CLAUDE_MAX_MEDIA_PER_REQUEST)
                        .map(|index| ContentItem::InputImage {
                            image_url: format!("https://example.com/recent-{index:03}.png"),
                            detail: Some(ImageDetail::High),
                        })
                        .collect(),
                    phase: None,
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
            count_claude_media(&request.messages[1].content),
            CLAUDE_MAX_MEDIA_PER_REQUEST
        );
        assert_eq!(
            request.messages[1].content.first(),
            Some(&ClaudeContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: ClaudeToolResultContent::Blocks(vec![ClaudeContentBlock::Text {
                    text: PRUNED_TOOL_RESULT_MEDIA_PLACEHOLDER.to_string(),
                    cache_control: None,
                }]),
                is_error: false,
                cache_reference: None,
            })
        );
    }

    #[test]
    fn normalizes_whitespace_only_assistant_message_and_merges_adjacent_users() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "first".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "\n\n".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "second".to_string(),
                    }],
                    phase: None,
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
            request.messages,
            vec![ClaudeMessage {
                role: ClaudeMessageRole::User,
                content: vec![
                    ClaudeContentBlock::Text {
                        text: "first".to_string(),
                        cache_control: None,
                    },
                    ClaudeContentBlock::Text {
                        text: "second".to_string(),
                        cache_control: None,
                    },
                ],
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
                ..Default::default()
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
    fn builds_deepseek_anthropic_request_with_output_config_effort() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "think harder".to_string(),
                }],
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
                reasoning_effort: Some(ReasoningEffortConfig::XHigh),
                provider_compat: ClaudeProviderCompat::DeepSeek,
                ..Default::default()
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
                        "text": "think harder"
                    }]
                }],
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": CLAUDE_THINKING_XHIGH_BUDGET_TOKENS
                },
                "output_config": {
                    "effort": "max"
                },
                "stream": true
            })
        );
    }

    #[test]
    fn builds_deepseek_anthropic_request_with_disabled_thinking() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "answer directly".to_string(),
                }],
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
                reasoning_effort: Some(ReasoningEffortConfig::None),
                provider_compat: ClaudeProviderCompat::DeepSeek,
                ..Default::default()
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
                        "text": "answer directly"
                    }]
                }],
                "thinking": {
                    "type": "disabled"
                },
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
                ..Default::default()
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
                    id: "msg_1_reasoning_0".to_string(),
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
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
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
            vec![
                ClaudeMessage {
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
                },
                ClaudeMessage {
                    role: ClaudeMessageRole::User,
                    content: vec![ClaudeContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: ClaudeToolResultContent::Text("ok".to_string()),
                        is_error: false,
                        cache_reference: None,
                    }],
                },
            ]
        );
    }

    #[test]
    fn builds_claude_history_without_openai_encrypted_reasoning_as_signature_only_block() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Reasoning {
                    id: "rs_1".to_string(),
                    summary: vec![ReasoningItemReasoningSummary::SummaryText {
                        text: "OpenAI reasoning summary".to_string(),
                    }],
                    content: None,
                    encrypted_content: Some("openai-encrypted-content".to_string()),
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "continue".to_string(),
                    }],
                    phase: None,
                },
            ],
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
                    text: "continue".to_string(),
                    cache_control: None,
                }],
            }]
        );
    }

    #[test]
    fn builds_claude_history_without_summaryless_openai_encrypted_reasoning_as_signature() {
        let prompt = Prompt {
            input: vec![ResponseItem::Reasoning {
                id: "rs_1".to_string(),
                summary: Vec::new(),
                content: None,
                encrypted_content: Some("openai-encrypted-content".to_string()),
            }],
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert!(
            request.messages.iter().all(|message| message
                .content
                .iter()
                .all(|block| !matches!(block, ClaudeContentBlock::Thinking { .. }))),
            "OpenAI encrypted reasoning must not be replayed as Claude thinking signature"
        );
    }

    #[test]
    fn builds_claude_history_with_omitted_thinking_signature_only_block() {
        let prompt = Prompt {
            input: vec![ResponseItem::Reasoning {
                id: "msg_1_reasoning_0".to_string(),
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: String::new(),
                }]),
                encrypted_content: Some("claude-thinking-signature".to_string()),
            }],
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            request.messages,
            vec![ClaudeMessage {
                role: ClaudeMessageRole::Assistant,
                content: vec![ClaudeContentBlock::Thinking {
                    thinking: String::new(),
                    signature: Some("claude-thinking-signature".to_string()),
                }],
            }]
        );
    }

    #[test]
    fn drops_unsigned_orphan_thinking_only_assistant_message() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "continue".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Reasoning {
                    id: "msg_1_reasoning_0".to_string(),
                    summary: Vec::new(),
                    content: Some(vec![ReasoningItemContent::ReasoningText {
                        text: "stale thinking".to_string(),
                    }]),
                    encrypted_content: None,
                },
            ],
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
                    text: "continue".to_string(),
                    cache_control: None,
                }],
            }]
        );
    }

    #[test]
    fn strips_unsigned_trailing_thinking_from_assistant_message() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "answer".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Reasoning {
                    id: "msg_1_reasoning_0".to_string(),
                    summary: Vec::new(),
                    content: Some(vec![ReasoningItemContent::ReasoningText {
                        text: "trailing thinking".to_string(),
                    }]),
                    encrypted_content: None,
                },
            ],
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert_eq!(
            request.messages,
            vec![ClaudeMessage {
                role: ClaudeMessageRole::Assistant,
                content: vec![ClaudeContentBlock::Text {
                    text: "answer".to_string(),
                    cache_control: None,
                }],
            }]
        );
    }

    #[test]
    fn builds_claude_history_without_openai_reasoning_as_thinking_block() {
        let prompt = Prompt {
            input: vec![ResponseItem::Reasoning {
                id: "rs_1".to_string(),
                summary: vec![ReasoningItemReasoningSummary::SummaryText {
                    text: "OpenAI reasoning summary".to_string(),
                }],
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: "visible reasoning".to_string(),
                }]),
                encrypted_content: Some("openai-encrypted-content".to_string()),
            }],
            ..Default::default()
        };

        let request =
            build_claude_messages_request(&prompt, &model_info(), ClaudeRequestOptions::default())
                .expect("request");

        assert!(
            request.messages.iter().all(|message| message
                .content
                .iter()
                .all(|block| !matches!(block, ClaudeContentBlock::Thinking { .. }))),
            "OpenAI reasoning content must not be replayed as Claude thinking"
        );
    }

    #[test]
    fn repairs_claude_tool_result_without_preceding_tool_use() {
        let prompt = Prompt {
            input: vec![ResponseItem::FunctionCallOutput {
                call_id: "call_1".to_string(),
                output: FunctionCallOutputPayload::from_text("ok".to_string()),
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
                    text: " ".to_string(),
                    cache_control: None,
                }],
            }]
        );
    }

    #[test]
    fn repairs_user_text_before_pending_tool_result() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "too soon".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
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
            request.messages,
            vec![
                ClaudeMessage {
                    role: ClaudeMessageRole::Assistant,
                    content: vec![ClaudeContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "lookup".to_string(),
                        input: json!({}),
                    }],
                },
                ClaudeMessage {
                    role: ClaudeMessageRole::User,
                    content: vec![
                        ClaudeContentBlock::ToolResult {
                            tool_use_id: "call_1".to_string(),
                            content: ClaudeToolResultContent::Text("ok".to_string()),
                            is_error: false,
                            cache_reference: None,
                        },
                        ClaudeContentBlock::Text {
                            text: "too soon".to_string(),
                            cache_control: None,
                        },
                    ],
                },
            ]
        );
    }

    #[test]
    fn repairs_reordered_claude_parallel_tool_results() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "read".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_2".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_2".to_string(),
                    output: FunctionCallOutputPayload::from_text("second first".to_string()),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("first second".to_string()),
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
            request.messages,
            vec![
                ClaudeMessage {
                    role: ClaudeMessageRole::Assistant,
                    content: vec![
                        ClaudeContentBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "lookup".to_string(),
                            input: json!({}),
                        },
                        ClaudeContentBlock::ToolUse {
                            id: "call_2".to_string(),
                            name: "read".to_string(),
                            input: json!({}),
                        },
                    ],
                },
                ClaudeMessage {
                    role: ClaudeMessageRole::User,
                    content: vec![
                        ClaudeContentBlock::ToolResult {
                            tool_use_id: "call_1".to_string(),
                            content: ClaudeToolResultContent::Text("first second".to_string()),
                            is_error: false,
                            cache_reference: None,
                        },
                        ClaudeContentBlock::ToolResult {
                            tool_use_id: "call_2".to_string(),
                            content: ClaudeToolResultContent::Text("second first".to_string()),
                            is_error: false,
                            cache_reference: None,
                        },
                    ],
                },
            ]
        );
    }

    #[test]
    fn repairs_missing_claude_tool_result_with_synthetic_error() {
        let prompt = Prompt {
            input: vec![ResponseItem::FunctionCall {
                id: None,
                name: "lookup".to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                call_id: "call_1".to_string(),
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
            vec![
                ClaudeMessage {
                    role: ClaudeMessageRole::Assistant,
                    content: vec![ClaudeContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "lookup".to_string(),
                        input: json!({}),
                    }],
                },
                ClaudeMessage {
                    role: ClaudeMessageRole::User,
                    content: vec![ClaudeContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: ClaudeToolResultContent::Text(
                            SYNTHETIC_TOOL_RESULT_PLACEHOLDER.to_string(),
                        ),
                        is_error: true,
                        cache_reference: None,
                    }],
                },
            ]
        );
    }

    #[test]
    fn repairs_duplicate_claude_tool_use_ids() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{\"id\":1}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{\"id\":2}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
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
                        cache_reference: None,
                    }],
                },
            ]
        );
    }

    #[test]
    fn builds_claude_request_with_system_prompt_cache_control() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hello".to_string(),
                }],
                phase: None,
            }],
            base_instructions: BaseInstructions {
                text: "stable system".to_string(),
            },
            ..Default::default()
        };

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                prompt_cache: ClaudePromptCacheOptions {
                    mode: ClaudePromptCacheMode::System,
                    ttl: Some(ClaudeCacheTtl::OneHour),
                },
                ..Default::default()
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request")["system"],
            json!([{
                "type": "text",
                "text": "stable system",
                "cache_control": {
                    "type": "ephemeral",
                    "ttl": "1h"
                }
            }])
        );
    }

    #[test]
    fn builds_claude_request_with_conversation_cache_on_prior_user_message() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "stable prior context".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "prior answer".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "volatile current question".to_string(),
                    }],
                    phase: None,
                },
            ],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                prompt_cache: ClaudePromptCacheOptions {
                    mode: ClaudePromptCacheMode::Conversation,
                    ttl: None,
                },
                ..Default::default()
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request")["messages"],
            json!([
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "stable prior context",
                        "cache_control": {"type": "ephemeral"}
                    }]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "prior answer"}]
                },
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "volatile current question"}]
                }
            ])
        );
    }

    #[test]
    fn adds_cache_reference_to_cached_prefix_tool_results_when_capability_enabled() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "tool noted".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "stable prior context".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "prior answer".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "volatile current question".to_string(),
                    }],
                    phase: None,
                },
            ],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                prompt_cache: ClaudePromptCacheOptions {
                    mode: ClaudePromptCacheMode::Conversation,
                    ttl: None,
                },
                cache_editing: cache_editing_options_for_provider(ClaudeProviderCompat::Anthropic),
                ..Default::default()
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request")["messages"],
            json!([
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "lookup",
                        "input": {}
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": "ok",
                        "cache_reference": "call_1"
                    }]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "tool noted"}]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "stable prior context",
                        "cache_control": {"type": "ephemeral"}
                    }]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "prior answer"}]
                },
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "volatile current question"}]
                }
            ])
        );
    }

    #[test]
    fn inserts_and_dedupes_cache_edits_after_tool_results_when_capability_enabled() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "prior answer".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "current question".to_string(),
                    }],
                    phase: None,
                },
            ],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                cache_editing: ClaudeCacheEditingOptions {
                    capability: ClaudeCacheEditingCapability::Enabled,
                    new_delete_references: vec![
                        "dup-ref".to_string(),
                        " new-ref ".to_string(),
                        String::new(),
                    ],
                    pinned_deletes: vec![ClaudePinnedCacheEdits {
                        user_message_index: 1,
                        delete_references: vec!["old-ref".to_string(), "dup-ref".to_string()],
                    }],
                },
                ..Default::default()
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request")["messages"],
            json!([
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "lookup",
                        "input": {}
                    }]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "call_1",
                            "content": "ok"
                        },
                        {
                            "type": "cache_edits",
                            "edits": [
                                {"type": "delete", "cache_reference": "old-ref"},
                                {"type": "delete", "cache_reference": "dup-ref"}
                            ]
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "prior answer"}]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "cache_edits",
                            "edits": [{"type": "delete", "cache_reference": "new-ref"}]
                        },
                        {
                            "type": "text",
                            "text": "current question"
                        }
                    ]
                }
            ])
        );
    }

    #[test]
    fn does_not_emit_cache_edit_fields_for_deepseek_capability() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "lookup".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "tool noted".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "stable prior context".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "prior answer".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "current question".to_string(),
                    }],
                    phase: None,
                },
            ],
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            ..Default::default()
        };
        let mut cache_editing = cache_editing_options_for_provider(ClaudeProviderCompat::DeepSeek);
        cache_editing.new_delete_references = vec!["call_1".to_string()];
        cache_editing.pinned_deletes = vec![ClaudePinnedCacheEdits {
            user_message_index: 1,
            delete_references: vec!["old-ref".to_string()],
        }];

        let request = build_claude_messages_request(
            &prompt,
            &model_info(),
            ClaudeRequestOptions {
                prompt_cache: ClaudePromptCacheOptions {
                    mode: ClaudePromptCacheMode::Conversation,
                    ttl: None,
                },
                provider_compat: ClaudeProviderCompat::DeepSeek,
                cache_editing,
                ..Default::default()
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request).expect("serialize request")["messages"],
            json!([
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "lookup",
                        "input": {}
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": "ok"
                    }]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "tool noted"}]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "stable prior context",
                        "cache_control": {"type": "ephemeral"}
                    }]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "prior answer"}]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "current question"
                    }]
                }
            ])
        );
    }

    #[test]
    fn builds_claude_request_with_preserved_provider_state_block() {
        let raw_compaction = json!({
            "type": "compaction",
            "content": "summarized provider state"
        });
        let prompt = Prompt {
            input: vec![ResponseItem::Compaction {
                encrypted_content: raw_compaction.to_string(),
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
            serde_json::to_value(&request.messages).expect("serialize messages"),
            json!([{
                "role": "assistant",
                "content": [raw_compaction]
            }])
        );
    }

    #[test]
    fn builds_claude_request_with_context_compaction_provider_state_block() {
        let raw_compaction = json!({
            "type": "compaction",
            "content": "summarized provider state"
        });
        let prompt = Prompt {
            input: vec![ResponseItem::ContextCompaction {
                encrypted_content: Some(raw_compaction.to_string()),
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
            serde_json::to_value(&request.messages).expect("serialize messages"),
            json!([{
                "role": "assistant",
                "content": [raw_compaction]
            }])
        );
    }

    #[test]
    fn drops_stale_server_provider_state_without_active_server_tools() {
        let stale_server_tool_result = json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvu_1",
            "content": [{"type": "text", "text": "old result"}]
        });
        let compaction = json!({
            "type": "compaction",
            "content": "summarized provider state"
        });
        let redacted_thinking = json!({
            "type": "redacted_thinking",
            "data": "opaque"
        });
        let unknown_provider_state = json!({
            "type": "future_tool_result",
            "data": "keep unknown state"
        });
        let prompt = Prompt {
            input: vec![
                ResponseItem::Compaction {
                    encrypted_content: stale_server_tool_result.to_string(),
                },
                ResponseItem::Compaction {
                    encrypted_content: compaction.to_string(),
                },
                ResponseItem::Compaction {
                    encrypted_content: redacted_thinking.to_string(),
                },
                ResponseItem::Compaction {
                    encrypted_content: unknown_provider_state.to_string(),
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
            serde_json::to_value(&request.messages).expect("serialize messages"),
            json!([{
                "role": "assistant",
                "content": [
                    compaction,
                    redacted_thinking,
                    unknown_provider_state
                ]
            }])
        );
    }

    #[test]
    fn preserves_server_provider_state_for_native_web_search() {
        let web_search_tool_result = json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvu_1",
            "content": [{"type": "text", "text": "current result"}]
        });
        let web_search_citation = json!({
            "type": "web_search_result_location",
            "url": "https://example.com/result"
        });
        let prompt = Prompt {
            input: vec![
                ResponseItem::Compaction {
                    encrypted_content: web_search_tool_result.to_string(),
                },
                ResponseItem::Compaction {
                    encrypted_content: web_search_citation.to_string(),
                },
            ],
            tools: vec![ToolSpec::WebSearch {
                external_web_access: Some(true),
                filters: None,
                user_location: None,
                search_context_size: None,
                search_content_types: None,
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
            serde_json::to_value(&request.messages).expect("serialize messages"),
            json!([{
                "role": "assistant",
                "content": [
                    web_search_tool_result,
                    web_search_citation
                ]
            }])
        );
    }

    #[test]
    fn preserves_server_state_for_follow_up_with_native_web_search() {
        let server_tool_use = json!({
            "type": "server_tool_use",
            "id": "srvu_1",
            "name": "web_search",
            "input": {"query": "claude protocol"}
        });
        let web_search_tool_result = json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvu_1",
            "content": [{"type": "text", "text": "current result"}]
        });
        let prompt = Prompt {
            input: vec![
                ResponseItem::Compaction {
                    encrypted_content: server_tool_use.to_string(),
                },
                ResponseItem::Compaction {
                    encrypted_content: web_search_tool_result.to_string(),
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "follow up".to_string(),
                    }],
                    phase: None,
                },
            ],
            tools: vec![ToolSpec::WebSearch {
                external_web_access: Some(true),
                filters: None,
                user_location: None,
                search_context_size: None,
                search_content_types: None,
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
            serde_json::to_value(&request.messages).expect("serialize messages"),
            json!([
                {
                    "role": "assistant",
                    "content": [server_tool_use, web_search_tool_result]
                },
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "follow up"}]
                }
            ])
        );
    }

    #[test]
    fn drops_web_search_provider_state_for_deepseek_local_web_search() {
        let web_search_tool_result = json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvu_1",
            "content": [{"type": "text", "text": "stale result"}]
        });
        let prompt = Prompt {
            input: vec![
                ResponseItem::Compaction {
                    encrypted_content: web_search_tool_result.to_string(),
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "continue".to_string(),
                    }],
                    phase: None,
                },
            ],
            tools: vec![ToolSpec::WebSearch {
                external_web_access: Some(true),
                filters: None,
                user_location: None,
                search_context_size: None,
                search_content_types: None,
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
                provider_compat: ClaudeProviderCompat::DeepSeek,
                ..Default::default()
            },
        )
        .expect("request");

        assert_eq!(
            serde_json::to_value(&request.messages).expect("serialize messages"),
            json!([{
                "role": "user",
                "content": [{"type": "text", "text": "continue"}]
            }])
        );
    }

    #[test]
    fn preserves_remote_mcp_provider_state_only_when_required() {
        let mcp_tool_use = json!({
            "type": "mcp_tool_use",
            "id": "mcpu_1",
            "name": "docs.search"
        });
        let mcp_tool_result = json!({
            "type": "mcp_tool_result",
            "tool_use_id": "mcpu_1",
            "content": [{"type": "text", "text": "mcp result"}]
        });
        let server_tool_result = json!({
            "type": "web_fetch_tool_result",
            "tool_use_id": "srvu_1",
            "content": [{"type": "text", "text": "fetch result"}]
        });
        let messages = vec![ClaudeMessage {
            role: ClaudeMessageRole::Assistant,
            content: vec![
                ClaudeContentBlock::ProviderState {
                    value: mcp_tool_use.clone(),
                },
                ClaudeContentBlock::ProviderState {
                    value: mcp_tool_result.clone(),
                },
                ClaudeContentBlock::ProviderState {
                    value: server_tool_result,
                },
            ],
        }];

        let (normalized, stats) = normalize_claude_messages_with_stats(
            messages,
            &ClaudeHistoryRequirements {
                preserve_mcp_tool_results: true,
                ..ClaudeHistoryRequirements::default()
            },
        );
        assert_eq!(stats.provider_state_dropped, 1);

        assert_eq!(
            serde_json::to_value(&normalized).expect("serialize messages"),
            json!([{
                "role": "assistant",
                "content": [mcp_tool_use, mcp_tool_result]
            }])
        );
    }

    #[test]
    fn preserves_remote_mcp_provider_state_with_follow_up_after_compaction_when_required() {
        let mcp_tool_use = json!({
            "type": "mcp_tool_use",
            "id": "mcpu_1",
            "server_name": "docs",
            "name": "search",
            "input": {"query": "claude"}
        });
        let mcp_tool_result = json!({
            "type": "mcp_tool_result",
            "tool_use_id": "mcpu_1",
            "content": [{"type": "text", "text": "mcp result"}]
        });
        let messages = vec![
            ClaudeMessage {
                role: ClaudeMessageRole::Assistant,
                content: vec![
                    ClaudeContentBlock::ProviderState {
                        value: mcp_tool_use.clone(),
                    },
                    ClaudeContentBlock::ProviderState {
                        value: mcp_tool_result.clone(),
                    },
                ],
            },
            ClaudeMessage {
                role: ClaudeMessageRole::User,
                content: vec![ClaudeContentBlock::Text {
                    text: "follow up".to_string(),
                    cache_control: None,
                }],
            },
        ];

        let (normalized, stats) = normalize_claude_messages_with_stats(
            messages,
            &ClaudeHistoryRequirements {
                preserve_mcp_tool_results: true,
                ..ClaudeHistoryRequirements::default()
            },
        );
        assert_eq!(stats.provider_state_dropped, 0);

        assert_eq!(
            serde_json::to_value(&normalized).expect("serialize messages"),
            json!([
                {
                    "role": "assistant",
                    "content": [mcp_tool_use, mcp_tool_result]
                },
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "follow up"}]
                }
            ])
        );
    }
}
