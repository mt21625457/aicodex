use crate::FreeformTool;
use crate::JsonSchema;
use crate::LoadableToolSpec;
use crate::ResponsesApiNamespace;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;

const MAX_CLAUDE_TOOL_NAME_LEN: usize = 64;
const CLAUDE_WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";

const APPLY_PATCH_CLAUDE_TOOL_DESCRIPTION: &str = r#"Use the `apply_patch` tool to edit files.
Your patch language is a stripped-down, file-oriented diff format designed to be easy to parse and safe to apply:

*** Begin Patch
[ one or more file sections ]
*** End Patch

Each operation starts with one of three headers:

*** Add File: <path> - create a new file. Every following line is a + line.
*** Delete File: <path> - remove an existing file.
*** Update File: <path> - patch an existing file in place.

A full patch can combine several operations:

*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch

Remember: include an Add/Delete/Update header, prefix new lines with `+`, and use relative file paths."#;

/// When serialized as JSON, this produces a valid "Tool" in the OpenAI
/// Responses API.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum ToolSpec {
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
    #[serde(rename = "namespace")]
    Namespace(ResponsesApiNamespace),
    #[serde(rename = "tool_search")]
    ToolSearch {
        execution: String,
        description: String,
        parameters: JsonSchema,
    },
    #[serde(rename = "local_shell")]
    LocalShell {},
    #[serde(rename = "image_generation")]
    ImageGeneration { output_format: String },
    // TODO: Understand why we get an error on web_search although the API docs
    // say it's supported.
    // https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses#:~:text=%7B%20type%3A%20%22web_search%22%20%7D%2C
    // The `external_web_access` field determines whether the web search is over
    // cached or live content.
    // https://platform.openai.com/docs/guides/tools-web-search#live-internet-access
    #[serde(rename = "web_search")]
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        external_web_access: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<ResponsesApiWebSearchFilters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<ResponsesApiWebSearchUserLocation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<WebSearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_content_types: Option<Vec<String>>,
    },
    #[serde(rename = "custom")]
    Freeform(FreeformTool),
}

impl ToolSpec {
    pub fn name(&self) -> &str {
        match self {
            ToolSpec::Function(tool) => tool.name.as_str(),
            ToolSpec::Namespace(namespace) => namespace.name.as_str(),
            ToolSpec::ToolSearch { .. } => "tool_search",
            ToolSpec::LocalShell {} => "local_shell",
            ToolSpec::ImageGeneration { .. } => "image_generation",
            ToolSpec::WebSearch { .. } => "web_search",
            ToolSpec::Freeform(tool) => tool.name.as_str(),
        }
    }
}

