use crate::FreeformTool;
use crate::JsonSchema;
use crate::LoadableToolSpec;
use crate::ResponsesApiNamespace;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::claude_native::CLAUDE_ADVISOR_TOOL_NAME;
use crate::claude_native::CLAUDE_BASH_TOOL_NAME;
use crate::claude_native::CLAUDE_CODE_EXECUTION_TOOL_NAME;
use crate::claude_native::CLAUDE_COMPUTER_TOOL_NAME;
use crate::claude_native::CLAUDE_MEMORY_TOOL_NAME;
use crate::claude_native::CLAUDE_TEXT_EDITOR_TOOL_NAME;
use crate::claude_native::CLAUDE_TOOL_SEARCH_BM25_TOOL_NAME;
use crate::claude_native::CLAUDE_TOOL_SEARCH_REGEX_TOOL_NAME;
use crate::claude_native::CLAUDE_WEB_FETCH_TOOL_NAME;
use crate::claude_native::CLAUDE_WEB_SEARCH_TOOL_NAME;
use crate::claude_native::ClaudeBetaFeature;
use crate::claude_native::ClaudeHistoryRequirements;
use crate::claude_native::ClaudeMcpServer;
use crate::claude_native::ClaudeNativeToolDecision;
use crate::claude_native::ClaudeNativeToolDecisionOutcome;
use crate::claude_native::ClaudeNativeToolKind;
use crate::claude_native::ClaudeNativeToolPolicy;
use crate::claude_native::ClaudeNativeToolSelection;
use crate::claude_native::evaluate_native_tool;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeSet;
use std::collections::HashSet;

