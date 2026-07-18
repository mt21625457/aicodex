use crate::FreeformTool;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::ToolSpec;
use crate::tool_spec::web_search_function_schema_json;
use codex_utils_string::approx_bytes_for_tokens;
use serde_json::Value;
use serde_json::json;
use std::io;

const MAX_CHAT_TOOL_NAME_LEN: usize = 64;
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

/// Serializes every Codex tool kind as a Chat Completions function tool.
///
/// The side table preserves the original Codex kind and namespace so streamed
/// function calls can be restored to the correct `ResponseItem` variant.
pub fn create_tools_json_for_chat_completions(
    tools: &[ToolSpec],
) -> Result<ChatToolsJson, serde_json::Error> {
    let mut chat_tools = Vec::new();
    let mut tool_call_info = Vec::new();

    for tool in tools {
        match tool {
            ToolSpec::Function(tool) => push_function_tool(
                &mut chat_tools,
                &mut tool_call_info,
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
                                ChatToolDefinition {
                                    namespace: Some(namespace.name.as_str()),
                                    name: &tool.name,
                                    description: &description,
                                    parameters: serde_json::to_value(&tool.parameters)?,
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
                ChatToolDefinition {
                    namespace: None,
                    name: tool.name(),
                    description,
                    parameters: serde_json::to_value(parameters)?,
                    kind: ChatToolCallKind::ToolSearch,
                },
            ),
            ToolSpec::WebSearch { .. } => push_tool(
                &mut chat_tools,
                &mut tool_call_info,
                ChatToolDefinition {
                    namespace: None,
                    name: tool.name(),
                    description: "Access the web using Codex's configured search backend. Results may be bounded snippets: cite returned URLs with markdown links and use an available fetch/read-page capability when full-page context is needed.",
                    parameters: web_search_function_schema_json(),
                    kind: ChatToolCallKind::Function,
                },
            ),
            ToolSpec::Freeform(tool) => {
                push_freeform_tool(&mut chat_tools, &mut tool_call_info, tool)
            }
        }
    }

    validate_tool_budgets(&chat_tools)?;

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
    namespace: Option<&str>,
    tool: &ResponsesApiTool,
) -> Result<(), serde_json::Error> {
    push_tool(
        chat_tools,
        tool_call_info,
        ChatToolDefinition {
            namespace,
            name: &tool.name,
            description: &tool.description,
            parameters: serde_json::to_value(&tool.parameters)?,
            kind: ChatToolCallKind::Function,
        },
    );
    Ok(())
}

fn push_freeform_tool(
    chat_tools: &mut Vec<Value>,
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    tool: &FreeformTool,
) {
    let description = format!(
        "{}\n\nPass the raw {} {} body in the `input` string.",
        tool.description, tool.format.syntax, tool.format.r#type
    );
    push_tool(
        chat_tools,
        tool_call_info,
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
            kind: ChatToolCallKind::Custom,
        },
    );
}

struct ChatToolDefinition<'a> {
    namespace: Option<&'a str>,
    name: &'a str,
    description: &'a str,
    parameters: Value,
    kind: ChatToolCallKind,
}

fn push_tool(
    chat_tools: &mut Vec<Value>,
    tool_call_info: &mut Vec<ChatToolCallInfo>,
    tool: ChatToolDefinition<'_>,
) {
    let chat_name = chat_tool_name(tool.namespace, tool.name, tool.kind);
    chat_tools.push(json!({
        "type": "function",
        "function": {
            "name": chat_name,
            "description": tool.description,
            "parameters": tool.parameters
        }
    }));
    tool_call_info.push(ChatToolCallInfo {
        chat_name,
        name: tool.name.to_string(),
        namespace: tool.namespace.map(str::to_string),
        kind: tool.kind,
    });
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