impl From<LoadableToolSpec> for ToolSpec {
    fn from(value: LoadableToolSpec) -> Self {
        match value {
            LoadableToolSpec::Function(tool) => ToolSpec::Function(tool),
            LoadableToolSpec::Namespace(namespace) => ToolSpec::Namespace(namespace),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredToolSpec {
    pub spec: ToolSpec,
    pub supports_parallel_tool_calls: bool,
}

impl ConfiguredToolSpec {
    pub fn new(spec: ToolSpec, supports_parallel_tool_calls: bool) -> Self {
        Self {
            spec,
            supports_parallel_tool_calls,
        }
    }

    pub fn name(&self) -> &str {
        self.spec.name()
    }
}

/// Returns JSON values that are compatible with Function Calling in the
/// Responses API:
/// https://platform.openai.com/docs/guides/function-calling?api-mode=responses
pub fn create_tools_json_for_responses_api(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let mut tools_json = Vec::new();

    for tool in tools {
        let json = serde_json::to_value(tool)?;
        tools_json.push(json);
    }

    Ok(tools_json)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeToolCallKind {
    Function,
    Custom,
    ToolSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeToolCallInfo {
    pub claude_name: String,
    pub name: String,
    pub namespace: Option<String>,
    pub kind: ClaudeToolCallKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeToolsJson {
    pub tools: Vec<Value>,
    pub tool_call_info: Vec<ClaudeToolCallInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeWebSearchToolKind {
    NativeServerTool,
    LocalFunctionTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeMessagesToolOptions {
    pub web_search_tool_kind: ClaudeWebSearchToolKind,
}

impl Default for ClaudeMessagesToolOptions {
    fn default() -> Self {
        Self {
            web_search_tool_kind: ClaudeWebSearchToolKind::NativeServerTool,
        }
    }
}

/// Returns JSON values compatible with Anthropic's Claude Messages API.
///
/// Claude does not have Responses API namespaces or custom/freeform tools, so
/// this flattens namespaced tools into valid Claude tool names and returns a
/// side table that callers can use to map streamed `tool_use.name` values back
/// to Codex's internal `ResponseItem` shape.
pub fn create_tools_json_for_claude_messages(
    tools: &[ToolSpec],
) -> Result<ClaudeToolsJson, serde_json::Error> {
    create_tools_json_for_claude_messages_with_options(tools, ClaudeMessagesToolOptions::default())
}

pub fn create_tools_json_for_claude_messages_with_options(
    tools: &[ToolSpec],
    options: ClaudeMessagesToolOptions,
) -> Result<ClaudeToolsJson, serde_json::Error> {
    let mut claude_tools = Vec::new();
    let mut tool_call_info = Vec::new();
    let mut used_names = HashSet::new();

    for tool in tools {
        match tool {
            ToolSpec::Function(function_tool) => {
                let claude_name = unique_claude_tool_name(
                    &mut used_names,
                    /*namespace*/ None,
                    &function_tool.name,
                );
                claude_tools.push(claude_function_tool_json(
                    &claude_name,
                    &function_tool.description,
                    serde_json::to_value(&function_tool.parameters)?,
                ));
                tool_call_info.push(ClaudeToolCallInfo {
                    claude_name,
                    name: function_tool.name.clone(),
                    namespace: None,
                    kind: ClaudeToolCallKind::Function,
                });
            }
            ToolSpec::Namespace(namespace) => {
                for tool in &namespace.tools {
                    match tool {
                        ResponsesApiNamespaceTool::Function(function_tool) => {
                            let claude_name = unique_claude_tool_name(
                                &mut used_names,
                                Some(namespace.name.as_str()),
                                &function_tool.name,
                            );
                            let description = format!(
                                "{}\n\n{}",
                                namespace.description, function_tool.description
                            );
                            claude_tools.push(claude_function_tool_json(
                                &claude_name,
                                &description,
                                serde_json::to_value(&function_tool.parameters)?,
                            ));
                            tool_call_info.push(ClaudeToolCallInfo {
                                claude_name,
                                name: function_tool.name.clone(),
                                namespace: Some(namespace.name.clone()),
                                kind: ClaudeToolCallKind::Function,
                            });
                        }
                    }
                }
            }
            ToolSpec::ToolSearch {
                description,
                parameters,
                ..
            } => {
                let claude_name =
                    unique_claude_tool_name(&mut used_names, /*namespace*/ None, tool.name());
                claude_tools.push(claude_function_tool_json(
                    &claude_name,
                    description,
                    serde_json::to_value(parameters)?,
                ));
                tool_call_info.push(ClaudeToolCallInfo {
                    claude_name,
                    name: tool.name().to_string(),
                    namespace: None,
                    kind: ClaudeToolCallKind::ToolSearch,
                });
            }
            ToolSpec::LocalShell {} => {
                let claude_name =
                    unique_claude_tool_name(&mut used_names, /*namespace*/ None, tool.name());
                claude_tools.push(claude_function_tool_json(
                    &claude_name,
                    "Runs a local shell command and returns its output.",
                    json!({
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "array",
                                "description": "Command and arguments to execute locally.",
                                "items": { "type": "string" }
                            },
                            "workdir": {
                                "type": "string",
                                "description": "Optional working directory for the command."
                            },
                            "timeout_ms": {
                                "type": "number",
                                "description": "Optional command timeout in milliseconds."
                            },
                            "sandbox_permissions": {
                                "type": "string",
                                "description": "Sandbox permissions for the command."
                            },
                            "additional_permissions": claude_permission_profile_schema_json(),
                            "justification": {
                                "type": "string",
                                "description": "Optional justification when requesting escalated permissions."
                            },
                            "prefix_rule": {
                                "type": "array",
                                "description": "Optional command prefix rule for similar future commands.",
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["command"],
                        "additionalProperties": false
                    }),
                ));
                tool_call_info.push(ClaudeToolCallInfo {
                    claude_name,
                    name: tool.name().to_string(),
                    namespace: None,
                    kind: ClaudeToolCallKind::Function,
                });
            }
            ToolSpec::WebSearch {
                filters,
                user_location,
                ..
            } => {
                match options.web_search_tool_kind {
                    ClaudeWebSearchToolKind::NativeServerTool => {
                        claude_tools.push(claude_web_search_tool_json(filters, user_location));
                    }
                    ClaudeWebSearchToolKind::LocalFunctionTool => {
                        let claude_name = unique_claude_tool_name(
                            &mut used_names,
                            /*namespace*/ None,
                            tool.name(),
                        );
                        claude_tools.push(claude_function_tool_json(
                            &claude_name,
                            "Search the web using Codex's local web search handler and return relevant text results. Use `query` for one search or `queries` for a small batch.",
                            claude_web_search_function_schema_json(),
                        ));
                        tool_call_info.push(ClaudeToolCallInfo {
                            claude_name,
                            name: tool.name().to_string(),
                            namespace: None,
                            kind: ClaudeToolCallKind::Function,
                        });
                    }
                }
            }
            ToolSpec::ImageGeneration { .. } => {}
            ToolSpec::Freeform(tool) => {
                let claude_name =
                    unique_claude_tool_name(&mut used_names, /*namespace*/ None, &tool.name);
                claude_tools.push(claude_function_tool_json(
                    &claude_name,
                    &claude_freeform_tool_description(tool),
                    json!({
                        "type": "object",
                        "properties": {
                            "input": {
                                "type": "string",
                                "description": claude_freeform_input_description(tool)
                            }
                        },
                        "required": ["input"],
                        "additionalProperties": false
                    }),
                ));
                tool_call_info.push(ClaudeToolCallInfo {
                    claude_name,
                    name: tool.name.clone(),
                    namespace: None,
                    kind: ClaudeToolCallKind::Custom,
                });
            }
        }
    }

    Ok(ClaudeToolsJson {
        tools: claude_tools,
        tool_call_info,
    })
}

pub fn claude_tool_name(namespace: Option<&str>, name: &str) -> String {
    sanitize_claude_tool_name(&claude_tool_raw_name(namespace, name))
}

fn unique_claude_tool_name(
    used_names: &mut HashSet<String>,
    namespace: Option<&str>,
    name: &str,
) -> String {
    let raw = claude_tool_raw_name(namespace, name);
    let base = sanitize_claude_tool_name(&raw);
    if used_names.insert(base.clone()) {
        return base;
    }

    let hash = fnv1a64(raw.as_bytes());
    for disambiguator in 1u64.. {
        let suffix = if disambiguator == 1 {
            format!("_{hash:016x}")
        } else {
            format!("_{hash:016x}_{disambiguator}")
        };
        let candidate = append_bounded_suffix(&base, &suffix);
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
    }

    unreachable!("unbounded disambiguator loop should always return")
}

fn claude_tool_raw_name(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(namespace) => format!("{namespace}{name}"),
        None => name.to_string(),
    }
}

fn claude_function_tool_json(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": input_schema,
    })
}

fn claude_web_search_tool_json(
    filters: &Option<ResponsesApiWebSearchFilters>,
    user_location: &Option<ResponsesApiWebSearchUserLocation>,
) -> Value {
    let mut tool = Map::new();
    tool.insert("type".to_string(), json!(CLAUDE_WEB_SEARCH_TOOL_TYPE));
    tool.insert("name".to_string(), json!("web_search"));
    if let Some(allowed_domains) = filters
        .as_ref()
        .and_then(|filters| filters.allowed_domains.as_ref())
        .filter(|domains| !domains.is_empty())
    {
        tool.insert("allowed_domains".to_string(), json!(allowed_domains));
    }
    if let Some(user_location) = user_location {
        tool.insert("user_location".to_string(), json!(user_location));
    }
    Value::Object(tool)
}

fn claude_web_search_function_schema_json() -> Value {
    json!({
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
    })
}

fn claude_permission_profile_schema_json() -> Value {
    json!({
        "type": "object",
        "properties": {
            "network": {
                "type": "object",
                "properties": {
                    "enabled": {
                        "type": "boolean",
                        "description": "Set to true to request network access."
                    }
                },
                "additionalProperties": false
            },
            "file_system": {
                "type": "object",
                "properties": {
                    "read": {
                        "type": "array",
                        "description": "Absolute paths to grant read access to.",
                        "items": { "type": "string" }
                    },
                    "write": {
                        "type": "array",
                        "description": "Absolute paths to grant write access to.",
                        "items": { "type": "string" }
                    }
                },
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

fn claude_freeform_tool_description(tool: &FreeformTool) -> String {
    let wrapper_guidance = "Claude must call this freeform tool through a JSON `tool_use.input` object. Put the raw freeform body in the nested `input` string. If any tool-specific text says not to wrap the body in JSON, that applies only to the nested `input` string; the Claude tool call itself is still JSON.";
    match tool.name.as_str() {
        "apply_patch" => format!("{APPLY_PATCH_CLAUDE_TOOL_DESCRIPTION}\n\n{wrapper_guidance}"),
        _ => format!(
            "{}\n\n{}\n\nFreeform input format ({} {}):\n```{}\n{}\n```",
            tool.description,
            wrapper_guidance,
            tool.format.syntax,
            tool.format.r#type,
            tool.format.syntax,
            tool.format.definition
        ),
    }
}

fn claude_freeform_input_description(tool: &FreeformTool) -> String {
    match tool.name.as_str() {
        "apply_patch" => "The entire raw apply_patch patch body. It must start with `*** Begin Patch`, contain one or more file sections such as `*** Add File: <path>`, and end with `*** End Patch`. For add-file sections, every content line must start with `+`.".to_string(),
        "exec" => "Raw JavaScript source text for Code Mode exec. The value is the raw source string inside Claude's JSON tool call, not a Markdown code fence or a JSON-quoted program. It may start with an optional first-line pragma like `// @exec: {\"yield_time_ms\": 10000, \"max_output_tokens\": 1000}` followed by JavaScript source on subsequent lines.".to_string(),
        _ => format!(
            "Raw freeform body for `{}`. The value is the raw string inside Claude's JSON tool call and must follow the tool's {} {} format.",
            tool.name, tool.format.syntax, tool.format.r#type
        ),
    }
}

fn sanitize_claude_tool_name(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('_');
    let sanitized = if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized.to_string()
    };

    if sanitized.len() <= MAX_CLAUDE_TOOL_NAME_LEN {
        return sanitized;
    }

    let hash = fnv1a64(raw.as_bytes());
    let suffix = format!("_{hash:016x}");
    append_bounded_suffix(&sanitized, &suffix)
}

fn append_bounded_suffix(base: &str, suffix: &str) -> String {
    let prefix_len = MAX_CLAUDE_TOOL_NAME_LEN.saturating_sub(suffix.len());
    let mut prefix_end = prefix_len.min(base.len());
    while !base.is_char_boundary(prefix_end) {
        prefix_end -= 1;
    }
    format!("{}{}", &base[..prefix_end], suffix)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
}

impl From<ConfigWebSearchFilters> for ResponsesApiWebSearchFilters {
    fn from(filters: ConfigWebSearchFilters) -> Self {
        Self {
            allowed_domains: filters.allowed_domains,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchUserLocation {
    #[serde(rename = "type")]
    pub r#type: WebSearchUserLocationType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl From<ConfigWebSearchUserLocation> for ResponsesApiWebSearchUserLocation {
    fn from(user_location: ConfigWebSearchUserLocation) -> Self {
        Self {
            r#type: user_location.r#type,
            country: user_location.country,
            region: user_location.region,
            city: user_location.city,
            timezone: user_location.timezone,
        }
    }
}

#[cfg(test)]
#[path = "tool_spec_tests.rs"]
mod tests;