const MAX_CLAUDE_TOOL_NAME_LEN: usize = 64;

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
    pub mcp_servers: Vec<ClaudeMcpServer>,
    pub beta_headers: BTreeSet<ClaudeBetaFeature>,
    pub native_tool_policy: ClaudeNativeToolPolicy,
    pub history_requirements: ClaudeHistoryRequirements,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeWebSearchToolKind {
    NativeServerTool,
    LocalFunctionTool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeMessagesToolOptions {
    pub web_search_tool_kind: ClaudeWebSearchToolKind,
    pub native_tool_selection: ClaudeNativeToolSelection,
}

impl Default for ClaudeMessagesToolOptions {
    fn default() -> Self {
        Self {
            web_search_tool_kind: ClaudeWebSearchToolKind::NativeServerTool,
            native_tool_selection: ClaudeNativeToolSelection::default(),
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
    let mut mcp_servers = Vec::new();
    let mut beta_headers = BTreeSet::new();
    let mut native_tool_policy = ClaudeNativeToolPolicy::default();
    let mut history_requirements = ClaudeHistoryRequirements::default();
    let mut used_names = HashSet::new();

    let mut native_tool_recorder = NativeToolRecorder {
        claude_tools: &mut claude_tools,
        tool_call_info: &mut tool_call_info,
        mcp_servers: &mut mcp_servers,
        beta_headers: &mut beta_headers,
        native_tool_policy: &mut native_tool_policy,
        history_requirements: &mut history_requirements,
        used_names: &mut used_names,
    };
    record_selected_native_tools(&mut native_tool_recorder, &options.native_tool_selection);

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
            ToolSpec::WebSearch {
                filters,
                user_location,
                ..
            } => {
                match options.web_search_tool_kind {
                    ClaudeWebSearchToolKind::NativeServerTool => {
                        native_tool_policy
                            .enabled_tools
                            .insert(ClaudeNativeToolKind::WebSearch20250305);
                        native_tool_policy.decisions.push(ClaudeNativeToolDecision {
                            tool: ClaudeNativeToolKind::WebSearch20250305,
                            outcome: ClaudeNativeToolDecisionOutcome::Enabled,
                            reason: "Claude native server web search selected",
                        });
                        history_requirements.preserve_server_tool_results = true;
                        history_requirements.preserve_structured_citations = true;
                        claude_tools.push(claude_web_search_tool_json(filters, user_location));
                    }
                    ClaudeWebSearchToolKind::LocalFunctionTool => {
                        native_tool_policy
                            .fallback_tools
                            .insert(ClaudeNativeToolKind::WebSearch20250305);
                        native_tool_policy.decisions.push(ClaudeNativeToolDecision {
                            tool: ClaudeNativeToolKind::WebSearch20250305,
                            outcome: ClaudeNativeToolDecisionOutcome::Fallback,
                            reason: "provider compatibility selected local web search function",
                        });
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
    if native_tool_policy
        .enabled_tools
        .contains(&ClaudeNativeToolKind::ToolSearchRegex20251119)
        || native_tool_policy
            .enabled_tools
            .contains(&ClaudeNativeToolKind::ToolSearchBm25V20251119)
    {
        apply_deferred_loading_policy(&mut claude_tools);
    }

    Ok(ClaudeToolsJson {
        tools: claude_tools,
        tool_call_info,
        mcp_servers,
        beta_headers,
        native_tool_policy,
        history_requirements,
    })
}

fn apply_deferred_loading_policy(tools: &mut [Value]) {
    let mut ordinary_tools_seen = 0usize;
    let mut non_deferred_ordinary_tools = 0usize;
    for tool in &mut *tools {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
        if is_native_tool_search_name(name) {
            continue;
        }
        if is_provider_side_tool(tool) {
            continue;
        }
        ordinary_tools_seen = ordinary_tools_seen.saturating_add(1);
        if ordinary_tools_seen <= 3 {
            non_deferred_ordinary_tools = non_deferred_ordinary_tools.saturating_add(1);
            continue;
        }
        if let Some(object) = tool.as_object_mut() {
            object.insert("defer_loading".to_string(), json!(true));
        }
    }
    if non_deferred_ordinary_tools == 0
        && let Some(tool) = tools.iter_mut().find(|tool| {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
            !is_native_tool_search_name(name) && !is_provider_side_tool(tool)
        })
        && let Some(object) = tool.as_object_mut()
    {
        object.remove("defer_loading");
    }
}

fn is_native_tool_search_name(name: &str) -> bool {
    matches!(
        name,
        CLAUDE_TOOL_SEARCH_REGEX_TOOL_NAME | CLAUDE_TOOL_SEARCH_BM25_TOOL_NAME
    )
}

fn is_provider_side_tool(tool: &Value) -> bool {
    tool.get("type")
        .and_then(Value::as_str)
        .is_some_and(|tool_type| {
            tool_type.starts_with("web_search_")
                || tool_type.starts_with("web_fetch_")
                || tool_type.starts_with("code_execution_")
                || tool_type.starts_with("advisor_")
                || tool_type == ClaudeNativeToolKind::McpToolset.tool_type()
        })
}

struct NativeToolRecorder<'a> {
    claude_tools: &'a mut Vec<Value>,
    tool_call_info: &'a mut Vec<ClaudeToolCallInfo>,
    mcp_servers: &'a mut Vec<ClaudeMcpServer>,
    beta_headers: &'a mut BTreeSet<ClaudeBetaFeature>,
    native_tool_policy: &'a mut ClaudeNativeToolPolicy,
    history_requirements: &'a mut ClaudeHistoryRequirements,
    used_names: &'a mut HashSet<String>,
}

fn record_selected_native_tools(
    recorder: &mut NativeToolRecorder<'_>,
    selection: &ClaudeNativeToolSelection,
) {
    for tool in selection.allowed_tools.iter().copied() {
        let decision = evaluate_native_tool(tool, selection);
        match decision.outcome {
            ClaudeNativeToolDecisionOutcome::Enabled => {
                let name = claude_native_tool_name(tool);
                if tool == ClaudeNativeToolKind::McpToolset {
                    let Some(valid_servers) =
                        valid_remote_mcp_servers(&selection.remote_mcp_servers)
                    else {
                        recorder.native_tool_policy.disabled_tools.insert(tool);
                        let disabled_decision = ClaudeNativeToolDecision {
                            tool,
                            outcome: ClaudeNativeToolDecisionOutcome::Disabled,
                            reason: "Claude native remote MCP requires unique HTTPS remote servers",
                        };
                        recorder
                            .native_tool_policy
                            .decisions
                            .push(disabled_decision);
                        continue;
                    };
                    for server in valid_servers {
                        recorder.mcp_servers.push(server.clone());
                        recorder.claude_tools.push(claude_mcp_toolset_json(server));
                    }
                    if let Some(beta_feature) = tool.beta_feature() {
                        recorder.beta_headers.insert(beta_feature);
                    }
                    recorder.native_tool_policy.enabled_tools.insert(tool);
                    recorder.history_requirements.preserve_mcp_tool_results = true;
                    recorder.history_requirements.preserve_server_tool_results = true;
                } else if recorder.used_names.insert(name.to_string()) {
                    recorder
                        .claude_tools
                        .push(claude_native_tool_json(tool, selection));
                    if let Some(beta_feature) = tool.beta_feature() {
                        recorder.beta_headers.insert(beta_feature);
                    }
                    recorder.native_tool_policy.enabled_tools.insert(tool);
                    if tool.execution() == crate::claude_native::ClaudeNativeToolExecution::Client {
                        recorder.tool_call_info.push(ClaudeToolCallInfo {
                            claude_name: name.to_string(),
                            name: name.to_string(),
                            namespace: None,
                            kind: ClaudeToolCallKind::Function,
                        });
                    } else {
                        recorder.history_requirements.preserve_server_tool_results = true;
                    }
                    if matches!(
                        tool,
                        ClaudeNativeToolKind::WebSearch20250305
                            | ClaudeNativeToolKind::WebSearch20260209
                    ) {
                        recorder.history_requirements.preserve_structured_citations = true;
                    }
                } else {
                    recorder.native_tool_policy.disabled_tools.insert(tool);
                    recorder
                        .native_tool_policy
                        .decisions
                        .push(ClaudeNativeToolDecision {
                            tool,
                            outcome: ClaudeNativeToolDecisionOutcome::Disabled,
                            reason: "native Claude tool name collides with an earlier tool",
                        });
                    continue;
                }
            }
            ClaudeNativeToolDecisionOutcome::Fallback => {
                recorder.native_tool_policy.fallback_tools.insert(tool);
            }
            ClaudeNativeToolDecisionOutcome::Disabled => {
                recorder.native_tool_policy.disabled_tools.insert(tool);
            }
        }
        recorder.native_tool_policy.decisions.push(decision);
    }
}

fn claude_native_tool_name(tool: ClaudeNativeToolKind) -> &'static str {
    match tool {
        ClaudeNativeToolKind::WebSearch20250305 | ClaudeNativeToolKind::WebSearch20260209 => {
            CLAUDE_WEB_SEARCH_TOOL_NAME
        }
        ClaudeNativeToolKind::WebFetch20250910 | ClaudeNativeToolKind::WebFetch20260209 => {
            CLAUDE_WEB_FETCH_TOOL_NAME
        }
        ClaudeNativeToolKind::CodeExecution20250825
        | ClaudeNativeToolKind::CodeExecution20260120 => CLAUDE_CODE_EXECUTION_TOOL_NAME,
        ClaudeNativeToolKind::Advisor20260301 => CLAUDE_ADVISOR_TOOL_NAME,
        ClaudeNativeToolKind::ToolSearchRegex20251119 => CLAUDE_TOOL_SEARCH_REGEX_TOOL_NAME,
        ClaudeNativeToolKind::ToolSearchBm25V20251119 => CLAUDE_TOOL_SEARCH_BM25_TOOL_NAME,
        ClaudeNativeToolKind::McpToolset => "mcp_toolset",
        ClaudeNativeToolKind::Memory20250818 => CLAUDE_MEMORY_TOOL_NAME,
        ClaudeNativeToolKind::Bash20250124 => CLAUDE_BASH_TOOL_NAME,
        ClaudeNativeToolKind::TextEditor20250728 | ClaudeNativeToolKind::TextEditor20250124 => {
            CLAUDE_TEXT_EDITOR_TOOL_NAME
        }
        ClaudeNativeToolKind::Computer20251124 | ClaudeNativeToolKind::Computer20250124 => {
            CLAUDE_COMPUTER_TOOL_NAME
        }
    }
}

fn claude_native_tool_json(
    tool: ClaudeNativeToolKind,
    selection: &ClaudeNativeToolSelection,
) -> Value {
    let mut value = Map::new();
    value.insert("type".to_string(), json!(tool.tool_type()));
    value.insert("name".to_string(), json!(claude_native_tool_name(tool)));
    if tool == ClaudeNativeToolKind::TextEditor20250728 {
        value.insert("max_characters".to_string(), json!(20_000));
    }
    if tool == ClaudeNativeToolKind::Advisor20260301 {
        if let Some(model) = &selection.advisor_model {
            value.insert("model".to_string(), json!(model));
        }
        if let Some(max_uses) = selection.advisor_max_uses {
            value.insert("max_uses".to_string(), json!(max_uses));
        }
    }
    Value::Object(value)
}

fn claude_mcp_toolset_json(server: &ClaudeMcpServer) -> Value {
    let mut tool = Map::new();
    tool.insert(
        "type".to_string(),
        json!(ClaudeNativeToolKind::McpToolset.tool_type()),
    );
    tool.insert("mcp_server_name".to_string(), json!(server.name));
    if let Some(default_config) = &server.toolset_config.default_config {
        tool.insert("default_config".to_string(), default_config.clone());
    }
    if let Some(configs) = &server.toolset_config.configs {
        tool.insert("configs".to_string(), configs.clone());
    }
    if let Some(allowed_tools) = &server.toolset_config.allowed_tools {
        tool.insert("allowed_tools".to_string(), json!(allowed_tools));
    }
    if let Some(denied_tools) = &server.toolset_config.denied_tools {
        tool.insert("denied_tools".to_string(), json!(denied_tools));
    }
    if let Some(defer_loading) = server.toolset_config.defer_loading {
        tool.insert("defer_loading".to_string(), json!(defer_loading));
    }
    if let Some(cache_control) = &server.toolset_config.cache_control {
        tool.insert("cache_control".to_string(), cache_control.clone());
    }
    Value::Object(tool)
}

fn valid_remote_mcp_servers(servers: &[ClaudeMcpServer]) -> Option<Vec<&ClaudeMcpServer>> {
    if servers.is_empty() {
        return None;
    }
    let mut names = BTreeSet::new();
    let mut valid = Vec::new();
    for server in servers {
        if server.name.trim().is_empty()
            || !server.url.starts_with("https://")
            || !names.insert(server.name.clone())
        {
            return None;
        }
        valid.push(server);
    }
    Some(valid)
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
        Some(namespace)
            if namespace.ends_with('_')
                || name.starts_with('_')
                || namespace
                    .chars()
                    .last()
                    .is_some_and(|ch| !ch.is_ascii_alphanumeric()) =>
        {
            format!("{namespace}{name}")
        }
        Some(namespace) => format!("{namespace}_{name}"),
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
    tool.insert(
        "type".to_string(),
        json!(ClaudeNativeToolKind::WebSearch20250305.tool_type()),
    );
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
