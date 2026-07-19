use crate::FreeformTool;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::ToolSpec;
use crate::tool_spec::web_search_function_schema_json;
use codex_utils_string::approx_bytes_for_tokens;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;

const MAX_CHAT_TOOL_NAME_LEN: usize = 64;
pub(crate) const MAX_CHAT_TOOLS: usize = 128;
/// Hard cap for the local reverse-mapping side table (wire `tools` stays at
/// [`MAX_CHAT_TOOLS`]). Prefer retained base identities, then the newest extras.
pub const MAX_CHAT_TOOL_CALL_INFO: usize = 512;
const MAX_CHAT_TOOL_TOKENS: usize = 10_000;
const MAX_CHAT_TOOLS_TOTAL_TOKENS: usize = 64_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatToolCallKind {
    Function,
    Custom,
    ToolSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatToolCallInfo {
    pub chat_name: String,
    pub name: String,
    pub namespace: Option<String>,
    pub kind: ChatToolCallKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatToolsJson {
    pub tools: Vec<Value>,
    pub tool_call_info: Vec<ChatToolCallInfo>,
}

/// Serializes Codex tool kinds as Chat Completions function tools, capped at the provider limit.
///
/// The side table preserves the original Codex kind and namespace so streamed
/// function calls can be restored to the correct `ResponseItem` variant.
/// `tool_call_info` is capped at [`MAX_CHAT_TOOL_CALL_INFO`] for reverse mapping
/// even when the wire `tools` array is truncated further.
pub fn create_tools_json_for_chat_completions(
    tools: &[ToolSpec],
) -> Result<ChatToolsJson, serde_json::Error> {
    create_tools_json_for_chat_completions_retaining(tools, std::iter::empty::<&str>())
}

/// Like [`create_tools_json_for_chat_completions`], but retains the listed Chat
/// wire names when the provider tool cap is exceeded.
///
/// Retained names are kept first (base plan tools) and their wire schemas are
/// protected from later discoveries with the same Chat identity. Any remaining
/// slots are filled with the newest non-retained tools so historical
/// `tool_search` discoveries can still appear when space remains.
pub fn create_tools_json_for_chat_completions_retaining(
    tools: &[ToolSpec],
    retain_chat_names: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<ChatToolsJson, serde_json::Error> {
    let retain_chat_names = retain_chat_names
        .into_iter()
        .map(|name| name.as_ref().to_string())
        .collect::<HashSet<_>>();
    let output = create_uncapped_tools_json_for_chat_completions(tools, &retain_chat_names)?;
    // Cap the wire array against the full parallel side table, then bound the
    // returned reverse-map separately so historical identities stay available.
    let tools = cap_chat_tools(output.tools, &output.tool_call_info, &retain_chat_names);
    validate_tool_budgets(&tools)?;
    let tool_call_info = cap_tool_call_info(output.tool_call_info, &retain_chat_names);
    Ok(ChatToolsJson {
        tools,
        tool_call_info,
    })
}

/// Builds wire tools from a base plan plus as many discovered tools as fit under
/// provider count/token budgets when a full merge would overflow.
///
/// Base plan schemas are never overwritten by discoveries. Newest discoveries
/// are preferred when packing. The reverse-mapping table covers base + every
/// discovery identity (capped).
pub fn create_tools_json_packing_discoveries(
    base_tools: &[ToolSpec],
    discovered_tools: &[ToolSpec],
) -> Result<ChatToolsJson, serde_json::Error> {
    let retain_chat_names = chat_tool_identity_names(base_tools);
    let mut combined = base_tools.to_vec();
    combined.extend(discovered_tools.iter().cloned());
    match create_tools_json_for_chat_completions_retaining(&combined, &retain_chat_names) {
        Ok(tools_json) => Ok(tools_json),
        Err(err) if !discovered_tools.is_empty() => {
            tracing::warn!(
                error = %err,
                discovered_tool_count = discovered_tools.len(),
                "Chat tool_search rehydration exceeded the tools budget; packing a budget-safe discovery subset"
            );
            pack_base_and_discoveries(base_tools, discovered_tools, &retain_chat_names)
        }
        Err(err) => Err(err),
    }
}

fn pack_base_and_discoveries(
    base_tools: &[ToolSpec],
    discovered_tools: &[ToolSpec],
    retain_chat_names: &HashSet<String>,
) -> Result<ChatToolsJson, serde_json::Error> {
    let base_uncapped =
        create_uncapped_tools_json_for_chat_completions(base_tools, retain_chat_names)?;
    let mut tools = cap_chat_tools(
        base_uncapped.tools,
        &base_uncapped.tool_call_info,
        retain_chat_names,
    );
    validate_tool_budgets(&tools)?;
    let identity_by_name = base_uncapped
        .tool_call_info
        .iter()
        .map(|info| (info.chat_name.clone(), info.clone()))
        .collect::<HashMap<_, _>>();
    let mut wire_info = tools
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .filter_map(|chat_name| identity_by_name.get(chat_name).cloned())
        .collect::<Vec<_>>();
    let mut index_by_name = wire_info
        .iter()
        .enumerate()
        .map(|(index, info)| (info.chat_name.clone(), index))
        .collect::<HashMap<_, _>>();
    let per_tool_limit = approx_bytes_for_tokens(MAX_CHAT_TOOL_TOKENS);
    let total_limit = approx_bytes_for_tokens(MAX_CHAT_TOOLS_TOTAL_TOKENS);
    let mut total_bytes = tool_array_bytes(&tools)?;

    // Newest discoveries first so packing prefers the latest tool_search results.
    for discovered in discovered_tools.iter().rev() {
        if tools.len() >= MAX_CHAT_TOOLS {
            break;
        }
        let piece = create_uncapped_tools_json_for_chat_completions(
            std::slice::from_ref(discovered),
            &HashSet::new(),
        )?;
        for (envelope, info) in piece.tools.into_iter().zip(piece.tool_call_info) {
            if retain_chat_names.contains(&info.chat_name) {
                // Plan schema wins: discoveries never refresh base wire contracts.
                continue;
            }
            let bytes = serde_json::to_vec(&envelope)?.len();
            if bytes > per_tool_limit {
                continue;
            }
            if let Some(&index) = index_by_name.get(&info.chat_name) {
                let previous_bytes = serde_json::to_vec(&tools[index])?.len();
                let next_total = total_bytes
                    .saturating_sub(previous_bytes)
                    .saturating_add(bytes);
                if next_total > total_limit {
                    continue;
                }
                move_tool_to_end(&mut tools, &mut wire_info, &mut index_by_name, index);
                let index = tools.len();
                tools.push(envelope);
                index_by_name.insert(info.chat_name.clone(), index);
                wire_info.push(info);
                total_bytes = next_total;
                continue;
            }
            if tools.len() >= MAX_CHAT_TOOLS {
                break;
            }
            let next_total = total_bytes.saturating_add(bytes);
            if next_total > total_limit {
                continue;
            }
            let index = tools.len();
            index_by_name.insert(info.chat_name.clone(), index);
            tools.push(envelope);
            wire_info.push(info);
            total_bytes = next_total;
        }
    }

    let mut full_specs = base_tools.to_vec();
    full_specs.extend(discovered_tools.iter().cloned());
    let tool_call_info = create_tool_call_info_for_chat_completions_retaining(
        &full_specs,
        retain_chat_names.iter().map(String::as_str),
    );
    validate_tool_budgets(&tools)?;
    Ok(ChatToolsJson {
        tools,
        tool_call_info,
    })
}

fn tool_array_bytes(tools: &[Value]) -> Result<usize, serde_json::Error> {
    let mut total = 0usize;
    for tool in tools {
        total = total.saturating_add(serde_json::to_vec(tool)?.len());
    }
    Ok(total)
}

fn move_tool_to_end(
    tools: &mut Vec<Value>,
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    index_by_name: &mut HashMap<String, usize>,
    index: usize,
) {
    let last = tools.len().saturating_sub(1);
    if index >= tools.len() {
        return;
    }
    if index != last {
        tools.swap(index, last);
        tool_call_info.swap(index, last);
        let swapped_name = tool_call_info[index].chat_name.clone();
        index_by_name.insert(swapped_name, index);
    }
    let removed_name = tool_call_info[last].chat_name.clone();
    tools.pop();
    tool_call_info.pop();
    index_by_name.remove(&removed_name);
}

fn cap_chat_tools(
    tools: Vec<Value>,
    tool_call_info: &[ChatToolCallInfo],
    retain_chat_names: &HashSet<String>,
) -> Vec<Value> {
    if tools.len() <= MAX_CHAT_TOOLS {
        return tools;
    }

    let mut keep = vec![false; tools.len()];
    let mut kept = 0usize;
    for (index, info) in tool_call_info.iter().enumerate() {
        if retain_chat_names.contains(&info.chat_name) {
            keep[index] = true;
            kept = kept.saturating_add(1);
        }
    }
    if kept > MAX_CHAT_TOOLS {
        // Prefer pinned critical base tools, then the newest retained entries so
        // move-to-end schema refreshes among non-pinned retained stay on the wire.
        kept = 0;
        keep.fill(false);
        for (index, info) in tool_call_info.iter().enumerate() {
            if retain_chat_names.contains(&info.chat_name) && is_priority_retained(info) {
                keep[index] = true;
                kept = kept.saturating_add(1);
            }
        }
        if kept > MAX_CHAT_TOOLS {
            kept = 0;
            keep.fill(false);
            for index in (0..tools.len()).rev() {
                if kept >= MAX_CHAT_TOOLS {
                    break;
                }
                let info = &tool_call_info[index];
                if !(retain_chat_names.contains(&info.chat_name) && is_priority_retained(info)) {
                    continue;
                }
                keep[index] = true;
                kept = kept.saturating_add(1);
            }
        } else {
            for index in (0..tools.len()).rev() {
                if kept >= MAX_CHAT_TOOLS {
                    break;
                }
                if keep[index] {
                    continue;
                }
                if !retain_chat_names.contains(&tool_call_info[index].chat_name) {
                    continue;
                }
                keep[index] = true;
                kept = kept.saturating_add(1);
            }
        }
    } else {
        for index in (0..tools.len()).rev() {
            if kept >= MAX_CHAT_TOOLS {
                break;
            }
            if keep[index] {
                continue;
            }
            keep[index] = true;
            kept = kept.saturating_add(1);
        }
    }

    tools
        .into_iter()
        .zip(keep)
        .filter_map(|(tool, keep)| keep.then_some(tool))
        .collect()
}

fn is_priority_retained(info: &ChatToolCallInfo) -> bool {
    matches!(info.kind, ChatToolCallKind::ToolSearch)
        || matches!(
            info.name.as_str(),
            "read_file" | "edit_file" | "write_file" | "shell" | "shell_command"
        )
}

fn cap_tool_call_info(
    tool_call_info: Vec<ChatToolCallInfo>,
    retain_chat_names: &HashSet<String>,
) -> Vec<ChatToolCallInfo> {
    if tool_call_info.len() <= MAX_CHAT_TOOL_CALL_INFO {
        return tool_call_info;
    }

    let mut keep = vec![false; tool_call_info.len()];
    let mut kept = 0usize;
    for (index, info) in tool_call_info.iter().enumerate() {
        if retain_chat_names.contains(&info.chat_name) {
            keep[index] = true;
            kept = kept.saturating_add(1);
        }
    }
    if kept > MAX_CHAT_TOOL_CALL_INFO {
        kept = 0;
        keep.fill(false);
        for index in (0..tool_call_info.len()).rev() {
            if kept >= MAX_CHAT_TOOL_CALL_INFO {
                break;
            }
            if !retain_chat_names.contains(&tool_call_info[index].chat_name) {
                continue;
            }
            keep[index] = true;
            kept = kept.saturating_add(1);
        }
    } else {
        for index in (0..tool_call_info.len()).rev() {
            if kept >= MAX_CHAT_TOOL_CALL_INFO {
                break;
            }
            if keep[index] {
                continue;
            }
            keep[index] = true;
            kept = kept.saturating_add(1);
        }
    }

    tool_call_info
        .into_iter()
        .zip(keep)
        .filter_map(|(info, keep)| keep.then_some(info))
        .collect()
}

/// Builds Chat wire identity metadata without serializing tool JSON envelopes.
pub fn chat_tool_identity_names(tools: &[ToolSpec]) -> HashSet<String> {
    chat_tool_identities(tools)
        .into_iter()
        .map(|info| info.chat_name)
        .collect()
}

/// Builds the reverse-mapping table for Chat tools without allocating wire JSON.
pub fn create_tool_call_info_for_chat_completions(tools: &[ToolSpec]) -> Vec<ChatToolCallInfo> {
    create_tool_call_info_for_chat_completions_retaining(tools, std::iter::empty::<&str>())
}

/// Like [`create_tool_call_info_for_chat_completions`], but treats `retain_chat_names`
/// as protected identities (plan schemas win over later duplicates).
pub fn create_tool_call_info_for_chat_completions_retaining(
    tools: &[ToolSpec],
    retain_chat_names: impl IntoIterator<Item = impl AsRef<str>>,
) -> Vec<ChatToolCallInfo> {
    let retain_chat_names = retain_chat_names
        .into_iter()
        .map(|name| name.as_ref().to_string())
        .collect::<HashSet<_>>();
    let info = chat_tool_identities_with_protect(tools, &retain_chat_names);
    cap_tool_call_info(info, &retain_chat_names)
}

fn chat_tool_identities(tools: &[ToolSpec]) -> Vec<ChatToolCallInfo> {
    chat_tool_identities_with_protect(tools, &HashSet::new())
}

fn chat_tool_identities_with_protect(
    tools: &[ToolSpec],
    protect_chat_names: &HashSet<String>,
) -> Vec<ChatToolCallInfo> {
    let mut tool_call_info = Vec::new();
    let mut index_by_name = HashMap::new();
    for tool in tools {
        match tool {
            ToolSpec::Function(tool) => push_identity(
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                /*namespace*/ None,
                &tool.name,
                ChatToolCallKind::Function,
            ),
            ToolSpec::Namespace(namespace) => {
                for nested in &namespace.tools {
                    match nested {
                        ResponsesApiNamespaceTool::Function(tool) => push_identity(
                            &mut tool_call_info,
                            &mut index_by_name,
                            protect_chat_names,
                            Some(namespace.name.as_str()),
                            &tool.name,
                            ChatToolCallKind::Function,
                        ),
                    }
                }
            }
            ToolSpec::ToolSearch { .. } => push_identity(
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                /*namespace*/ None,
                tool.name(),
                ChatToolCallKind::ToolSearch,
            ),
            ToolSpec::WebSearch { .. } => push_identity(
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                /*namespace*/ None,
                tool.name(),
                ChatToolCallKind::Function,
            ),
            ToolSpec::Freeform(tool) => push_identity(
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                /*namespace*/ None,
                &tool.name,
                ChatToolCallKind::Custom,
            ),
        }
    }
    tool_call_info
}

fn push_identity(
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    index_by_name: &mut HashMap<String, usize>,
    protect_chat_names: &HashSet<String>,
    namespace: Option<&str>,
    name: &str,
    kind: ChatToolCallKind,
) {
    let chat_name = chat_tool_name(namespace, name, kind);
    let info = ChatToolCallInfo {
        chat_name: chat_name.clone(),
        name: name.to_string(),
        namespace: namespace.map(str::to_string),
        kind,
    };
    if let Some(&index) = index_by_name.get(&chat_name) {
        if protect_chat_names.contains(&chat_name) {
            return;
        }
        move_identity_to_end(tool_call_info, index_by_name, index);
    }
    let index = tool_call_info.len();
    index_by_name.insert(chat_name, index);
    tool_call_info.push(info);
}

fn move_identity_to_end(
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    index_by_name: &mut HashMap<String, usize>,
    index: usize,
) {
    let last = tool_call_info.len().saturating_sub(1);
    if index >= tool_call_info.len() {
        return;
    }
    if index != last {
        tool_call_info.swap(index, last);
        let swapped_name = tool_call_info[index].chat_name.clone();
        index_by_name.insert(swapped_name, index);
    }
    let removed_name = tool_call_info[last].chat_name.clone();
    tool_call_info.pop();
    index_by_name.remove(&removed_name);
}

fn create_uncapped_tools_json_for_chat_completions(
    tools: &[ToolSpec],
    protect_chat_names: &HashSet<String>,
) -> Result<ChatToolsJson, serde_json::Error> {
    let mut chat_tools = Vec::new();
    let mut tool_call_info = Vec::new();
    let mut index_by_name = HashMap::new();

    for tool in tools {
        match tool {
            ToolSpec::Function(tool) => push_function_tool(
                &mut chat_tools,
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                /*namespace*/ None,
                tool,
            )?,
            ToolSpec::Namespace(namespace) => {
                for tool in &namespace.tools {
                    match tool {
                        ResponsesApiNamespaceTool::Function(tool) => {
                            let description =
                                format!("{}\n\n{}", namespace.description, tool.description);
                            push_tool(
                                &mut chat_tools,
                                &mut tool_call_info,
                                &mut index_by_name,
                                protect_chat_names,
                                ChatToolDefinition {
                                    namespace: Some(namespace.name.as_str()),
                                    name: &tool.name,
                                    description: &description,
                                    parameters: serde_json::to_value(&tool.parameters)?,
                                    strict: tool.strict.then_some(true),
                                    kind: ChatToolCallKind::Function,
                                },
                            );
                        }
                    }
                }
            }
            ToolSpec::ToolSearch {
                description,
                parameters,
                ..
            } => push_tool(
                &mut chat_tools,
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                ChatToolDefinition {
                    namespace: None,
                    name: tool.name(),
                    description,
                    parameters: serde_json::to_value(parameters)?,
                    strict: None,
                    kind: ChatToolCallKind::ToolSearch,
                },
            ),
            ToolSpec::WebSearch { .. } => push_tool(
                &mut chat_tools,
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                ChatToolDefinition {
                    namespace: None,
                    name: tool.name(),
                    description: "Access the web using Codex's configured search backend. Results may be bounded snippets: cite returned URLs with markdown links and use an available fetch/read-page capability when full-page context is needed.",
                    parameters: web_search_function_schema_json(),
                    strict: None,
                    kind: ChatToolCallKind::Function,
                },
            ),
            ToolSpec::Freeform(tool) => push_freeform_tool(
                &mut chat_tools,
                &mut tool_call_info,
                &mut index_by_name,
                protect_chat_names,
                tool,
            ),
        }
    }

    Ok(ChatToolsJson {
        tools: chat_tools,
        tool_call_info,
    })
}

fn validate_tool_budgets(tools: &[Value]) -> Result<(), serde_json::Error> {
    let per_tool_limit = approx_bytes_for_tokens(MAX_CHAT_TOOL_TOKENS);
    let total_limit = approx_bytes_for_tokens(MAX_CHAT_TOOLS_TOTAL_TOKENS);
    let mut total_bytes = 0usize;
    for tool in tools {
        let bytes = serde_json::to_vec(tool)?.len();
        if bytes > per_tool_limit {
            let name = tool
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            return Err(serde_json::Error::io(io::Error::other(format!(
                "Chat tool `{name}` exceeds the {MAX_CHAT_TOOL_TOKENS}-token context budget"
            ))));
        }
        total_bytes = total_bytes.saturating_add(bytes);
    }
    if total_bytes > total_limit {
        return Err(serde_json::Error::io(io::Error::other(format!(
            "Chat tools exceed the {MAX_CHAT_TOOLS_TOTAL_TOKENS}-token total context budget"
        ))));
    }
    Ok(())
}

fn push_function_tool(
    chat_tools: &mut Vec<Value>,
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    index_by_name: &mut HashMap<String, usize>,
    protect_chat_names: &HashSet<String>,
    namespace: Option<&str>,
    tool: &ResponsesApiTool,
) -> Result<(), serde_json::Error> {
    push_tool(
        chat_tools,
        tool_call_info,
        index_by_name,
        protect_chat_names,
        ChatToolDefinition {
            namespace,
            name: &tool.name,
            description: &tool.description,
            parameters: serde_json::to_value(&tool.parameters)?,
            strict: tool.strict.then_some(true),
            kind: ChatToolCallKind::Function,
        },
    );
    Ok(())
}

fn push_freeform_tool(
    chat_tools: &mut Vec<Value>,
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    index_by_name: &mut HashMap<String, usize>,
    protect_chat_names: &HashSet<String>,
    tool: &FreeformTool,
) {
    let description = format!(
        "{}\n\nPass the raw {} {} body in the `input` string.",
        tool.description, tool.format.syntax, tool.format.r#type
    );
    push_tool(
        chat_tools,
        tool_call_info,
        index_by_name,
        protect_chat_names,
        ChatToolDefinition {
            namespace: None,
            name: &tool.name,
            description: &description,
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Raw freeform tool input."
                    }
                },
                "required": ["input"],
                "additionalProperties": false
            }),
            strict: None,
            kind: ChatToolCallKind::Custom,
        },
    );
}

struct ChatToolDefinition<'a> {
    namespace: Option<&'a str>,
    name: &'a str,
    description: &'a str,
    parameters: Value,
    strict: Option<bool>,
    kind: ChatToolCallKind,
}

fn push_tool(
    chat_tools: &mut Vec<Value>,
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    index_by_name: &mut HashMap<String, usize>,
    protect_chat_names: &HashSet<String>,
    tool: ChatToolDefinition<'_>,
) {
    let chat_name = chat_tool_name(tool.namespace, tool.name, tool.kind);
    if index_by_name.contains_key(&chat_name) && protect_chat_names.contains(&chat_name) {
        // Plan / retained identity wins: ignore later schema refreshes.
        return;
    }

    let mut function = json!({
        "name": chat_name,
        "description": tool.description,
        "parameters": tool.parameters
    });
    if let Some(strict) = tool.strict {
        function["strict"] = json!(strict);
    }
    let envelope = json!({"type": "function", "function": function});
    let info = ChatToolCallInfo {
        chat_name: chat_name.clone(),
        name: tool.name.to_string(),
        namespace: tool.namespace.map(str::to_string),
        kind: tool.kind,
    };
    if let Some(&index) = index_by_name.get(&chat_name) {
        // Latest declaration wins and moves to the end so refreshed tools stay
        // among the newest entries when the provider cap keeps a suffix.
        move_tool_to_end(chat_tools, tool_call_info, index_by_name, index);
    }
    let index = chat_tools.len();
    index_by_name.insert(chat_name, index);
    chat_tools.push(envelope);
    tool_call_info.push(info);
}

/// Returns the stable Chat Completions wire name for a Codex tool identity.
///
/// The result depends only on the original namespace, name, and kind so adding,
/// removing, or reordering other tools cannot rewrite historical Chat messages.
pub fn chat_tool_name(namespace: Option<&str>, name: &str, kind: ChatToolCallKind) -> String {
    let raw = namespace.map_or_else(
        || name.to_string(),
        |namespace| format!("{namespace}__{name}"),
    );
    let base = sanitize_chat_tool_name(&raw);
    let suffix = format!("__{:016x}", chat_tool_identity_hash(namespace, name, kind));
    let keep = MAX_CHAT_TOOL_NAME_LEN.saturating_sub(suffix.len());
    format!("{}{suffix}", &base[..base.len().min(keep)])
}

fn sanitize_chat_tool_name(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized
    }
}

fn chat_tool_identity_hash(namespace: Option<&str>, name: &str, kind: ChatToolCallKind) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    let mut update = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(FNV_PRIME);
    };
    update(&[u8::from(namespace.is_some())]);
    update(namespace.unwrap_or_default().as_bytes());
    update(name.as_bytes());
    update(&[match kind {
        ChatToolCallKind::Function => 0,
        ChatToolCallKind::Custom => 1,
        ChatToolCallKind::ToolSearch => 2,
    }]);
    hash
}

#[cfg(test)]
#[path = "chat_completions_tests.rs"]
mod tests;
